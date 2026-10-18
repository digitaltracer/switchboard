use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;

use super::*;
use crate::proto::Req;

#[derive(Default)]
struct FakeHandler {
    ops: Mutex<Vec<Op>>,
}

impl Handler for FakeHandler {
    async fn handle(&self, op: Op) -> Result<Value, ProtoError> {
        self.ops.lock().unwrap().push(op);
        Ok(json!({"handled": true}))
    }

    async fn owned(&self) -> Vec<OwnedSession> {
        vec![OwnedSession {
            session_id: "s1".into(),
            workspace_id: "w2".into(),
            pane_id: Some("w2:p1".into()),
            agent_status: "idle".into(),
            hook_failed: false,
        }]
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

/// Accepts one connection, checking the bearer token like the hub will.
#[allow(clippy::result_large_err)] // tungstenite's callback signature
async fn accept(listener: &TcpListener, token: &'static str) -> Option<Ws> {
    let (stream, _) = listener.accept().await.unwrap();
    let check = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let ok = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(crate::proto::auth::bearer)
            == Some(token);
        if ok {
            Ok(resp)
        } else {
            let mut e = ErrorResponse::new(None);
            *e.status_mut() = StatusCode::UNAUTHORIZED;
            Err(e)
        }
    };
    tokio_tungstenite::accept_hdr_async(stream, check)
        .await
        .ok()
}

async fn recv(ws: &mut Ws) -> Frame {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Text(t) = msg {
            return decode(&t).unwrap();
        }
    }
}

async fn start(token: &str) -> (TcpListener, Arc<FakeHandler>, mpsc::Sender<Event>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("ws://{}/runner", listener.local_addr().unwrap())).unwrap();
    let handler = Arc::new(FakeHandler::default());
    let (tx, rx) = mpsc::channel(16);
    let cfg = LinkConfig {
        url,
        token: token.into(),
        machine_id: "mac".into(),
        herdr_version: Some("0.9.1".into()),
    };
    tokio::spawn(run(cfg, handler.clone(), rx));
    (listener, handler, tx)
}

#[tokio::test]
async fn hello_request_response_events_and_reconnect() {
    let (listener, handler, events) = start("runner-token").await;
    let mut ws = accept(&listener, "runner-token").await.unwrap();

    let Frame::Hello(hello) = recv(&mut ws).await else {
        panic!("hello first")
    };
    assert_eq!(
        (hello.machine_id.as_str(), hello.protocol_version),
        ("mac", PROTOCOL_VERSION)
    );
    assert_eq!(hello.sessions[0].session_id, "s1");

    let req = Frame::Req(Req {
        id: 42,
        deadline_ms: 5000,
        op: Op::ListSessions,
    });
    ws.send(Message::text(encode(&req))).await.unwrap();
    let Frame::Resp(resp) = recv(&mut ws).await else {
        panic!("response")
    };
    assert_eq!(
        resp,
        Resp {
            id: 42,
            outcome: Outcome::Ok(json!({"handled": true}))
        }
    );
    assert_eq!(handler.ops.lock().unwrap().as_slice(), [Op::ListSessions]);

    events
        .send(Event::SessionExited {
            session_id: "s1".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        recv(&mut ws).await,
        Frame::Event(Event::SessionExited {
            session_id: "s1".into()
        })
    );

    // Hub goes away; an event raised while the runner is disconnected is delivered after it reconnects.
    drop(ws);
    tokio::time::sleep(Duration::from_millis(300)).await;
    events
        .send(Event::SessionExited {
            session_id: "s2".into(),
        })
        .await
        .unwrap();
    let mut ws = accept(&listener, "runner-token").await.unwrap();
    assert!(
        matches!(recv(&mut ws).await, Frame::Hello(_)),
        "hello again on the new connection"
    );
    assert_eq!(
        recv(&mut ws).await,
        Frame::Event(Event::SessionExited {
            session_id: "s2".into()
        })
    );
}

#[tokio::test]
async fn rejected_token_is_retried_with_backoff() {
    let (listener, _handler, _events) = start("wrong").await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let t0 = Instant::now();
    for _ in 0..2 {
        assert!(accept(&listener, "runner-token").await.is_none());
        attempts.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(t0.elapsed() < Duration::from_secs(5));
}
