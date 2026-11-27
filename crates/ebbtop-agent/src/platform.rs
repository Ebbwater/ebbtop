//! ebbtop: platform policy and portable metric helpers.
//!
//! Everything here is a pure function of its arguments so the rules the
//! shipped Apple Silicon agent enforces can be exercised by tests on any
//! host. Two kinds of rule live here:
//!
//! * **Startup policy** ([`startup_policy`]) — the Mac agent must fail closed:
//!   a nonempty token and a loopback-or-private bind, with broadcast
//!   discovery off unless the operator turns it on. Linux keeps its existing
//!   behaviour exactly (no policy is applied there), so the OpenGate legacy
//!   path the Linux agent has always had is untouched.
//! * **Portable readings** ([`cpu_model_from_brand`], [`keep_macos_volume`]) —
//!   the values the Mac agent can report honestly, and the ones it must
//!   report as unavailable rather than guess.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// Errors from [`startup_policy`] are operator-facing: one line, no secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyError(pub String);

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PolicyError {}

/// Refuse a bind address that is not loopback or private.
///
/// Private means the ranges a tailnet, LAN or a personal hotspot uses:
/// RFC 1918 v4, CGNAT 100.64/10 (Tailscale), link-local, IPv6 unique-local
/// and IPv6 link-local. A public address, the wildcard address, or any
/// unspecified address is refused: an agent on a laptop must never answer on
/// an untrusted network, and `0.0.0.0` would put it on every interface the
/// machine has, including the coffee-shop Wi-Fi.
pub fn bind_is_local(bind: IpAddr) -> bool {
    match bind {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || (o[0] == 100 && (64..=127).contains(&o[1])) // 100.64/10 CGNAT/Tailscale
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return bind_is_local(IpAddr::V4(v4));
            }
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10
        }
    }
}

/// Liveness of the authentication gate, as the agent will actually run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatePolicy {
    /// Apply the fail-closed rules (the Mac agent).
    Enforce,
    /// Leave the historical behaviour alone (the Linux agent).
    Permissive,
}

/// Startup checks. `Err` is a refusal: the agent must not serve.
///
/// In [`GatePolicy::Enforce`]:
/// * the token must be present and nonempty — the agent's
///   [`crate::state::AppState::authenticate`] treats "no token configured"
///   as "no authentication required", which is safe for a Linux box the
///   operator reached over a tailnet and unacceptable for a laptop;
/// * the bind address must be loopback or private ([`bind_is_local`]).
///
/// In [`GatePolicy::Permissive`] nothing is checked: this is the Linux agent
/// the existing installers, unit file and documentation describe.
pub fn startup_policy(
    policy: GatePolicy,
    bind: IpAddr,
    token: Option<&str>,
) -> Result<(), PolicyError> {
    if policy == GatePolicy::Permissive {
        return Ok(());
    }
    match token {
        Some(t) if !t.trim().is_empty() => {}
        _ => {
            return Err(PolicyError(
                "refusing to start without a token: set a nonempty EBBTOP_TOKEN in the agent's \
                 agent.env (the Mac agent never serves unauthenticated)"
                    .into(),
            ));
        }
    }
    if !bind_is_local(bind) {
        return Err(PolicyError(format!(
            "refusing bind address {bind}: it is not loopback or a private/tailnet address, so the \
             agent would answer on an untrusted network. Bind 127.0.0.1, a 10/8, 172.16/12 or \
             192.168/16 address, or the Mac's Tailscale 100.64/10 address."
        )));
    }
    Ok(())
}

/// Discovery default for a [`GatePolicy`]: the pilot ships the Mac agent with
/// broadcast discovery off, because an unauthenticated UDP responder that
/// announces "an ebbtop agent is here" is the exact surface the fail-closed
/// policy exists to avoid. An explicit `EBBTOP_AGENT_DISCOVERY` value always
/// wins, on both platforms.
pub fn discovery_default(policy: GatePolicy) -> bool {
    matches!(policy, GatePolicy::Permissive)
}

