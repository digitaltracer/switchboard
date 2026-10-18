use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use super::auth::*;
use super::conn::*;
use super::*;

fn hello(machine: &str, version: u32) -> Frame {
    Frame::Hello(Hello {
        machine_id: machine.into(),
        protocol_version: version,
        runner_version: "0.1.0".into(),
        herdr_version: Some("0.9.1".into()),
        sessions: vec![OwnedSession {
            session_id: "s1".into(),
            workspace_id: "w2".into(),
            pane_id: Some("w2:p1".into()),
            agent_status: "idle".into(),
            hook_failed: false,
        }],
    })
}

fn start_op() -> Op {
    Op::StartSession {
        session_id: "s1".into(),
        project_id: "switchboard".into(),
        repo_path: "/srv/switchboard".into(),
        branch: "sb/s1".into(),
        agent: Agent::Codex,
        prompt: "fix it".into(),
        deny_patterns: vec!["git push".into()],
    }
}

#[test]
fn every_frame_round_trips() {
    let frames = vec![
        hello("mac", PROTOCOL_VERSION),
        Frame::Req(Req {
            id: 1,
            deadline_ms: 30_000,
            op: start_op(),
        }),
        Frame::Req(Req {
            id: 2,
            deadline_ms: 5_000,
            op: Op::ListSessions,
        }),
        Frame::Req(Req {
            id: 3,
            deadline_ms: 5_000,
            op: Op::Decide {
                session_id: "s1".into(),
                approval_id: "a1".into(),
                allow: false,
            },
        }),
        Frame::Resp(Resp {
            id: 1,
            outcome: Outcome::Ok(json!({"branch": "sb/s1"})),
        }),
        Frame::Resp(Resp {
            id: 2,
            outcome: Outcome::Err(ProtoError::new(
                ErrorCode::HooksUntrusted,
                "hook never answered",
            )),
        }),
        Frame::Event(Event::ApprovalRequested {
            session_id: "s1".into(),
            approval_id: "a1".into(),
            tool_name: "Bash".into(),
            tool_input: json!({"command": "rm -rf build"}),
            deadline_ms: 50_000,
        }),
        Frame::Event(Event::AgentStatus {
            session_id: "s1".into(),
            agent_status: "blocked".into(),
            revision: 3,
        }),
        Frame::Event(Event::Resynced { sessions: vec![] }),
    ];
    for f in frames {
        assert_eq!(decode(&encode(&f)).unwrap(), f);
    }
}

#[test]
fn wire_format_is_stable() {
    let req = Frame::Req(Req {
        id: 7,
        deadline_ms: 1000,
        op: Op::Read {
            session_id: "s1".into(),
            lines: 40,
        },
    });
    assert_eq!(
        serde_json::to_value(&req).unwrap(),
        json!({"type": "req", "id": 7, "deadline_ms": 1000, "op": {"op": "read", "session_id": "s1", "lines": 40}})
    );
    let resp = Frame::Resp(Resp {
        id: 7,
        outcome: Outcome::Err(ProtoError::new(ErrorCode::OutcomeUnknown, "lost")),
    });
    assert_eq!(
        serde_json::to_value(&resp).unwrap(),
        json!({"type": "resp", "id": 7, "err": {"code": "outcome_unknown", "message": "lost"}})
    );
    let ok = Frame::Resp(Resp {
        id: 8,
        outcome: Outcome::Ok(json!(null)),
    });
    assert_eq!(
        serde_json::to_value(&ok).unwrap(),
        json!({"type": "resp", "id": 8, "ok": null})
    );
}

