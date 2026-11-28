//! ebbtop P3a: LAN discovery and pairing-code enrollment wire types.
//!
//! Two additive control-plane surfaces, neither of which touches the
//! WebSocket data path:
//!
//! * **Discovery** is one UDP request/response. A client broadcasts
//!   [`DISCOVERY_REQUEST`] to [`DISCOVERY_PORT`]; every ebbtop agent on the
//!   same broadcast domain answers with one JSON [`DiscoveryResponse`]
//!   (display name, agent version, random per-boot nonce, agent port).
//!   It is unauthenticated by design and answers nothing else: no token, no
//!   commands, no host inventory.
//! * **Pairing** is one HTTP `POST /pair` on the agent's existing port with a
//!   JSON [`PairRequest`] in the *body* (`{"code":"123456"}`) and a JSON
//!   [`PairResponse`] carrying the issued client token. The code is short
//!   lived, single use, and never appears in a query string or a log.
//!
//! Every field these structures read is `#[serde(default)]`-tolerant, so a
//! payload written before a field existed still decodes (see the
//! `discovery_without_port_decodes_legacy` test).

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::client::{AGENT_PORT, ProbeFailure};

/// Exact datagram a client broadcasts to find ebbtop agents. Anything else
/// (including a longer or padded packet) is ignored by the agent.
pub const DISCOVERY_REQUEST: &[u8] = b"EBBTOP-DISCOVER/1";

/// UDP port the discovery responder listens on, next to the agent's TCP
/// port 45976. Overridable on the agent with `EBBTOP_DISCOVERY_PORT`.
pub const DISCOVERY_PORT: u16 = 45978;

/// Value of [`DiscoveryResponse::ebbtop_discovery`]. A response with a
/// different number is a different protocol and must be ignored.
pub const DISCOVERY_PROTOCOL: u32 = 1;

/// Path of the enrollment endpoint on the agent's HTTP listener.
pub const PAIR_PATH: &str = "/pair";

/// Discovery reply. The four required fields are the whole unauthenticated
/// surface: what the host calls itself, which agent build answers, and a
/// nonce that changes when the agent restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryResponse {
    /// Protocol marker; must equal [`DISCOVERY_PROTOCOL`].
    pub ebbtop_discovery: u32,
    /// Host display name (the agent's hostname; the same value the metrics
    /// frame reports as `hostname`).
    pub name: String,
    /// Agent build version, e.g. `0.2.8`.
    pub agent_version: String,
    /// Random hex value minted once per agent process. A client can use a
    /// changed nonce to tell "the agent restarted" apart from "the network
    /// blipped".
    pub nonce: String,
    /// The agent's WebSocket TCP port, so a discovered host is immediately
    /// usable. Absent (0) from a responder that predates the field; the
    /// client then falls back to [`crate::client::AGENT_PORT`].
    #[serde(default)]
    pub port: u16,
}

impl DiscoveryResponse {
    /// True when this is an ebbtop discovery reply at the protocol version
    /// this client understands.
    pub fn is_current(&self) -> bool {
        self.ebbtop_discovery == DISCOVERY_PROTOCOL
    }
}

/// Body of `POST /pair`. The code travels in the body, never the query
/// string, so it does not land in an access log or a browser history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairRequest {
    /// The short-lived numeric code printed by `ebbtop-agent --pair`.
    pub code: String,
    /// Optional human label for the issued credential ("Kyler's laptop").
    #[serde(default)]
    pub label: Option<String>,
}

