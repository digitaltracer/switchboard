use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::thread;

use super::*;

fn secret(dir: &Path, addr: std::net::SocketAddr) -> PathBuf {
    let path = dir.join("secret.json");
    SecretFile {
        addr,
        session_id: "s1".into(),
        token: "tok".into(),
    }
    .write(&path)
    .unwrap();
    path
}

/// A one-shot fake runner endpoint; `answer` gets the request and returns the raw reply.
fn fake_runner(
    answer: impl FnOnce(HookRequest) -> Option<String> + Send + 'static,
) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let req: HookRequest = serde_json::from_str(&line).unwrap();
        if let Some(reply) = answer(req) {
            let mut s = stream;
            s.write_all(reply.as_bytes()).unwrap();
        }
    });
    addr
}

#[test]
fn render_shapes() {
    let allow = HookReply::Allow {
        updated_input: Some(json!({"answers": {"q": "a"}})),
    };
    let claude = render(Agent::Claude, "PermissionRequest", &allow).unwrap();
    assert_eq!(
        claude["hookSpecificOutput"]["decision"]["behavior"],
        "allow"
    );
    assert_eq!(
        claude["hookSpecificOutput"]["decision"]["updatedInput"]["answers"]["q"],
        "a"
    );
    let codex = render(Agent::Codex, "PermissionRequest", &allow).unwrap();
    assert!(
        codex["hookSpecificOutput"]["decision"]
            .get("updatedInput")
            .is_none()
    );

    let deny = render(Agent::Codex, "PermissionRequest", &HookReply::deny("no")).unwrap();
    assert_eq!(
        deny["hookSpecificOutput"]["decision"],
        json!({"behavior": "deny", "message": "no"})
    );
    let ack = render(Agent::Claude, "PermissionRequest", &HookReply::Ack).unwrap();
    assert_eq!(
        ack["hookSpecificOutput"]["decision"]["behavior"], "deny",
        "ack is no decision"
    );
    assert_eq!(render(Agent::Claude, "SessionStart", &HookReply::Ack), None);
    assert_eq!(
        render(Agent::Claude, "SessionStart", &HookReply::deny("x")),
        None
    );
    let pre = render(
        Agent::Claude,
        "PreToolUse",
        &HookReply::Allow {
            updated_input: None,
        },
    )
    .unwrap();
    assert_eq!(
        pre["hookSpecificOutput"]["permissionDecision"], "deny",
        "never an allow"
    );
    assert_eq!(
        render(Agent::Claude, "PreToolUse", &HookReply::Ack),
        None,
        "before a tool, no deny pattern means no decision"
    );
    let pre = render(Agent::Claude, "PreToolUse", &HookReply::deny("x")).unwrap();
    assert_eq!(pre["hookSpecificOutput"]["permissionDecision"], "deny");
}

#[test]
fn round_trip_carries_session_token_and_payload() {
    let dir = tempfile::tempdir().unwrap();
    let addr = fake_runner(|req| {
        assert_eq!((req.session_id.as_str(), req.token.as_str()), ("s1", "tok"));
        assert_eq!(req.event, "PermissionRequest");
        assert_eq!(req.payload["tool_name"], "Bash");
        Some("{\"reply\":\"allow\",\"updated_input\":null}\n".into())
    });
    let payload = json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash"});
    let reply = ask_runner(
        &secret(dir.path(), addr),
        Agent::Codex,
        "PermissionRequest",
        &payload,
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(
        reply,
        HookReply::Allow {
            updated_input: None
        }
    );
}

#[test]
fn every_failure_is_an_error_the_caller_turns_into_deny() {
    let dir = tempfile::tempdir().unwrap();
    let payload = json!({});
    let ask = |addr, deadline| {
        ask_runner(
            &secret(dir.path(), addr),
            Agent::Claude,
            "PermissionRequest",
            &payload,
            deadline,
        )
    };

    let closes = fake_runner(|_| None);
    assert!(
        ask(closes, Duration::from_secs(5))
            .unwrap_err()
            .contains("bad answer")
    );

    let garbage = fake_runner(|_| Some("not json\n".into()));
    assert!(ask(garbage, Duration::from_secs(5)).is_err());

    let slow = fake_runner(|_| {
        thread::sleep(Duration::from_secs(3));
        Some("{\"reply\":\"allow\",\"updated_input\":null}\n".into())
    });
    let t = Instant::now();
    assert!(
        ask(slow, Duration::from_millis(300))
            .unwrap_err()
            .contains("no answer")
    );
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "helper keeps its own deadline"
    );

    let unused = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    assert!(
        ask(unused, Duration::from_secs(1))
            .unwrap_err()
            .contains("unreachable")
    );

    let missing = ask_runner(
        &dir.path().join("nope"),
        Agent::Claude,
        "PermissionRequest",
        &payload,
        Duration::from_secs(1),
    );
    assert!(missing.unwrap_err().contains("secret file"));
}

#[test]
fn secret_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = secret(dir.path(), "127.0.0.1:1".parse().unwrap());
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(SecretFile::read(&path).unwrap().session_id, "s1");
}