/// Which policy a build applies, given its target and the
/// `EBBTOP_AGENT_REQUIRE_AUTH` value.
///
/// A Darwin build is *always* [`GatePolicy::Enforce`]; the variable cannot
/// relax it, whatever it says. Only a non-Darwin build may opt in with a
/// truthy value, which is how the Linux harness exercises the enforced gate.
/// This is a pure function so the Darwin rule is testable on Linux — the
/// whole point is that a Linux run must be able to prove the Mac rule, and a
/// Linux fixture must never be able to stand in for it.
pub fn resolve_gate(is_darwin: bool, require_auth: Option<&str>) -> GatePolicy {
    if is_darwin {
        return GatePolicy::Enforce;
    }
    match require_auth
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "1" | "true" | "yes" | "on" => GatePolicy::Enforce,
        _ => GatePolicy::Permissive,
    }
}

/// CPU model from a sysinfo CPU brand string.
///
/// Returns `None` for an empty or whitespace-only brand so a client can tell
/// "this agent reported no model" from a real one; never a placeholder. On
/// macOS the brand is the chip name ("Apple M3 Pro"); on Linux the same value
/// comes from `/proc/cpuinfo` and this helper is not used.
pub fn cpu_model_from_brand(brand: &str) -> Option<String> {
    let t = brand.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Whether this platform reports a CPU temperature at all.
///
/// macOS reports `cpu_temp_c` as *unavailable* by contract
/// (`docs/platform-macos.md`: "unavailable — `null`, not borrowed from another
/// sensor"), because there is no portable CPU sensor: sysinfo's macOS
/// component scan reaches IOHID, where Apple Silicon sensor Product names such
/// as `PMU tdie*` match the generic label filter and would silently turn a
/// die-temperature reading into a documented-null field. Reporting nothing is
/// the honest answer, and it keeps the published docs true.
///
/// A pure function so the Darwin rule is testable on Linux, exactly like
/// [`resolve_gate`]: a Linux run must be able to prove the Mac rule without a
/// Mac, and `cfg!(target_os = "macos")` is the only input that matters.
pub fn cpu_temp_reported(is_macos: bool) -> bool {
    !is_macos
}

/// Default directory for the agent's writable state (the pending pairing code
/// and the issued client tokens) when `EBBTOP_STATE_DIR` is unset.
///
/// The two platforms keep their state in different places because their
/// installers already do, and the agent must agree with the installer that
/// created the directory and that `--uninstall --purge` deletes:
///
/// * macOS: `$HOME/Library/Application Support/ebbtop-agent` — what
///   `deploy/macos/install-agent.sh` creates for `agent.env` and the one
///   directory its `--purge` removes. Anywhere else and `--pair` (run from a
///   plain shell, per the docs) and the LaunchAgent would both write issued
///   read-only tokens outside the purge target, so uninstalling with purge
///   would leave live credentials behind.
/// * Linux: `$HOME/services/ebbtop-agent` — the historical path
///   `deploy/install-agent.sh` already uses.
///
/// A pure function so the Darwin rule is testable on Linux, exactly like
/// [`resolve_gate`] and [`cpu_temp_reported`]: a Linux run must be able to
/// prove the Mac rule without a Mac.
pub fn default_state_dir(is_macos: bool, home: &Path) -> PathBuf {
    if is_macos {
        home.join("Library")
            .join("Application Support")
            .join("ebbtop-agent")
    } else {
        home.join("services").join("ebbtop-agent")
    }
}

/// One mounted volume as the platform reports it, before the agent decides
/// what to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total: u64,
    pub available: u64,
}

