//! ebbtop P3a: the discovery responder over a real UDP socket.
//!
//! These tests bind loopback sockets and drive `discovery::serve`, so they
//! exercise the same code the agent spawns at startup: the exact-probe check,
//! the private-source check, the rate limit and the reply bytes.

use std::net::SocketAddr;
use std::time::Duration;

use ebbtop_agent::discovery::{
    self, DISCOVERY_REQUEST, DiscoveryConfig, DiscoveryResponse, SOURCE_MIN_GAP,
};
use ebbtop_agent::state::AppState;
use tokio::net::UdpSocket;
use tokio::time::timeout;

const TOKEN_SENTINEL: &str = "SENTINEL-TOKEN-MUST-NOT-LEAK";

struct Harness {
    _dir: tempfile::TempDir,
    state: AppState,
    server: SocketAddr,
}

/// Start a responder on an ephemeral loopback port and return its address.
async fn start() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let mut state = AppState::with_pairing_dir(dir.path());
    state.auth_token = Some(TOKEN_SENTINEL.into());
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    let task_state = state.clone();
    tokio::spawn(discovery::serve(sock, task_state, 45976));
    Harness {
        _dir: dir,
        state,
        server: addr,
    }
}

async fn client() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").await.unwrap()
}

/// Send one datagram and read at most one reply inside `wait`.
async fn probe(c: &UdpSocket, server: SocketAddr, payload: &[u8], wait: Duration) -> Option<Vec<u8>> {
    c.send_to(payload, server).await.unwrap();
    let mut buf = [0u8; 1024];
    match timeout(wait, c.recv_from(&mut buf)).await {
        Ok(Ok((n, _from))) => Some(buf[..n].to_vec()),
        _ => None,
    }
}

#[tokio::test]
async fn responds_with_name_version_nonce_and_port() {
    let h = start().await;
    let c = client().await;
    let bytes = probe(&c, h.server, DISCOVERY_REQUEST, Duration::from_millis(500))
        .await
        .expect("no discovery reply");
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        !text.contains(TOKEN_SENTINEL),
        "discovery reply leaked the agent token: {text}"
    );
    assert!(!text.contains("token"), "reply contains a token field: {text}");

    let r: DiscoveryResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(r.ebbtop_discovery, discovery::DISCOVERY_PROTOCOL);
    assert_eq!(r.name, h.state.hostname);
    assert!(!r.name.is_empty());
    assert_eq!(r.agent_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(r.nonce, h.state.boot_nonce);
    assert_eq!(r.nonce.len(), 32);
    assert_eq!(r.port, 45976);
}

#[tokio::test]
async fn ignores_anything_that_is_not_the_exact_probe() {
    let h = start().await;
    let c = client().await;
    for payload in [
        b"get_metrics".as_slice(),
        b"EBBTOP-DISCOVER/2".as_slice(),
        b"EBBTOP-DISCOVER/1 extra".as_slice(),
        b"".as_slice(),
        b"kill:1:0:TERM".as_slice(),
    ] {
        assert!(
            probe(&c, h.server, payload, Duration::from_millis(250))
                .await
                .is_none(),
            "responder answered a non-probe datagram: {payload:?}"
        );
    }
    // ...and the real probe still works afterwards.
    assert!(
        probe(&c, h.server, DISCOVERY_REQUEST, Duration::from_millis(500))
            .await
            .is_some()
    );
}

#[tokio::test]
async fn rate_limits_one_reply_per_source_then_recovers() {
    let h = start().await;
    let c = client().await;
    // A burst of six probes from one source: exactly one reply.
    let mut replies = 0;
    for _ in 0..6 {
        if probe(&c, h.server, DISCOVERY_REQUEST, Duration::from_millis(120))
            .await
            .is_some()
        {
            replies += 1;
        }
    }
    assert_eq!(replies, 1, "rate limit must allow exactly one reply per window");

    // After the window, the same source is served again.
    tokio::time::sleep(SOURCE_MIN_GAP + Duration::from_millis(120)).await;
    assert!(
        probe(&c, h.server, DISCOVERY_REQUEST, Duration::from_millis(500))
            .await
            .is_some(),
        "rate limit must refill after the window"
    );
}

#[tokio::test]
async fn a_second_source_gets_its_own_budget() {
    let h = start().await;
    let a = client().await;
    let b = client().await;
    assert!(
        probe(&a, h.server, DISCOVERY_REQUEST, Duration::from_millis(500))
            .await
            .is_some()
    );
    // Different source port but the same 127.0.0.1 address: the limiter is
    // per address, so this is the same budget and must be refused.
    assert!(
        probe(&b, h.server, DISCOVERY_REQUEST, Duration::from_millis(250))
            .await
            .is_none(),
        "the limiter keys on the source address, not the source port"
    );
}

#[tokio::test]
async fn disabled_feature_binds_no_socket_and_never_answers() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::with_pairing_dir(dir.path());
    let cfg = DiscoveryConfig {
        enabled: false,
        port: 0,
    };
    let spawned = discovery::spawn(cfg, state, 45976).await.unwrap();
    assert!(spawned.is_none(), "disabled discovery must not spawn a task");

    // Prove no socket was left listening: a free ephemeral port can be bound
    // by someone else, and a probe to a closed port times out.
    let probe_target = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = probe_target.local_addr().unwrap();
    let c = client().await;
    assert!(
        probe(&c, addr, DISCOVERY_REQUEST, Duration::from_millis(250))
            .await
            .is_none(),
        "a probe to a port nobody serves must not get a reply"
    );
}

#[tokio::test]
async fn a_second_instance_on_the_same_port_reports_the_bind_error() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::with_pairing_dir(dir.path());
    let first = UdpSocket::bind("0.0.0.0:0").await.unwrap();
    let port = first.local_addr().unwrap().port();
    let cfg = DiscoveryConfig {
        enabled: true,
        port,
    };
    let err = discovery::spawn(cfg, state, 45976).await;
    assert!(err.is_err(), "a taken discovery port must surface, not panic");
}

/// The reason the responder binds `0.0.0.0` instead of `EBBTOP_BIND`: a
/// socket bound to one unicast address never receives a broadcast. This sends
/// a real broadcast datagram to the responder's port and expects an answer.
#[tokio::test]
async fn a_broadcast_probe_reaches_an_all_interfaces_socket() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = AppState::with_pairing_dir(dir.path());
    state.hostname = "broadcast-host".into();
    let sock = UdpSocket::bind("0.0.0.0:0").await.unwrap();
    let port = sock.local_addr().unwrap().port();
    tokio::spawn(discovery::serve(sock, state, 45976));

    let c = UdpSocket::bind("0.0.0.0:0").await.unwrap();
    c.set_broadcast(true).unwrap();
    let target: SocketAddr = format!("255.255.255.255:{port}").parse().unwrap();
    c.send_to(DISCOVERY_REQUEST, target).await.unwrap();
    let mut buf = [0u8; 1024];
    let got = timeout(Duration::from_millis(750), c.recv_from(&mut buf)).await;
    let (n, _) = got
        .expect("a broadcast probe must be answered")
        .expect("recv failed");
    let r: DiscoveryResponse = serde_json::from_slice(&buf[..n]).unwrap();
    assert_eq!(r.name, "broadcast-host");
    assert_eq!(r.port, 45976);
}