/// Successful `POST /pair` reply. The token is the only secret in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairResponse {
    /// Credential for the client's credential store, 32 random bytes hex.
    pub token: String,
    /// Host display name, for the client's source list.
    pub host: String,
    /// Agent build that issued the token.
    pub agent_version: String,
    /// The agent's per-boot nonce, so the client can spot a restart.
    #[serde(default)]
    pub nonce: Option<String>,
    /// What the token is allowed to do. `read_only` is the only scope this
    /// agent issues; an empty value means an older/unknown agent.
    #[serde(default)]
    pub scope: String,
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
                || v4.is_private() // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local() // 169.254/16
                || (o[0] == 100 && (64..=127).contains(&o[1])) // 100.64/10 (CGNAT, Tailscale)
                || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_source(IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                // fc00::/7 unique-local
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // fe80::/10 link-local
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Digits in a pairing code. The agent mints exactly six; anything else
/// never reaches the network, so a typo cannot spend one of the five
/// attempts the host allows before it cancels the code.
pub const PAIR_CODE_DIGITS: usize = 6;

/// Largest datagram a discovery reply may be. The responder's reply is a
/// small fixed struct, so anything bigger is not that reply.
pub const DISCOVERY_REPLY_MAX: usize = 1024;

/// Scan window: long enough for two spaced probes (the agent answers one
/// reply per source per second) and short enough to feel instant.
pub const SCAN_WINDOW_MS: u64 = 1400;

/// Probe datagrams one scan sends.
pub const SCAN_PROBES: usize = 3;

/// Spacing between probe datagrams, above the responder's per-source limit.
pub const SCAN_PROBE_GAP_MS: u64 = 450;

/// Most agents one scan reports. A scan is bounded in every dimension.
pub const SCAN_MAX_HOSTS: usize = 24;

/// A pairing code as the user typed it: surrounding and internal whitespace
/// dropped (a paste can carry a newline), then exactly six ASCII digits.
pub fn clean_pair_code(raw: &str) -> Result<String, String> {
    let code: String = raw.split_whitespace().collect();
    if code.is_empty() {
        return Err(format!(
            "enter the {PAIR_CODE_DIGITS}-digit code the host prints for `ebbtop-agent --pair`"
        ));
    }
    if code.len() == PAIR_CODE_DIGITS && code.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(code);
    }
    Err(format!(
        "the pairing code is exactly {PAIR_CODE_DIGITS} digits (numbers only; the host prints it once)"
    ))
}

/// The `/pair` endpoint for a stored source URL: same host and port, plain
/// HTTP(S) instead of ws(s), path `/pair`, no query. The code goes in the
/// body, never a query string, so it cannot land in an access log.
pub fn pair_url(base: &str) -> Result<String, String> {
    let normalized = crate::client::normalize_base_url(base).map_err(|e| e.to_string())?;
    let mut u = url::Url::parse(&normalized).map_err(|e| e.to_string())?;
    match u.scheme() {
        "ws" => u
            .set_scheme("http")
            .map_err(|_| "cannot map ws:// to http://".to_string())?,
        "wss" => u
            .set_scheme("https")
            .map_err(|_| "cannot map wss:// to https://".to_string())?,
        other => {
            return Err(format!(
                "pairing needs a ws:// or wss:// address, not {other:?}"
            ));
        }
    }
    u.set_path(PAIR_PATH);
    u.set_query(None);
    u.set_fragment(None);
    Ok(u.to_string())
}

/// Why a pairing exchange failed, as far as the client can tell. Agents and
/// client live in different repos but one release, so the classes come from
/// the accepted `/pair` contract: 404 no pending code, 403 invalid, 403
/// expired, 429 locked out, 400 malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairFailure {
    /// No code is waiting: none was minted, or it was already used.
    NoPendingCode,
    /// A code is waiting and this is not it. A typo does not burn it: the
    /// host counts wrong guesses and cancels after five.
    InvalidCode,
    /// The code was minted more than the TTL (about five minutes) ago.
    ExpiredCode,
    /// Five wrong guesses invalidated the code; the host now answers 404.
    LockedOut,
    /// The host answered, but not to a `/pair` request it understood.
    BadRequest,
    /// Nothing is reachable at that address.
    Unreachable,
    /// Nothing came back inside the deadline.
    Timeout,
    /// Something answered, but not the EbbTop agent.
    NotAgent,
    /// Any other failure: the detail is all the evidence there is.
    Ambiguous,
}