/// Which volumes the Mac agent reports, and therefore how capacity is summed.
///
/// APFS does not give each volume its own capacity: every volume in a
/// container reports the *container's* total and free space, and the
/// read-only system snapshot plus the writable Data volume are two views of
/// one container. Listing sysinfo's macOS disk list verbatim therefore
/// reports the same gigabytes several times, and a user comparing the agent's
/// total against Finder (which shows the container once) sees the agent
/// overstate the disk.
///
/// The rules, in order:
///   1. drop mount points under `/System/Volumes/` — those are the system
///      snapshot, the Data firmlink and the APFS preboot/recovery/VM volumes,
///      none of which Finder shows as a disk;
///   2. for APFS specifically, keep only the first volume of a container.
///      Container identity is not exposed for a mount point, but a container's
///      capacity is unique enough in practice and is what the duplicate rows
///      share: two APFS volumes with the same `(total, available)` pair come
///      from one container (a distinct container with byte-identical capacity
///      and free space at the same instant is possible but vanishingly
///      unlikely, and collapsing it costs one row, never a wrong number);
///   3. keep every other volume (`/Volumes/USB`, a case-sensitive APFS
///      volume on its own container, HFS+, a disk image) verbatim.
///
/// The caller keeps the order sysinfo gave, so `/` stays first.
pub fn keep_macos_volume(v: &Volume, seen: &mut Vec<(u64, u64)>) -> bool {
    if v.mount_point.starts_with("/System/Volumes/") {
        return false;
    }
    if v.file_system.eq_ignore_ascii_case("apfs") {
        let key = (v.total, v.available);
        if seen.contains(&key) {
            return false;
        }
        seen.push(key);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// Loopback and the ranges a tailnet or LAN uses are accepted.
    #[test]
    fn local_binds_are_accepted() {
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.5.4",
            "192.168.1.50",
            "100.64.0.5",
            "100.127.255.254",
            "169.254.1.1",
            "::1",
            "fd7a:115c:a1e0::1",
            "fe80::1",
            "::ffff:192.168.1.9",
        ] {
            assert!(bind_is_local(ip(s)), "{s} must be accepted");
        }
    }

    /// The wildcard, the unspecified address and public addresses are refused
    /// — the Mac agent must not answer on an untrusted network.
    #[test]
    fn wildcard_unspecified_and_public_binds_are_refused() {
        for s in [
            "0.0.0.0",
            "::",
            "8.8.8.8",
            "172.32.0.1",  // just outside 172.16/12
            "100.128.0.1", // just outside CGNAT
            "203.0.113.7",
            "2001:4860:4860::8888",
        ] {
            assert!(!bind_is_local(ip(s)), "{s} must be refused");
        }
    }

    /// Fail closed: an absent or blank token refuses to start, and the
    /// refusal never contains a value.
    #[test]
    fn enforce_refuses_a_missing_or_blank_token() {
        for token in [None, Some(""), Some("   ")] {
            let err = startup_policy(GatePolicy::Enforce, ip("127.0.0.1"), token)
                .expect_err("must refuse");
            assert!(err.0.contains("without a token"), "{err}");
        }
        assert!(
            startup_policy(GatePolicy::Enforce, ip("127.0.0.1"), Some(&"a".repeat(64))).is_ok()
        );
    }

    /// The bind rule is enforced under Enforce, and its refusal names the
    /// address (an address is not a secret) and says what to do instead.
    #[test]
    fn enforce_refuses_a_public_bind() {
        let err = startup_policy(GatePolicy::Enforce, ip("8.8.8.8"), Some("tok"))
            .expect_err("must refuse");
        assert!(err.0.contains("8.8.8.8"), "{err}");
        assert!(err.0.contains("untrusted"), "{err}");
    }

    /// Linux is untouched: the historical open gate and any bind address
    /// still start, which is what the installed units and docs describe.
    #[test]
    fn permissive_applies_no_policy() {
        for bind in ["0.0.0.0", "8.8.8.8", "127.0.0.1"] {
            for token in [None, Some(""), Some("tok")] {
                assert!(
                    startup_policy(GatePolicy::Permissive, ip(bind), token).is_ok(),
                    "permissive must not refuse {bind}"
                );
            }
        }
    }

    /// Discovery is off in the Mac pilot by default and on for Linux;
    /// an explicit environment value overrides either default elsewhere.
    #[test]
    fn discovery_defaults_differ_by_policy() {
        assert!(!discovery_default(GatePolicy::Enforce));
        assert!(discovery_default(GatePolicy::Permissive));
    }

    /// The Darwin gate cannot be relaxed, and the variable cannot be used to
    /// smuggle a permissive Mac runtime back in — no value of
    /// `EBBTOP_AGENT_REQUIRE_AUTH`, including the falsy ones, changes it. Only a
    /// non-Darwin build reads the variable at all.
    #[test]
    fn the_darwin_gate_cannot_be_turned_off() {
        for v in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("no"),
            Some("off"),
            Some(" FALSE "),
            Some("garbage"),
            Some("1"),
            Some("true"),
        ] {
            assert_eq!(
                resolve_gate(true, v),
                GatePolicy::Enforce,
                "Darwin must fail closed for REQUIRE_AUTH={v:?}"
            );
        }
    }

    /// Linux keeps its historical permissive runtime, and can be *opted in* to
    /// the enforced gate for fixtures. Every falsy/absent value stays permissive:
    /// a typo must never silently tighten a running host either.
    #[test]
    fn linux_is_permissive_by_default_and_opt_in_only() {
        for v in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("no"),
            Some("off"),
            Some("nonsense"),
        ] {
            assert_eq!(resolve_gate(false, v), GatePolicy::Permissive, "{v:?}");
        }
        for v in ["1", "true", "TRUE", " yes ", "on", "On"] {
            assert_eq!(resolve_gate(false, Some(v)), GatePolicy::Enforce, "{v}");
        }
    }

    #[test]
    fn cpu_model_ignores_blank_brands() {
        assert_eq!(
            cpu_model_from_brand("Apple M3 Pro\n"),
            Some("Apple M3 Pro".into())
        );
        assert_eq!(cpu_model_from_brand("  "), None);
        assert_eq!(cpu_model_from_brand(""), None);
    }

    /// F1: macOS must report no CPU temperature. The docs promise
    /// "unavailable — `null`" and this is the gate that keeps that true, so it
    /// is pinned here rather than left to sysinfo's sensor scan (whose IOHID
    /// labels match Apple Silicon `PMU tdie*` die sensors).
    #[test]
    fn macos_reports_no_cpu_temperature() {
        assert!(
            !cpu_temp_reported(true),
            "macOS must not report a CPU temperature"
        );
        // Linux keeps collecting: this fix must not remove a working metric.
        assert!(
            cpu_temp_reported(false),
            "Linux must keep reporting CPU temperature"
        );
    }

    /// F4: the agent's default state directory must be the one its own
    /// installer creates and its own `--uninstall --purge` deletes.
    ///
    /// The macOS installer keeps everything under
    /// `~/Library/Application Support/ebbtop-agent`; a Darwin agent defaulting
    /// to the Linux `~/services/ebbtop-agent` wrote `pairing.json` and
    /// `clients.json` outside that directory, so `--purge` left live read-only
    /// client tokens behind. This pins both directions so a future refactor
    /// cannot silently re-split installer state from agent state.
    #[test]
    fn macos_state_dir_is_the_purgeable_installer_directory() {
        let home = Path::new("/Users/example");
        assert_eq!(
            default_state_dir(true, home),
            Path::new("/Users/example/Library/Application Support/ebbtop-agent"),
            "macOS state must live where the macOS installer installs and purges"
        );
        // Linux keeps its historical path; this fix must not move it.
        assert_eq!(
            default_state_dir(false, home),
            Path::new("/Users/example/services/ebbtop-agent"),
            "Linux state must stay at ~/services/ebbtop-agent"
        );
    }

    fn vol(name: &str, mp: &str, fs: &str, total: u64, avail: u64) -> Volume {
        Volume {
            name: name.into(),
            mount_point: mp.into(),
            file_system: fs.into(),
            total,
            available: avail,
        }
    }

    /// The real Apple Silicon layout: one container mounted at `/` and
    /// firmlinked into `/System/Volumes/*`, each reporting the same capacity.
    /// Exactly one row survives, and it is `/`.
    #[test]
    fn apfs_container_is_reported_once() {
        let volumes = vec![
            vol(
                "Macintosh HD",
                "/",
                "apfs",
                994_662_584_320,
                412_316_057_600,
            ),
            vol(
                "Macintosh HD",
                "/System/Volumes/Data",
                "apfs",
                994_662_584_320,
                412_316_057_600,
            ),
            vol(
                "com.apple.os.update-ABC",
                "/System/Volumes/Update",
                "apfs",
                994_662_584_320,
                412_316_057_600,
            ),
            vol(
                "Preboot",
                "/System/Volumes/Preboot",
                "apfs",
                994_662_584_320,
                412_316_057_600,
            ),
        ];
        let mut seen = Vec::new();
        let kept: Vec<&Volume> = volumes
            .iter()
            .filter(|v| keep_macos_volume(v, &mut seen))
            .collect();
        assert_eq!(
            kept.len(),
            1,
            "one container must produce one row: {kept:?}"
        );
        assert_eq!(kept[0].mount_point, "/");
    }

    /// A second APFS volume on its *own* container (different capacity) and a
    /// USB stick are both kept; the System/Volumes filter must not hide them.
    #[test]
    fn distinct_volumes_are_kept() {
        let volumes = vec![
            vol(
                "Macintosh HD",
                "/",
                "apfs",
                994_662_584_320,
                412_316_057_600,
            ),
            vol(
                "Case Sensitive",
                "/Volumes/Case Sensitive",
                "apfs",
                500_107_862_016,
                499_000_000_000,
            ),
            vol(
                "USB",
                "/Volumes/USB",
                "msdos",
                32_000_000_000,
                31_000_000_000,
            ),
            vol(
                "Backup",
                "/Volumes/Backup",
                "hfs",
                1_000_204_886_016,
                900_000_000_000,
            ),
        ];
        let mut seen = Vec::new();
        let kept: Vec<&Volume> = volumes
            .iter()
            .filter(|v| keep_macos_volume(v, &mut seen))
            .collect();
        assert_eq!(
            kept.iter()
                .map(|v| v.mount_point.as_str())
                .collect::<Vec<_>>(),
            vec![
                "/",
                "/Volumes/Case Sensitive",
                "/Volumes/USB",
                "/Volumes/Backup"
            ]
        );
    }

    /// Two APFS volumes with the same capacity pair are one container and
    /// collapse even when neither is under /System/Volumes.
    #[test]
    fn duplicate_apfs_capacity_collapses() {
        let volumes = vec![
            vol("A", "/Volumes/A", "apfs", 500_000_000_000, 400_000_000_000),
            vol("B", "/Volumes/B", "apfs", 500_000_000_000, 400_000_000_000),
        ];
        let mut seen = Vec::new();
        let kept: Vec<&Volume> = volumes
            .iter()
            .filter(|v| keep_macos_volume(v, &mut seen))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "A");
    }

    /// The same capacity pair on a non-APFS filesystem is not a container
    /// claim: two HFS+ disks are two disks.
    #[test]
    fn equal_capacity_on_non_apfs_is_not_collapsed() {
        let volumes = vec![
            vol("One", "/Volumes/One", "hfs", 1, 1),
            vol("Two", "/Volumes/Two", "hfs", 1, 1),
        ];
        let mut seen = Vec::new();
        let kept: Vec<&Volume> = volumes
            .iter()
            .filter(|v| keep_macos_volume(v, &mut seen))
            .collect();
        assert_eq!(kept.len(), 2);
    }
}
