//! Argument parsing for the agent binary, in the library so the integration
//! tests exercise the production parser (the previous test shipped its own
//! copy of the logic and drifted from it silently).
//!
//! CLI acceptance semantics are exactly the binary's historical ones: a
//! value-taking flag is `--name value` (no `--name=value` form), `--port`
//! and `-p` name the same setting, and an unrecognized flag is ignored by
//! the getters rather than refused — the shipped unit, LaunchAgent and
//! install scripts pass a wide flag set, and failing loudly on an unknown
//! one would turn a packaging typo into a host with no monitoring agent.

/// The value after `name` in `args`, or `None` when the flag is absent or
/// last (a trailing flag with no value reads as absent, not as a panic).
pub fn arg_value(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().cloned();
        }
    }
    None
}

/// Whether `name` appears in `args` at all.
pub fn arg_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// Port resolution, in order: `--port`, `-p`, then the env fallback the
/// caller supplies (`EBBTOP_PORT`, then `SOCKTOP_PORT` compat), else 3000 —
/// the packaged unit passes `--port 45976`; the binary default stays
/// upstream's 3000. An unparseable value falls through exactly like the
/// historical chain: a bad `--port` does not reach the env fallbacks.
pub fn parse_port(args: &[String], env_fallback: Option<&str>) -> u16 {
    arg_value(args, "--port")
        .or_else(|| arg_value(args, "-p"))
        .or_else(|| env_fallback.map(str::to_string))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(3000)
}

/// Bind address resolution, in order: `--bind`, then the env fallback the
/// caller supplies (`EBBTOP_BIND`, then `SOCKTOP_BIND` compat), else the
/// wildcard. `Err` carries the operator-facing message with the raw value;
/// it contains no secret (an address is not a credential).
pub fn parse_bind(args: &[String], env_fallback: Option<&str>) -> Result<std::net::IpAddr, String> {
    let s = arg_value(args, "--bind")
        .or_else(|| env_fallback.map(str::to_string))
        .unwrap_or_else(|| "0.0.0.0".to_string());
    s.parse()
        .map_err(|e| format!("invalid bind address {s:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn port_prefers_the_long_flag_then_the_short_one_then_env() {
        assert_eq!(
            parse_port(&argv(&["ebbtop-agent", "--port", "45976"]), None),
            45976
        );
        assert_eq!(
            parse_port(&argv(&["ebbtop-agent", "-p", "45976"]), None),
            45976
        );
        assert_eq!(
            parse_port(&argv(&["ebbtop-agent", "--port", "1", "-p", "2"]), None),
            1,
            "--port wins over -p"
        );
        assert_eq!(
            parse_port(&argv(&["ebbtop-agent"]), Some("45976")),
            45976,
            "env fallback applies when no flag is given"
        );
        assert_eq!(
            parse_port(&argv(&["ebbtop-agent", "--port", "x"]), Some("45976")),
            3000,
            "a bad flag value falls through to the default, not to env"
        );
        assert_eq!(parse_port(&argv(&["ebbtop-agent"]), None), 3000);
        // The historical parser never accepted the --port=9003 form; it
        // binds the default instead. Pinned so a silent change is caught.
        assert_eq!(
            parse_port(&argv(&["ebbtop-agent", "--port=9003"]), None),
            3000
        );
    }

    #[test]
    fn bind_resolves_flag_then_env_then_wildcard() {
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        assert_eq!(
            parse_bind(&argv(&["ebbtop-agent", "--bind", "127.0.0.1"]), None).unwrap(),
            ip("127.0.0.1")
        );
        assert_eq!(
            parse_bind(&argv(&["ebbtop-agent"]), Some("10.0.0.5")).unwrap(),
            ip("10.0.0.5")
        );
        assert_eq!(
            parse_bind(&argv(&["ebbtop-agent"]), None).unwrap(),
            ip("0.0.0.0")
        );
        let err = parse_bind(&argv(&["ebbtop-agent", "--bind", "not-an-ip"]), None)
            .expect_err("must refuse");
        assert!(err.contains("not-an-ip"), "{err}");
        // A flag beats env even when the flag value is unusable: the error
        // names the flag's value, not the env one.
        let err = parse_bind(&argv(&["ebbtop-agent", "--bind", "nope"]), Some("10.0.0.5"))
            .expect_err("must refuse");
        assert!(err.contains("\"nope\""), "{err}");
    }

    #[test]
    fn values_and_flags_scan_the_real_argv_shape() {
        let a = argv(&[
            "ebbtop-agent",
            "--probe",
            "h:1",
            "--token-file",
            "/t",
            "--dump-metrics",
        ]);
        assert_eq!(arg_value(&a, "--probe").as_deref(), Some("h:1"));
        assert_eq!(arg_value(&a, "--token-file").as_deref(), Some("/t"));
        assert!(arg_flag(&a, "--dump-metrics"));
        assert!(!arg_flag(&a, "--pair"));
        // A trailing flag with no value reads as absent.
        assert_eq!(
            arg_value(&argv(&["ebbtop-agent", "--state-dir"]), "--state-dir"),
            None
        );
    }
}
