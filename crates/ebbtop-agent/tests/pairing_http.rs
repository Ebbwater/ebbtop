//! ebbtop P3a: `POST /pair` over real HTTP and the WebSocket token gate.
//!
//! The router is the production one (`ebbtop_agent::server::router`) served on
//! an ephemeral loopback port, so these tests cover the same route table the
//! agent runs.

use std::net::SocketAddr;
use std::time::Duration;

use ebbtop_agent::pairing::{CLIENTS_FILE, MAX_ATTEMPTS, PAIR_FILE, SCOPE_READ_ONLY, now_ms};
use ebbtop_agent::server;
use ebbtop_agent::state::{AppState, AuthScope};
use ebbtop_core::pairing::PairResponse;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

const MANUAL_TOKEN: &str = "manual-operator-token-for-tests";

struct Harness {
    _dir: tempfile::TempDir,
    state: AppState,
    addr: SocketAddr,
}

async fn start() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let mut state = AppState::with_pairing_dir(dir.path());
    state.auth_token = Some(MANUAL_TOKEN.into());
    // Zero pair-rate gaps: these tests post back-to-back from one source and
    // exercise the enrollment rules, not the limiter (which has its own
    // end-to-end test below and unit tests in pairing.rs).
    state.pair_rate = std::sync::Arc::new(std::sync::Mutex::new(
        ebbtop_agent::pairing::PairRateLimit::with_gaps(
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
        ),
    ));
    let app = server::router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    Harness {
        _dir: dir,
        state,
        addr,
    }
}

/// One-shot HTTP/1.0 POST. HTTP/1.0 keeps the body close-delimited (no
/// chunked transfer to decode) which is all these tests need.
async fn post(addr: SocketAddr, path: &str, body: &str) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "POST {path} HTTP/1.0\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let text = String::from_utf8_lossy(&out).to_string();
    let (head, bodytxt) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    (status, head.to_string(), bodytxt.to_string())
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("not JSON: {e}: {body:?}"))
}

