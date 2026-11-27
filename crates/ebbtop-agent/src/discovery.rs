//! ebbtop P3a: read-only LAN discovery responder.
//!
//! The agent answers one UDP datagram per probe on a dedicated port
//! (default [`DISCOVERY_PORT`]) with its display name, build version and
//! random per-boot nonce. That is the entire surface: no token, no command,
//! no host inventory, no process data — the probe cannot ask a question, so
//! there is nothing to enumerate beyond "an ebbtop agent is at this address".
//!
//! Guards, in the order they run:
//! 1. the datagram must be exactly [`DISCOVERY_REQUEST`] (trailing CR/LF ok);
//! 2. the source must be private/loopback/link-local/CGNAT — a public source
//!    gets nothing, so the responder cannot be used off-network;
//! 3. a rate limit: at most one reply per source per second and 20 replies
//!    per second overall, so a flood cannot turn the agent into a reflector.
//!
//! Feature flag: `EBBTOP_AGENT_DISCOVERY=0` (also `false`/`no`/`off`)
//! disables it entirely — no socket is bound.
//!
//! The decoder for these bytes lives in `ebbtop_core::pairing` (the crates
//! do not depend on each other: the agent owns the encoder types exactly as
//! `types::Metrics` is mirrored by `wire::Metrics`). Both sides pin the same
//! JSON fixture in tests, so a field rename fails on both.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

use crate::state::AppState;

/// Exact datagram a client broadcasts to find ebbtop agents. Anything else
/// (including a longer or padded packet) is ignored.
pub const DISCOVERY_REQUEST: &[u8] = b"EBBTOP-DISCOVER/1";
/// UDP port the responder listens on, next to the agent's TCP port 45976.
pub const DISCOVERY_PORT: u16 = 45978;
/// Protocol marker in every reply; a client ignores any other number.
pub const DISCOVERY_PROTOCOL: u32 = 1;

/// JSON reply. The three identity fields are the whole unauthenticated
/// surface; `port` makes a discovered host immediately usable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryResponse {
    pub ebbtop_discovery: u32,
    pub name: String,
    pub agent_version: String,
    pub nonce: String,
    #[serde(default)]
    pub port: u16,
}

/// At most one reply per source address per this window.
pub const SOURCE_MIN_GAP: Duration = Duration::from_secs(1);
/// At most one reply per this window overall (20 per second).
pub const GLOBAL_MIN_GAP: Duration = Duration::from_millis(50);
/// Source entries kept; beyond this, stale entries are pruned so a spoofed
/// source flood cannot grow the map without bound.
pub const MAX_TRACKED_SOURCES: usize = 256;
/// Entries older than this are dropped when the map is pruned.
pub const TRACK_TTL: Duration = Duration::from_secs(300);

/// Feature flag + port, read once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryConfig {
    pub enabled: bool,
    pub port: u16,
}

impl DiscoveryConfig {
    /// Environment form: enabled unless `EBBTOP_AGENT_DISCOVERY` says off;
    /// port from `EBBTOP_DISCOVERY_PORT` (a bad value keeps the default).
    pub fn from_env() -> Self {
        Self::from_env_or_default(true)
    }

    /// Same, with the default when `EBBTOP_AGENT_DISCOVERY` is unset. The
    /// macOS pilot passes `false` so an unauthenticated responder is not on
    /// by default on a laptop; an explicit value still wins on every
    /// platform, and Linux keeps its historical `true`.
    pub fn from_env_or_default(default_enabled: bool) -> Self {
        let enabled = std::env::var("EBBTOP_AGENT_DISCOVERY")
            .ok()
            .map(|v| enabled_from(&v))
            .unwrap_or(default_enabled);
        let port = std::env::var("EBBTOP_DISCOVERY_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .filter(|p| *p != 0)
            .unwrap_or(DISCOVERY_PORT);
        Self { enabled, port }
    }
}

/// True when a datagram source is on the same private network as the agent:
/// loopback, RFC 1918, link-local, CGNAT/Tailscale, or IPv6 unique-local.
///
/// The responder refuses anything else, so it cannot be used as an
/// off-network reflector or as a public "which hosts exist" oracle even if
/// the operator binds a public address by mistake.
pub fn is_private_source(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || (o[0] == 100 && (64..=127).contains(&o[1])) // 100.64/10
                || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_source(IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10
        }
    }
}