impl PairFailure {
    /// Stable tag for log lines and tests. Never carries a code or token.
    pub fn tag(self) -> &'static str {
        match self {
            Self::NoPendingCode => "no-pending-code",
            Self::InvalidCode => "invalid-code",
            Self::ExpiredCode => "expired-code",
            Self::LockedOut => "locked-out",
            Self::BadRequest => "bad-request",
            Self::Unreachable => "unreachable",
            Self::Timeout => "timeout",
            Self::NotAgent => "not-agent",
            Self::Ambiguous => "ambiguous",
        }
    }

    /// Short label for the UI: what happened, in the user's terms.
    pub fn headline(self) -> &'static str {
        match self {
            Self::NoPendingCode => "No pairing code is waiting on that host",
            Self::InvalidCode => "Pairing code not accepted",
            Self::ExpiredCode => "Pairing code expired",
            Self::LockedOut => "Too many wrong codes",
            Self::BadRequest => "The host did not understand the pairing request",
            Self::Unreachable => "No route to the host",
            Self::Timeout => "The host did not answer in time",
            Self::NotAgent => "Not the EbbTop agent",
            Self::Ambiguous => "Pairing failed",
        }
    }

    /// One concrete next action, distinct per class: a user is never told to
    /// "check everything" for a failure that already has an answer.
    pub fn next_action(self) -> &'static str {
        match self {
            Self::NoPendingCode => {
                "Run `ebbtop-agent --pair` on the agent host to mint a code, then enter it here \
                 within about five minutes. A code is single use: if you already paired with it, \
                 this is expected and no action is needed beyond pairing again only if you need \
                 another client."
            }
            Self::InvalidCode => {
                "Re-type the six digits exactly as the agent host printed them. Five wrong \
                 guesses cancel the code: if you have already tried several, run \
                 `ebbtop-agent --pair` on the host for a new one."
            }
            Self::ExpiredCode => {
                "Codes last about five minutes. Run `ebbtop-agent --pair` on the agent host for a \
                 fresh code and enter it straight away."
            }
            Self::LockedOut => {
                "The host cancelled that code after five wrong guesses (the deliberate brute-force \
                 limit). Run `ebbtop-agent --pair` on the agent host for a new code."
            }
            Self::BadRequest => {
                "The host answered, but refused the request shape. Confirm the address is the \
                 EbbTop agent (default port 45976) and that its version supports pairing (0.2.9 or \
                 newer); an older agent has no `/pair`, so paste its token instead."
            }
            Self::Unreachable => {
                "Nothing is reachable at that address. Check the host, that this computer is on \
                 the same private network or VPN as the agent, the port (default 45976), and that \
                 the agent is running (`systemctl --user is-active ebbtop-agent` on the host)."
            }
            Self::Timeout => {
                "The host did not answer in time. A firewall, a VPN that is down and a stopped \
                 agent all look like this. Confirm the host and port, check \
                 `systemctl --user is-active ebbtop-agent` on the host, then pair again."
            }
            Self::NotAgent => {
                "Something answered, but not the EbbTop agent. Check the port (default 45976) and \
                 that nothing else is listening there; a bare hostname gets `:45976` added."
            }
            Self::Ambiguous => {
                "The exchange failed without a clear cause. Check the host, the port (default \
                 45976) and the agent status (`systemctl --user is-active ebbtop-agent` on the \
                 host), then try again — or paste the agent token instead."
            }
        }
    }

    /// Classify one `/pair` refusal by HTTP status and body text. The agent's
    /// bodies are fixed strings; the status carries the class even when a
    /// proxy rewrites the body.
    pub fn classify_status(status: u16, body: &str) -> Self {
        let b = body.to_ascii_lowercase();
        match status {
            404 => Self::NoPendingCode,
            403 if b.contains("expired") => Self::ExpiredCode,
            403 => Self::InvalidCode,
            429 => Self::LockedOut,
            400 => Self::BadRequest,
            _ => Self::Ambiguous,
        }
    }

    /// Classify a transport error with the same evidence rules the ws probe
    /// uses (E4): refused/DNS/no-route never reached the host, a deadline
    /// miss proves nothing, a handshake error means something else answered.
    pub fn classify_transport(err: &str) -> Self {
        match ProbeFailure::classify(err) {
            ProbeFailure::Unreachable => Self::Unreachable,
            ProbeFailure::Timeout => Self::Timeout,
            ProbeFailure::NotAgent => Self::NotAgent,
            ProbeFailure::AuthRefused | ProbeFailure::Ambiguous => Self::Ambiguous,
        }
    }
}

