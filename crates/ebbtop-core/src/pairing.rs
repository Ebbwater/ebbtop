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
        let mut r: DiscoveryResponse =
            serde_json::from_str(r#"{"ebbtop_discovery":2,"name":"x","agent_version":"1","nonce":"n"}"#)
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
        assert!(serde_json::from_str::<PairRequest>(r#"{"code":"123456"}"#)
            .unwrap()
            .label
            .is_none());

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
}