/// `0`, `false`, `no`, `off` (any case, surrounding space ignored) disable
/// discovery; an empty or missing value leaves it enabled. Deliberately not
/// "anything unrecognised disables": a typo must not silently drop the
/// feature an operator thinks is on.
pub fn enabled_from(v: &str) -> bool {
    !matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

/// True when a datagram is a discovery probe. Trailing CR/LF is tolerated
/// (a shell `printf '…\n'`), nothing else is.
pub fn is_discovery_request(bytes: &[u8]) -> bool {
    let t = bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(bytes);
    t == DISCOVERY_REQUEST
}

/// Per-source and global token bucket, kept deliberately simple: one reply
/// per `SOURCE_MIN_GAP` per source, one per `GLOBAL_MIN_GAP` overall.
pub struct RateLimit {
    per_source: HashMap<IpAddr, Instant>,
    last_global: Option<Instant>,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimit {
    pub fn new() -> Self {
        Self {
            per_source: HashMap::new(),
            last_global: None,
        }
    }

    /// May this source get a reply at `now`? Records the reply when true, so
    /// a caller does not need a second call to "commit".
    pub fn allow(&mut self, source: IpAddr, now: Instant) -> bool {
        if let Some(last) = self.last_global
            && now.saturating_duration_since(last) < GLOBAL_MIN_GAP
        {
            return false;
        }
        if let Some(last) = self.per_source.get(&source)
            && now.saturating_duration_since(*last) < SOURCE_MIN_GAP
        {
            return false;
        }
        if self.per_source.len() >= MAX_TRACKED_SOURCES {
            self.prune(now);
        }
        self.per_source.insert(source, now);
        self.last_global = Some(now);
        true
    }

    /// Drop expired entries; if the map is still full, drop the oldest half
    /// so a spoofed-source flood cannot lock out real clients forever.
    fn prune(&mut self, now: Instant) {
        self.per_source
            .retain(|_, at| now.saturating_duration_since(*at) < TRACK_TTL);
        if self.per_source.len() >= MAX_TRACKED_SOURCES {
            let mut ages: Vec<(IpAddr, Instant)> =
                self.per_source.iter().map(|(k, v)| (*k, *v)).collect();
            ages.sort_by_key(|(_, at)| *at);
            for (ip, _) in ages.into_iter().take(MAX_TRACKED_SOURCES / 2) {
                self.per_source.remove(&ip);
            }
        }
    }

    pub fn tracked_sources(&self) -> usize {
        self.per_source.len()
    }
}

/// The one JSON object the responder ever sends.
pub fn response_bytes(state: &AppState, ws_port: u16) -> Vec<u8> {
    let r = DiscoveryResponse {
        ebbtop_discovery: DISCOVERY_PROTOCOL,
        name: state.hostname.clone(),
        agent_version: state.agent_version.clone(),
        nonce: state.boot_nonce.clone(),
        port: ws_port,
    };
    // Serialising a fixed struct cannot fail; an empty datagram would still
    // be better than a panic on the responder path.
    serde_json::to_vec(&r).unwrap_or_default()
}

/// Serve probes until the task is aborted. Every error is local to one
/// datagram: a bad packet or a failed reply never stops the responder.
pub async fn serve(socket: UdpSocket, state: AppState, ws_port: u16) {
    let mut buf = [0u8; 512];
    let mut limit = RateLimit::new();
    loop {
        let (n, src) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(_) => continue, // e.g. EMSGSIZE from a jumbo datagram
        };
        if !is_discovery_request(&buf[..n]) {
            continue;
        }
        if !is_private_source(src.ip()) {
            continue;
        }
        if !limit.allow(src.ip(), Instant::now()) {
            continue;
        }
        let reply = response_bytes(&state, ws_port);
        let _ = socket.send_to(&reply, src).await;
    }
}

/// Bind and start the responder, or return `Ok(None)` when it is disabled.
///
/// The socket binds `0.0.0.0:<port>`, deliberately **not** `EBBTOP_BIND`: on
/// Linux a socket bound to one unicast address never receives a broadcast
/// datagram, so binding the agent's address would make discovery unicast-only
/// and defeat the point. What keeps that honest is the reply rule, not the
/// bind address: only exact probes from private/loopback/link-local/CGNAT
/// sources are answered, so a public interface cannot be probed into a
/// response.
///
/// A bind failure is returned to the caller, which logs it and keeps serving
/// the WebSocket: discovery is a convenience, not a reason to fail the agent.
pub async fn spawn(
    config: DiscoveryConfig,
    state: AppState,
    ws_port: u16,
) -> std::io::Result<Option<JoinHandle<()>>> {
    if !config.enabled {
        return Ok(None);
    }
    let socket = UdpSocket::bind(bind_addr(config.port)).await?;
    Ok(Some(tokio::spawn(serve(socket, state, ws_port))))
}

/// The responder's bind address: every IPv4 interface, one fixed port.
pub fn bind_addr(port: u16) -> std::net::SocketAddr {
    std::net::SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// The exact bytes the agent emits, pinned here AND in
    /// `ebbtop_core::pairing::tests::discovery_fixture_is_the_agents_bytes`.
    /// A field rename on either side fails one of the two tests.
    const FIXTURE: &str = r#"{"ebbtop_discovery":1,"name":"k-server","agent_version":"0.2.8","nonce":"00112233445566778899aabbccddeeff","port":45976}"#;

    fn state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::with_pairing_dir(dir.path());
        (dir, st)
    }

    fn ip(s: &str) -> IpAddr {
        IpAddr::V4(s.parse::<Ipv4Addr>().unwrap())
    }

    #[test]
    fn reply_bytes_are_pinned_and_carry_no_token() {
        let (_d, mut st) = state();
        st.hostname = "k-server".into();
        st.agent_version = "0.2.8".into();
        st.boot_nonce = "00112233445566778899aabbccddeeff".into();
        st.auth_token = Some("SENTINEL-TOKEN-MUST-NOT-LEAK".into());
        let bytes = response_bytes(&st, 45976);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert_eq!(text, FIXTURE);
        assert!(!text.contains("SENTINEL"), "reply leaked token material");
        assert!(!text.contains("token"));
        // and it decodes back to the same struct
        let back: DiscoveryResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.ebbtop_discovery, DISCOVERY_PROTOCOL);
        assert_eq!(back.port, 45976);
    }

    #[test]
    fn a_reply_without_port_still_decodes() {
        // A future/older responder without `port` must not break a client.
        let legacy = r#"{"ebbtop_discovery":1,"name":"pi","agent_version":"0.2.7","nonce":"abc"}"#;
        let r: DiscoveryResponse = serde_json::from_str(legacy).unwrap();
        assert_eq!(r.port, 0);
    }

    #[test]
    fn only_the_exact_probe_is_a_request() {
        assert!(is_discovery_request(DISCOVERY_REQUEST));
        assert!(is_discovery_request(b"EBBTOP-DISCOVER/1\n"));
        assert!(is_discovery_request(b"EBBTOP-DISCOVER/1\r\n"));
        assert!(!is_discovery_request(b"EBBTOP-DISCOVER/2"));
        assert!(!is_discovery_request(b"EBBTOP-DISCOVER/1 extra"));
        assert!(!is_discovery_request(b""));
        assert!(!is_discovery_request(b" get_metrics"));
    }

    #[test]
    fn private_sources_are_accepted_and_public_ones_refused() {
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.1.10",
            "169.254.4.4",
            "100.99.193.22",
        ] {
            assert!(is_private_source(ip(s)), "{s}");
        }
        for s in [
            "8.8.8.8",
            "1.1.1.1",
            "172.15.0.1",
            "172.32.0.1",
            "100.63.255.255",
            "100.128.0.0",
        ] {
            assert!(!is_private_source(ip(s)), "{s}");
        }
        let v6: IpAddr = "2001:4860:4860::8888".parse().unwrap();
        assert!(!is_private_source(v6));
        let v6_local: IpAddr = "fd7a:115c:a1e0::1".parse().unwrap();
        assert!(is_private_source(v6_local));
        let mapped: IpAddr = "::ffff:192.168.1.10".parse().unwrap();
        assert!(is_private_source(mapped));
    }

    #[test]
    fn feature_flag_parsing() {
        for off in ["0", "false", "FALSE", " no ", "off", "Off"] {
            assert!(!enabled_from(off), "{off:?} must disable discovery");
        }
        for on in ["1", "true", "yes", "on", "", "maybe"] {
            assert!(enabled_from(on), "{on:?} must leave discovery enabled");
        }
    }

    #[test]
    fn per_source_limit_allows_one_then_blocks_inside_the_window() {
        let mut l = RateLimit::new();
        let t0 = Instant::now();
        assert!(l.allow(ip("192.168.1.10"), t0));
        assert!(!l.allow(ip("192.168.1.10"), t0 + Duration::from_millis(1)));
        assert!(!l.allow(
            ip("192.168.1.10"),
            t0 + SOURCE_MIN_GAP - Duration::from_millis(1)
        ));
        // a different source is independent, once the global gap has passed
        assert!(l.allow(ip("192.168.1.11"), t0 + GLOBAL_MIN_GAP));
    }

    #[test]
    fn source_limit_refills_after_the_window() {
        let mut l = RateLimit::new();
        let t0 = Instant::now();
        assert!(l.allow(ip("10.0.0.5"), t0));
        assert!(l.allow(ip("10.0.0.5"), t0 + SOURCE_MIN_GAP));
        assert!(l.allow(ip("10.0.0.5"), t0 + SOURCE_MIN_GAP * 2));
    }

    #[test]
    fn global_limit_caps_total_replies() {
        let mut l = RateLimit::new();
        let t0 = Instant::now();
        let mut allowed_at = Vec::new();
        // 200 distinct sources, one probe per millisecond: only the ones on a
        // 50 ms boundary may be answered.
        for n in 0..200u32 {
            let at = t0 + Duration::from_millis(u64::from(n));
            if l.allow(ip(&format!("10.1.0.{}", n % 250 + 1)), at) {
                allowed_at.push(n);
            }
        }
        assert_eq!(allowed_at, vec![0, 50, 100, 150], "global cap must be 20/s");
    }

    #[test]
    fn the_source_map_is_bounded() {
        let mut l = RateLimit::new();
        let t0 = Instant::now();
        for n in 0..(MAX_TRACKED_SOURCES as u32 * 2) {
            // spread over time so the per-source and global limits do not
            // interfere; only the map size is under test
            let at = t0 + GLOBAL_MIN_GAP * n * 2;
            l.allow(ip(&format!("10.2.{}.{}", n / 250, n % 250)), at);
        }
        assert!(
            l.tracked_sources() <= MAX_TRACKED_SOURCES,
            "map grew to {}",
            l.tracked_sources()
        );
    }
}