#[tokio::test]
async fn a_code_is_exchanged_once_for_a_read_only_token() {
    let h = start().await;
    let pending = h.state.pairing.mint().unwrap();
    let (status, head, body) = post(
        h.addr,
        "/pair",
        &format!(r#"{{"code":"{}","label":"kyler-laptop"}}"#, pending.code),
    )
    .await;
    assert_eq!(status, 200, "body: {body}");
    assert!(
        head.to_ascii_lowercase()
            .contains("cache-control: no-store")
    );

    let resp: PairResponse = serde_json::from_str(&body).unwrap();
    assert_eq!(resp.scope, SCOPE_READ_ONLY);
    assert_eq!(resp.host, h.state.hostname);
    assert_eq!(resp.agent_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(resp.nonce.as_deref(), Some(h.state.boot_nonce.as_str()));
    assert_eq!(resp.token.len(), 64);
    assert!(resp.token.chars().all(|c| c.is_ascii_hexdigit()));
    // the code itself never comes back
    assert!(!body.contains(&pending.code), "response echoed the code");

    // Same credential store and validation as the manual token.
    let stored = h.state.pairing.authenticate(&resp.token);
    assert_eq!(stored.as_deref(), Some(SCOPE_READ_ONLY));
    assert_eq!(
        h.state.authenticate(Some(&resp.token)),
        Some(AuthScope::ReadOnly)
    );

    // Single use: the pending file is gone and a replay is a 404.
    assert!(!h.state.pairing.pair_file().exists());
    let (again, _, body2) = post(
        h.addr,
        "/pair",
        &format!(r#"{{"code":"{}"}}"#, pending.code),
    )
    .await;
    assert_eq!(again, 404, "body: {body2}");
    assert!(body2.contains("no pending pairing code"));
}

#[tokio::test]
async fn a_wrong_code_is_refused_and_issues_nothing() {
    let h = start().await;
    let pending = h.state.pairing.mint().unwrap();
    let wrong = if pending.code == "000000" {
        "111111"
    } else {
        "000000"
    };
    let (status, _, body) = post(h.addr, "/pair", &format!(r#"{{"code":"{wrong}"}}"#)).await;
    assert_eq!(status, 403);
    assert!(body.contains("invalid pairing code"));
    assert!(
        !h.state.pairing.clients_file().exists(),
        "no token for a wrong code"
    );
    // a typo does not burn the enrollment: the right code still works
    let (ok, _, body) = post(
        h.addr,
        "/pair",
        &format!(r#"{{"code":"{}"}}"#, pending.code),
    )
    .await;
    assert_eq!(ok, 200, "body: {body}");
}

#[tokio::test]
async fn an_expired_code_is_refused() {
    let h = start().await;
    let pending = ebbtop_agent::pairing::PendingCode {
        code: "424242".into(),
        expires_at_ms: now_ms() - 1,
        attempts: 0,
    };
    std::fs::create_dir_all(h.state.pairing.dir()).unwrap();
    std::fs::write(
        h.state.pairing.pair_file(),
        serde_json::to_vec(&pending).unwrap(),
    )
    .unwrap();
    let (status, _, body) = post(h.addr, "/pair", r#"{"code":"424242"}"#).await;
    assert_eq!(status, 403);
    assert!(body.contains("expired"), "body: {body}");
    assert!(!h.state.pairing.pair_file().exists());
}

#[tokio::test]
async fn a_replayed_code_is_refused_after_the_first_exchange() {
    let h = start().await;
    let pending = h.state.pairing.mint().unwrap();
    let body = format!(r#"{{"code":"{}"}}"#, pending.code);
    assert_eq!(post(h.addr, "/pair", &body).await.0, 200);
    // the second exchange (the replay) is refused; exactly one token exists
    assert_eq!(post(h.addr, "/pair", &body).await.0, 404);
    let clients = h.state.pairing.read_clients().clients;
    assert_eq!(clients.len(), 1);
}

#[tokio::test]
async fn five_wrong_guesses_lock_the_pending_code() {
    let h = start().await;
    let pending = h.state.pairing.mint().unwrap();
    let wrong = if pending.code == "000000" {
        "111111"
    } else {
        "000000"
    };
    let body = format!(r#"{{"code":"{wrong}"}}"#);
    for n in 1..MAX_ATTEMPTS {
        let (status, _, _) = post(h.addr, "/pair", &body).await;
        assert_eq!(status, 403, "attempt {n}");
    }
    let (status, _, text) = post(h.addr, "/pair", &body).await;
    assert_eq!(status, 429);
    assert!(text.contains("too many attempts"));
    // even the real code is dead now
    let (status, _, _) = post(
        h.addr,
        "/pair",
        &format!(r#"{{"code":"{}"}}"#, pending.code),
    )
    .await;
    assert_eq!(status, 404);
    assert!(!h.state.pairing.pair_file().exists());
}

#[tokio::test]
async fn no_pending_code_is_a_404_and_a_query_string_code_is_refused() {
    let h = start().await;
    assert_eq!(post(h.addr, "/pair", r#"{"code":"123456"}"#).await.0, 404);

    // A code in the query string is not read, and not echoed back.
    let pending = h.state.pairing.mint().unwrap();
    let (status, _, body) = post(h.addr, &format!("/pair?code={}", pending.code), "{}").await;
    assert_eq!(status, 400, "a body-less pair request must be refused");
    assert!(!body.contains(&pending.code), "error echoed the code");

    // Malformed JSON is refused without echoing the code either.
    let (status, _, body) = post(h.addr, "/pair", "not json").await;
    assert_eq!(status, 400);
    assert!(!body.contains(&pending.code));
    assert!(h.state.pairing.read_pending().is_some(), "nothing consumed");
}

#[tokio::test]
async fn healthz_stays_open_and_unchanged() {
    let h = start().await;
    let mut s = TcpStream::connect(h.addr).await.unwrap();
    s.write_all(b"GET /healthz HTTP/1.0\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(text.starts_with("HTTP/1."), "{text}");
    assert!(text.contains(" 200 "), "{text}");
}

// ---------------------------------------------------------------- WS gate

async fn ws_connect(
    addr: SocketAddr,
    token: Option<&str>,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>> {
    let url = match token {
        Some(t) => format!("ws://{addr}/ws?token={t}"),
        None => format!("ws://{addr}/ws"),
    };
    let (ws, _resp) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws
}

/// Read one frame within a short deadline.
async fn next_frame(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
) -> Option<Message> {
    match timeout(Duration::from_secs(5), ws.next()).await {
        Ok(Some(Ok(m))) => Some(m),
        _ => None,
    }
}

#[tokio::test]
async fn the_manual_token_keeps_full_scope() {
    let h = start().await;
    let mut ws = ws_connect(h.addr, Some(MANUAL_TOKEN)).await;
    ws.send(Message::Text("get_metrics".into())).await.unwrap();
    let m = next_frame(&mut ws).await.expect("no metrics reply");
    assert!(matches!(m, Message::Text(_) | Message::Binary(_)));

    // A malformed kill reaches the kill handler (grammar refusal), proving
    // the manual token still has the full path it had before P3a.
    ws.send(Message::Text("kill".into())).await.unwrap();
    let m = next_frame(&mut ws).await.expect("no kill reply");
    let Message::Text(t) = m else {
        panic!("kill reply must be text")
    };
    assert!(t.contains("bad request grammar"), "{t}");
}

#[tokio::test]
async fn an_issued_token_authenticates_but_cannot_kill() {
    let h = start().await;
    let rec = h.state.pairing.issue(Some("laptop")).unwrap();
    let mut ws = ws_connect(h.addr, Some(&rec.token)).await;
    // monitoring works
    ws.send(Message::Text("get_metrics".into())).await.unwrap();
    assert!(
        next_frame(&mut ws).await.is_some(),
        "issued token must reach metrics"
    );
    // kill does not
    ws.send(Message::Text("kill:1:0:TERM".into()))
        .await
        .unwrap();
    let m = next_frame(&mut ws).await.expect("no kill reply");
    let Message::Text(t) = m else {
        panic!("kill reply must be text")
    };
    let v = json(&t);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"], "read-only token scope");
    assert!(
        !t.contains("pid 1"),
        "the kill path must not even be reached: {t}"
    );
}

#[tokio::test]
async fn a_wrong_or_missing_token_is_closed() {
    let h = start().await;
    // wrong token
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{}/ws?token=definitely-wrong", h.addr))
            .await
            .unwrap();
    let got = timeout(Duration::from_secs(3), ws.next()).await;
    assert!(
        matches!(got, Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Err(_)),
        "wrong token must not keep the connection: {got:?}"
    );

    // no token at all
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", h.addr))
        .await
        .unwrap();
    let got = timeout(Duration::from_secs(3), ws.next()).await;
    assert!(
        matches!(got, Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Err(_)),
        "missing token must not keep the connection: {got:?}"
    );
}

#[tokio::test]
async fn pairing_writes_files_next_to_the_agent_state() {
    let h = start().await;
    let pending = h.state.pairing.mint().unwrap();
    assert!(h.state.pairing.pair_file().ends_with(PAIR_FILE));
    assert_eq!(
        h.state.pairing.clients_file().file_name().unwrap(),
        CLIENTS_FILE
    );
    assert_eq!(
        post(
            h.addr,
            "/pair",
            &format!(r#"{{"code":"{}"}}"#, pending.code)
        )
        .await
        .0,
        200
    );
    // the code is not persisted anywhere after the exchange
    let left = std::fs::read_dir(h.state.pairing.dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert!(left.contains(&CLIENTS_FILE.to_string()));
    let raw_clients = std::fs::read_to_string(h.state.pairing.clients_file()).unwrap();
    assert!(!raw_clients.contains(&pending.code));
}

#[tokio::test]
async fn pair_requests_are_rate_limited_per_source() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = AppState::with_pairing_dir(dir.path());
    state.auth_token = Some(MANUAL_TOKEN.into());
    // Tiny but nonzero gaps so the third rapid post is refused in real time.
    state.pair_rate = std::sync::Arc::new(std::sync::Mutex::new(
        ebbtop_agent::pairing::PairRateLimit::with_gaps(
            std::time::Duration::from_millis(250),
            std::time::Duration::from_millis(50),
        ),
    ));
    let app = server::router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    // A minted code exists so refusals are the limiter's, not the store's.
    let _pending = state.pairing.mint().unwrap();
    let (first, _, body1) = post(addr, "/pair", r#"{"code":"000000"}"#).await;
    assert_ne!(first, 429, "first request must pass the limiter: {body1}");
    let (second, _, _) = post(addr, "/pair", r#"{"code":"000000"}"#).await;
    assert_eq!(second, 429, "a rapid second request is rate-limited");
    let (third, _, body3) = post(addr, "/pair", r#"{"code":"000000"}"#).await;
    assert_eq!(third, 429, "body: {body3}");
    assert!(body3.contains("slow down"), "body: {body3}");
    // After the source gap the source passes again.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (later, _, _) = post(addr, "/pair", r#"{"code":"000000"}"#).await;
    assert_ne!(later, 429, "after the gap the source is served again");
}

#[tokio::test]
async fn an_oversized_label_is_refused_before_any_exchange() {
    let h = start().await;
    let pending = h.state.pairing.mint().unwrap();
    let label = "x".repeat(129);
    let (status, _, body) = post(
        h.addr,
        "/pair",
        &format!(r#"{{"code":"{}","label":"{label}"}}"#, pending.code),
    )
    .await;
    assert_eq!(status, 400, "body: {body}");
    assert!(body.contains("label too long"), "body: {body}");
    // The refusal must not burn the enrollment: the right code still works.
    let (ok, _, body) = post(
        h.addr,
        "/pair",
        &format!(r#"{{"code":"{}","label":"ok"}}"#, pending.code),
    )
    .await;
    assert_eq!(ok, 200, "body: {body}");
}