#[test]
fn unknown_error_code_decodes_as_unknown() {
    let f = decode(r#"{"type":"resp","id":1,"err":{"code":"brand_new","message":"x"}}"#).unwrap();
    let Frame::Resp(Resp {
        outcome: Outcome::Err(e),
        ..
    }) = f
    else {
        panic!()
    };
    assert_eq!(e.code, ErrorCode::Unknown);
}

#[test]
fn decode_rejects_oversized_and_malformed() {
    let big = format!(
        r#"{{"type":"event","event":"session_exited","session_id":"{}"}}"#,
        "x".repeat(MAX_FRAME_BYTES)
    );
    assert!(matches!(decode(&big), Err(CodecError::TooLarge(_))));
    assert!(matches!(decode("{"), Err(CodecError::Malformed(_))));
    assert!(matches!(
        decode(r#"{"type":"teleport"}"#),
        Err(CodecError::Malformed(_))
    ));
}

#[test]
fn hello_checks_version_machine_and_order() {
    assert!(accept_hello(&encode(&hello("mac", PROTOCOL_VERSION)), "mac").is_ok());
    assert!(matches!(
        accept_hello(&encode(&hello("mac", PROTOCOL_VERSION + 1)), "mac"),
        Err(CodecError::VersionMismatch { .. })
    ));
    assert!(matches!(
        accept_hello(&encode(&hello("vps", PROTOCOL_VERSION)), "mac"),
        Err(CodecError::WrongMachine { .. })
    ));
    let req = Frame::Req(Req {
        id: 1,
        deadline_ms: 1,
        op: Op::ListSessions,
    });
    assert!(matches!(
        accept_hello(&encode(&req), "mac"),
        Err(CodecError::ExpectedHello)
    ));
}

#[tokio::test(start_paused = true)]
async fn correlator_delivers_reply_on_current_generation() {
    let mut c = Correlator::new();
    let generation = c.connected();
    let (g, req, rx) = c
        .register(Op::ListSessions, Duration::from_secs(5), Instant::now())
        .unwrap();
    assert_eq!((g, req.deadline_ms), (generation, 5000));
    assert!(c.complete(
        g,
        Resp {
            id: req.id,
            outcome: Outcome::Ok(json!([1]))
        }
    ));
    assert_eq!(rx.await.unwrap().unwrap(), json!([1]));
    assert!(
        !c.complete(
            g,
            Resp {
                id: req.id,
                outcome: Outcome::Ok(json!(null))
            }
        ),
        "already completed"
    );
}

#[tokio::test(start_paused = true)]
async fn stale_generation_reply_is_dropped_and_inflight_becomes_outcome_unknown() {
    let mut c = Correlator::new();
    let g1 = c.connected();
    let (_, start, start_rx) = c
        .register(start_op(), Duration::from_secs(30), Instant::now())
        .unwrap();
    let (_, read, read_rx) = c
        .register(
            Op::Read {
                session_id: "s1".into(),
                lines: 10,
            },
            Duration::from_secs(30),
            Instant::now(),
        )
        .unwrap();

    let g2 = c.connected();
    assert_ne!(g1, g2);
    assert_eq!(
        start_rx.await.unwrap().unwrap_err().code,
        ErrorCode::OutcomeUnknown
    );
    assert_eq!(
        read_rx.await.unwrap().unwrap_err().code,
        ErrorCode::RunnerDisconnected
    );
    assert!(!c.complete(
        g1,
        Resp {
            id: start.id,
            outcome: Outcome::Ok(json!(null))
        }
    ));
    assert!(!c.complete(
        g2,
        Resp {
            id: read.id,
            outcome: Outcome::Ok(json!(null))
        }
    ));
    assert_eq!(
        c.in_flight(),
        0,
        "nothing is replayed on the new connection"
    );
}

#[tokio::test(start_paused = true)]
async fn deadlines_expire_with_paused_clock() {
    let mut c = Correlator::new();
    let g = c.connected();
    let t0 = Instant::now();
    let (_, _, decide_rx) = c
        .register(
            Op::Decide {
                session_id: "s1".into(),
                approval_id: "a1".into(),
                allow: true,
            },
            Duration::from_secs(2),
            t0,
        )
        .unwrap();
    let (_, late, late_rx) = c
        .register(Op::ListSessions, Duration::from_secs(10), t0)
        .unwrap();
    assert_eq!(c.next_deadline(), Some(t0 + Duration::from_secs(2)));

    tokio::time::advance(Duration::from_secs(3)).await;
    assert_eq!(c.expire(Instant::now()), 1);
    assert_eq!(
        decide_rx.await.unwrap().unwrap_err().code,
        ErrorCode::OutcomeUnknown
    );
    assert!(c.complete(
        g,
        Resp {
            id: late.id,
            outcome: Outcome::Ok(json!("ok"))
        }
    ));
    assert_eq!(late_rx.await.unwrap().unwrap(), json!("ok"));
}

#[test]
fn in_flight_is_bounded() {
    let mut c = Correlator::new();
    c.connected();
    let now = Instant::now();
    let _keep: Vec<_> = (0..MAX_IN_FLIGHT)
        .map(|_| {
            c.register(Op::ListSessions, Duration::from_secs(1), now)
                .unwrap()
        })
        .collect();
    let err = c
        .register(Op::ListSessions, Duration::from_secs(1), now)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Busy);
}

#[test]
fn backoff_grows_to_cap_with_jitter_bounds() {
    let mut b = Backoff::new(Duration::from_millis(500), Duration::from_secs(30));
    let ceilings: Vec<_> = (0..10)
        .map(|_| {
            let c = b.ceiling();
            assert!(b.next_with(0.999) <= c);
            c
        })
        .collect();
    assert_eq!(ceilings[0], Duration::from_millis(500));
    assert_eq!(ceilings[1], Duration::from_secs(1));
    assert_eq!(ceilings[9], Duration::from_secs(30));
    assert_eq!(b.next_with(0.0), Duration::ZERO);
    b.reset();
    assert_eq!(b.ceiling(), Duration::from_millis(500));
    assert!(b.next_delay() <= Duration::from_millis(500));
}

#[test]
fn token_hashing_and_constant_time_match() {
    assert_eq!(
        hash_token("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let h = hash_token("s3cret");
    assert!(token_matches("s3cret", &h));
    assert!(!token_matches("s3cret ", &h));
    assert!(!token_matches("s3cret", "zz"));
    assert!(!token_matches("s3cret", &h[..62]));
}

#[test]
fn bearer_and_authenticate() {
    assert_eq!(bearer("Bearer tok"), Some("tok"));
    assert_eq!(bearer("bearer  tok "), Some("tok"));
    assert_eq!(bearer("Basic tok"), None);
    assert_eq!(bearer("Bearer "), None);
    let (mac, vps) = (hash_token("m"), hash_token("v"));
    let machines = [("mac", mac.as_str()), ("vps", vps.as_str())];
    assert_eq!(authenticate("v", machines), Some("vps"));
    assert_eq!(authenticate("x", machines), None);
}