/// Drop any `code=<value>` pair from text that may leave the app. The
/// pairing code is short-lived but it is still the only secret in that
/// exchange, and a proxy (or a future agent) can echo a query back. Same
/// rules as [`crate::client::scrub`], for the other secret name.
pub fn scrub_pair_detail(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("code=") {
        out.push_str(&rest[..i]);
        out.push_str("code=<code>");
        let after = &rest[i + "code=".len()..];
        let end = after
            .find(|c: char| c == '&' || c == '#' || c == '"' || c == '\'' || c.is_whitespace())
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    crate::client::scrub(&out)
}

/// A classified pairing failure plus the detail it was derived from. The
/// detail is scrubbed, so a caller may show, log or copy it: no code or token
/// is ever part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairError {
    pub kind: PairFailure,
    pub detail: String,
}

impl PairError {
    /// From a non-success `/pair` response. The body is the agent's own
    /// refusal text (never the submitted code); scrubbing is a belt-and-braces
    /// pass in case a proxy or a future agent echoes a query.
    pub fn from_status(status: u16, body: &str) -> Self {
        let detail = scrub_pair_detail(&format!("HTTP {status}: {}", body.trim()));
        Self {
            kind: PairFailure::classify_status(status, &detail),
            detail,
        }
    }

    /// From a transport error string (never contains the code: the code
    /// travels in the body, not the URL).
    pub fn from_transport(err: &str) -> Self {
        let detail = scrub_pair_detail(err);
        Self {
            kind: PairFailure::classify_transport(&detail),
            detail,
        }
    }

    /// From a 2xx body that is not a usable [`PairResponse`].
    pub fn bad_reply(what: &str) -> Self {
        Self {
            kind: PairFailure::Ambiguous,
            detail: scrub_pair_detail(what),
        }
    }

    pub fn headline(&self) -> &'static str {
        self.kind.headline()
    }

    pub fn next_action(&self) -> &'static str {
        self.kind.next_action()
    }

    /// One log line. Tag + scrubbed detail only: safe to append verbatim.
    pub fn log_line(&self) -> String {
        format!("pair {}: {}", self.kind.tag(), self.detail)
    }
}

/// Parse a 2xx `/pair` body. An empty token is a broken reply, not a
/// credential: the caller must never store one.
pub fn pair_response(body: &str) -> Result<PairResponse, PairError> {
    let r: PairResponse = serde_json::from_str(body).map_err(|e| {
        PairError::bad_reply(&format!("the host's pairing reply did not parse: {e}"))
    })?;
    if r.token.trim().is_empty() {
        return Err(PairError::bad_reply(
            "the host's pairing reply carried no token (empty)",
        ));
    }
    Ok(r)
}

/// Decode one datagram as a discovery reply. Anything that is not this
/// protocol, at this version, from a named agent is ignored: a client must
/// never list a foreign host.
pub fn decode_discovery_reply(bytes: &[u8]) -> Option<DiscoveryResponse> {
    if bytes.is_empty() || bytes.len() > DISCOVERY_REPLY_MAX {
        return None;
    }
    let r: DiscoveryResponse = serde_json::from_slice(bytes).ok()?;
    if !r.is_current() || r.name.trim().is_empty() {
        return None;
    }
    Some(r)
}

/// One agent a scan found: where it answered from, the port it serves, and
/// the identity the reply carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredAgent {
    /// Source address of the reply, as the client saw it.
    pub addr: String,
    /// The agent's ws port (falls back to [`AGENT_PORT`] for a responder
    /// that predates the field).
    pub port: u16,
    pub name: String,
    pub agent_version: String,
    /// Per-boot nonce: a changed value means the agent restarted.
    pub nonce: String,
}

impl DiscoveredAgent {
    /// Build from a reply and the datagram source address.
    pub fn from_reply(src: IpAddr, r: &DiscoveryResponse) -> Self {
        Self {
            addr: src.to_string(),
            port: if r.port == 0 { AGENT_PORT } else { r.port },
            name: r.name.trim().to_string(),
            agent_version: r.agent_version.trim().to_string(),
            nonce: r.nonce.clone(),
        }
    }

    /// Address for the URL field: IPv6 literals are bracketed.
    pub fn host(&self) -> String {
        if self.addr.contains(':') {
            format!("[{}]", self.addr)
        } else {
            self.addr.clone()
        }
    }

    /// The source URL this host prefills: `ws://addr:port/ws`.
    pub fn url(&self) -> String {
        crate::client::normalize_base_url(&format!("{}:{}", self.host(), self.port))
            .unwrap_or_else(|_| format!("ws://{}:{}/ws", self.host(), self.port))
    }
}

/// A source name that does not collide: the agent's own name when it is
/// free, else `<name>-2`, `<name>-3`, … A duplicate hostname on a second
/// address is common (two boxes named `server`), and a silent overwrite
/// would repoint a source the user already trusts.
pub fn unique_source_name(base: &str, taken: &[String]) -> String {
    let base = base.trim();
    let base = if base.is_empty() { "host" } else { base };
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}-{n}");
        if !taken.iter().any(|t| t == &candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse::<Ipv4Addr>().unwrap())
    }
    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse::<Ipv6Addr>().unwrap())
    }

    #[test]
    fn private_sources_are_accepted() {
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "172.31.255.254",
            "192.168.1.10",
            "169.254.4.4",
            "100.64.0.1",
            "100.99.193.22", // the agent's tailnet address shape
            "100.127.255.255",
            "::1",
            "fd7a:115c:a1e0::1", // tailscale ULA
            "fe80::1",
            "::ffff:192.168.1.10",
        ] {
            let ip = if s.contains(':') { v6(s) } else { v4(s) };
            assert!(is_private_source(ip), "{s} should be private");
        }
    }

    #[test]
    fn public_and_borderline_sources_are_refused() {
        for s in [
            "8.8.8.8",
            "1.1.1.1",
            "172.15.0.1",  // just below 172.16/12
            "172.32.0.1",  // just above 172.16/12
            "192.167.1.1", // just below 192.168/16
            "100.63.255.255",
            "100.128.0.0",
            "2001:4860:4860::8888",
        ] {
            let ip = if s.contains(':') { v6(s) } else { v4(s) };
            assert!(!is_private_source(ip), "{s} must not be treated as private");
        }
    }

    #[test]
    fn discovery_roundtrips() {
        let r = DiscoveryResponse {
            ebbtop_discovery: DISCOVERY_PROTOCOL,
            name: "k-server".into(),
            agent_version: "0.2.8".into(),
            nonce: "00112233445566778899aabbccddeeff".into(),
            port: 45976,
        };
        let bytes = serde_json::to_vec(&r).unwrap();
        let back: DiscoveryResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(r, back);
        assert!(back.is_current());
    }

    /// A frame written before `port` existed still decodes (the field
    /// defaults), so a newer client reads an older responder.
    #[test]
    fn discovery_without_port_decodes_legacy() {
        let json = r#"{"ebbtop_discovery":1,"name":"pi","agent_version":"0.2.7","nonce":"abc"}"#;
        let r: DiscoveryResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.port, 0);
        assert_eq!(r.name, "pi");
        assert!(r.is_current());
    }

    /// A stray JSON datagram from something that is not an ebbtop agent does
    /// not decode, so a client never reports a foreign host.
    #[test]
    fn foreign_json_is_not_a_discovery_reply() {
        assert!(serde_json::from_str::<DiscoveryResponse>(r#"{"hello":"world"}"#).is_err());
        let mut r: DiscoveryResponse = serde_json::from_str(
            r#"{"ebbtop_discovery":2,"name":"x","agent_version":"1","nonce":"n"}"#,
        )
        .unwrap();
        assert!(!r.is_current());
        r.ebbtop_discovery = DISCOVERY_PROTOCOL;
        assert!(r.is_current());
    }

    #[test]
    fn pair_request_and_response_decode() {
        let req: PairRequest =
            serde_json::from_str(r#"{"code":"123456","label":"laptop"}"#).unwrap();
        assert_eq!(req.code, "123456");
        assert_eq!(req.label.as_deref(), Some("laptop"));
        // label is optional
        assert!(
            serde_json::from_str::<PairRequest>(r#"{"code":"123456"}"#)
                .unwrap()
                .label
                .is_none()
        );

        let resp: PairResponse = serde_json::from_str(
            r#"{"token":"aa","host":"k-server","agent_version":"0.2.8","nonce":"n","scope":"read_only"}"#,
        )
        .unwrap();
        assert_eq!(resp.scope, "read_only");
        assert_eq!(resp.nonce.as_deref(), Some("n"));
        // nonce and scope are optional for forward/backward tolerance
        let legacy: PairResponse =
            serde_json::from_str(r#"{"token":"aa","host":"k-server","agent_version":"0.2.8"}"#)
                .unwrap();
        assert!(legacy.nonce.is_none());
        assert_eq!(legacy.scope, "");
    }

    /// The exact bytes `ebbtop-agent`'s discovery responder emits. The same
    /// literal is pinned in `ebbtop_agent::discovery::tests::FIXTURE`; a field
    /// rename on either side fails one of the two tests.
    const DISCOVERY_FIXTURE: &str = r#"{"ebbtop_discovery":1,"name":"k-server","agent_version":"0.2.8","nonce":"00112233445566778899aabbccddeeff","port":45976}"#;

    #[test]
    fn discovery_fixture_is_the_agents_bytes() {
        let r: DiscoveryResponse = serde_json::from_str(DISCOVERY_FIXTURE).unwrap();
        assert_eq!(r.ebbtop_discovery, DISCOVERY_PROTOCOL);
        assert_eq!(r.name, "k-server");
        assert_eq!(r.agent_version, "0.2.8");
        assert_eq!(r.nonce, "00112233445566778899aabbccddeeff");
        assert_eq!(r.port, 45976);
        // re-serialising reproduces the agent's bytes exactly
        assert_eq!(serde_json::to_string(&r).unwrap(), DISCOVERY_FIXTURE);
    }

    /// The exact bytes `ebbtop-agent`'s `/pair` handler returns, pinned in
    /// `ebbtop_agent::pairing::tests::PAIR_RESPONSE_FIXTURE` too.
    const PAIR_RESPONSE_FIXTURE: &str = r#"{"token":"placeholder-not-a-credential","host":"k-server","agent_version":"0.2.8","nonce":"00112233445566778899aabbccddeeff","scope":"read_only"}"#;

    #[test]
    fn pair_response_fixture_is_the_agents_bytes() {
        let r: PairResponse = serde_json::from_str(PAIR_RESPONSE_FIXTURE).unwrap();
        assert_eq!(r.token, "placeholder-not-a-credential");
        assert_eq!(r.host, "k-server");
        assert_eq!(r.scope, "read_only");
        assert!(r.nonce.is_some());
        assert_eq!(serde_json::to_string(&r).unwrap(), PAIR_RESPONSE_FIXTURE);
    }

    #[test]
    fn discovery_request_is_exact_and_versioned() {
        assert_eq!(DISCOVERY_REQUEST, b"EBBTOP-DISCOVER/1");
        assert!(DISCOVERY_REQUEST.len() < 64, "must fit one small datagram");
    }

    // ---- P3b: the client half of the same contract -----------------------

    #[test]
    fn pairing_code_is_six_digits_and_nothing_else() {
        assert_eq!(clean_pair_code("123456").as_deref(), Ok("123456"));
        // A paste can carry a newline or spaces; the digits are what counts.
        assert_eq!(clean_pair_code(" 12 34 56\n").as_deref(), Ok("123456"));
        for bad in ["", "   ", "12345", "1234567", "12a456", "12-456", "١٢٣٤٥٦"] {
            assert!(clean_pair_code(bad).is_err(), "{bad:?} must not be a code");
        }
        // The message names the shape, so a user can fix it without a manual.
        let e = clean_pair_code("12345").expect_err("short code");
        assert!(e.contains("6 digits"), "unhelpful: {e}");
    }

    #[test]
    fn pair_url_is_the_same_host_on_plain_http_with_no_query() {
        assert_eq!(
            pair_url("ws://10.0.0.5:45976/ws").as_deref(),
            Ok("http://10.0.0.5:45976/pair")
        );
        // Bare host, custom port and a token-bearing URL all reduce to the
        // endpoint: the code never rides a query string.
        assert_eq!(
            pair_url("k-server").as_deref(),
            Ok("http://k-server:45976/pair")
        );
        assert_eq!(
            pair_url("ws://h:9000/custom").as_deref(),
            Ok("http://h:9000/pair")
        );
        assert_eq!(
            pair_url("ws://h:45976/ws?token=leak").as_deref(),
            Ok("http://h:45976/pair")
        );
        assert_eq!(
            pair_url("wss://h/ws").as_deref(),
            Ok("https://h:45976/pair")
        );
        assert!(pair_url("http://h/ws").is_err(), "scheme must be ws/wss");
    }

    #[test]
    fn each_refusal_status_maps_to_its_own_class() {
        assert_eq!(
            PairFailure::classify_status(404, "no pending pairing code"),
            PairFailure::NoPendingCode
        );
        assert_eq!(
            PairFailure::classify_status(403, "invalid pairing code"),
            PairFailure::InvalidCode
        );
        assert_eq!(
            PairFailure::classify_status(403, "pairing code expired"),
            PairFailure::ExpiredCode
        );
        assert_eq!(
            PairFailure::classify_status(429, "too many attempts"),
            PairFailure::LockedOut
        );
        assert_eq!(
            PairFailure::classify_status(400, "bad pair request"),
            PairFailure::BadRequest
        );
        // An unknown status is not guessed into a definite class.
        assert_eq!(
            PairFailure::classify_status(500, "boom"),
            PairFailure::Ambiguous
        );
        assert_eq!(
            PairFailure::classify_status(200, ""),
            PairFailure::Ambiguous
        );
    }

    #[test]
    fn transport_failures_reuse_the_e4_evidence_rules() {
        assert_eq!(
            PairFailure::classify_transport(
                "error sending request: Connection refused (os error 111)"
            ),
            PairFailure::Unreachable
        );
        assert_eq!(
            PairFailure::classify_transport("error sending request: operation timed out"),
            PairFailure::Timeout
        );
        assert_eq!(
            PairFailure::classify_transport("HTTP format error: invalid status"),
            PairFailure::NotAgent
        );
        assert_eq!(
            PairFailure::classify_transport("something new"),
            PairFailure::Ambiguous
        );
    }

    #[test]
    fn pair_error_detail_carries_no_code_and_every_class_is_actionable() {
        // The failure paths must never echo the submitted code back.
        const CODE: &str = "424242";
        let e = PairError::from_status(403, "invalid pairing code");
        assert!(
            !e.log_line().contains(CODE),
            "code in the log line: {}",
            e.log_line()
        );
        assert_eq!(e.kind, PairFailure::InvalidCode);

        // A future agent that echoes a query cannot leak one either.
        let e = PairError::from_status(400, "bad request ?code=424242");
        assert!(
            !e.detail.contains(CODE),
            "code survived scrub: {}",
            e.detail
        );

        // Every class says what happened and what to do, and no two classes
        // share a headline (a user must be able to tell them apart).
        let all = [
            PairFailure::NoPendingCode,
            PairFailure::InvalidCode,
            PairFailure::ExpiredCode,
            PairFailure::LockedOut,
            PairFailure::BadRequest,
            PairFailure::Unreachable,
            PairFailure::Timeout,
            PairFailure::NotAgent,
            PairFailure::Ambiguous,
        ];
        let mut headlines = std::collections::HashSet::new();
        let mut tags = std::collections::HashSet::new();
        for k in all {
            assert!(!k.headline().is_empty());
            assert!(
                k.next_action().len() > 40,
                "{} has no concrete action",
                k.tag()
            );
            assert!(
                headlines.insert(k.headline()),
                "duplicate headline: {}",
                k.headline()
            );
            assert!(tags.insert(k.tag()), "duplicate tag: {}", k.tag());
        }
        // The code-minting classes all point at the one action that fixes
        // them, and the network classes do not.
        for k in [
            PairFailure::NoPendingCode,
            PairFailure::ExpiredCode,
            PairFailure::LockedOut,
        ] {
            assert!(
                k.next_action().contains("--pair"),
                "{} must say how to mint a code",
                k.tag()
            );
        }
        assert!(!PairFailure::Unreachable.next_action().contains("--pair"));
    }

    #[test]
    fn pair_reply_must_carry_a_token() {
        let ok: PairResponse =
            pair_response(PAIR_RESPONSE_FIXTURE).expect("the agent's own reply must parse");
        assert_eq!(ok.scope, "read_only");
        // An empty token is not a credential: refuse it rather than store it.
        let e = pair_response(r#"{"token":"","host":"h","agent_version":"0.2.8"}"#)
            .expect_err("empty token must be refused");
        assert_eq!(e.kind, PairFailure::Ambiguous);
        let e = pair_response("<html>proxy</html>").expect_err("non-JSON must be refused");
        assert_eq!(e.kind, PairFailure::Ambiguous);
        assert!(
            !e.detail.contains("token=a"),
            "detail not scrubbed: {}",
            e.detail
        );
    }

    #[test]
    fn discovery_decode_ignores_foreign_and_oversize_replies() {
        let r = decode_discovery_reply(DISCOVERY_FIXTURE.as_bytes()).expect("agent fixture");
        assert_eq!(r.name, "k-server");
        assert_eq!(r.port, 45976);
        // Not our protocol, no name, empty, oversized: never listed.
        assert!(decode_discovery_reply(br#"{"hello":"world"}"#).is_none());
        assert!(
            decode_discovery_reply(
                br#"{"ebbtop_discovery":2,"name":"x","agent_version":"1","nonce":"n"}"#
            )
            .is_none()
        );
        assert!(
            decode_discovery_reply(
                br#"{"ebbtop_discovery":1,"name":"  ","agent_version":"1","nonce":"n"}"#
            )
            .is_none()
        );
        assert!(decode_discovery_reply(b"").is_none());
        let big = format!(
            r#"{{"ebbtop_discovery":1,"name":"x","agent_version":"1","nonce":"{}"}}"#,
            "a".repeat(DISCOVERY_REPLY_MAX)
        );
        assert!(decode_discovery_reply(big.as_bytes()).is_none());
        // A datagram from something that is not an agent never becomes a row.
        assert!(serde_json::from_str::<DiscoveryResponse>(r#"{"name":"x"}"#).is_err());
    }

    #[test]
    fn discovered_agent_prefills_the_source_url_and_row_label() {
        let reply = DiscoveryResponse {
            ebbtop_discovery: DISCOVERY_PROTOCOL,
            name: "k-server".into(),
            agent_version: "0.2.8".into(),
            nonce: "00ff".into(),
            port: 45977,
        };
        let a = DiscoveredAgent::from_reply(v4("10.0.0.5"), &reply);
        assert_eq!(a.url(), "ws://10.0.0.5:45977/ws");
        // The three facts a picker needs are all on the row.
        assert_eq!(
            (a.name.as_str(), a.agent_version.as_str(), a.host(), a.port),
            ("k-server", "0.2.8", "10.0.0.5".to_string(), 45977)
        );

        // A responder that predates `port` still preflights to the default.
        let legacy: DiscoveryResponse = serde_json::from_str(
            r#"{"ebbtop_discovery":1,"name":"pi","agent_version":"0.2.7","nonce":"n"}"#,
        )
        .unwrap();
        let a = DiscoveredAgent::from_reply(v4("192.168.1.22"), &legacy);
        assert_eq!(a.port, crate::client::AGENT_PORT);
        assert_eq!(a.url(), "ws://192.168.1.22:45976/ws");

        // IPv6 replies are bracketed, never pasted raw into a URL.
        let a = DiscoveredAgent::from_reply(v6("fd7a:115c:a1e0::1"), &reply);
        assert_eq!(a.host(), "[fd7a:115c:a1e0::1]");
        assert_eq!(a.url(), "ws://[fd7a:115c:a1e0::1]:45977/ws");
        assert!(crate::client::normalize_base_url(&a.url()).is_ok());
    }

    #[test]
    fn a_second_host_with_the_same_name_gets_a_suffix() {
        let taken = vec!["k-server".to_string(), "k-server-2".to_string()];
        assert_eq!(unique_source_name("lab", &taken), "lab");
        assert_eq!(unique_source_name("k-server", &taken), "k-server-3");
        assert_eq!(unique_source_name("  k-server ", &[]), "k-server");
        assert_eq!(unique_source_name("", &[]), "host");
    }
}
