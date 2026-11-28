//! ebbtop: a conservative `KEY=VALUE` environment-file reader.
//!
//! systemd can point a unit at an `EnvironmentFile`; launchd cannot. The
//! packaged macOS LaunchAgent therefore starts the agent with
//! `--env-file <path>`, so the plist and the process argument list carry a
//! path and never a token, and the bind address, token and tuning knobs stay
//! in one 0600 file that the installer already writes for Linux.
//!
//! The accepted grammar is the same narrow subset `deploy/install-agent.sh`
//! checks before it trusts an existing `agent.env` on Linux, because both
//! sides must read one file the same way:
//!
//! * TAB, LF, CR and printable ASCII only. NUL, VT, FF and every non-ASCII
//!   byte are refused: systemd treats VT/FF as part of a key (so the file
//!   would mean something different there) and a non-ASCII indent is not
//!   whitespace to either parser, so refusing is the only reading both
//!   agree on.
//! * CR ends a line, as in systemd's parser.
//! * `#` and `;` start a comment; blank lines are skipped.
//! * a non-comment line is `KEY=VALUE`, key `[A-Za-z_][A-Za-z0-9_]*`,
//!   surrounding space/TAB stripped from key and value, repeated keys
//!   replaced so the LAST assignment wins.
//! * quoted or backslash-escaped values are refused rather than
//!   half-interpreted.
//!
//! Only keys on [`ALLOWED_KEYS`] are applied, and a file naming any other key
//! is refused: an environment file must never be able to set `LD_PRELOAD`,
//! `PATH` or anything else the agent does not read. A refusal names the key
//! and the line, never a value (a value can be a token).

/// Environment variables the agent itself reads. Anything else in a
/// `--env-file` is a mistake worth refusing, not something to inject.
pub const ALLOWED_KEYS: &[&str] = &[
    "EBBTOP_BIND",
    "EBBTOP_TOKEN",
    "EBBTOP_STATE_DIR",
    "EBBTOP_AGENT_DISCOVERY",
    "EBBTOP_DISCOVERY_PORT",
    "EBBTOP_AGENT_GPU",
    "EBBTOP_AGENT_TEMP",
    "EBBTOP_AGENT_METRICS_TTL_MS",
    "EBBTOP_AGENT_DISKS_TTL_MS",
    "EBBTOP_AGENT_PROCESSES_TTL_MS",
    "EBBTOP_AGENT_NAME_CACHE_CLEANUP_THRESHOLD",
    "EBBTOP_WORKER_THREADS",
    // Pre-rename names, kept because an agent.env written by an older
    // installer may still carry them; the agent reads both.
    "SOCKTOP_BIND",
    "SOCKTOP_TOKEN",
    "SOCKTOP_AGENT_METRICS_TTL_MS",
    "SOCKTOP_AGENT_DISKS_TTL_MS",
    "SOCKTOP_AGENT_PROCESSES_TTL_MS",
];

/// One accepted assignment, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub key: String,
    pub value: String,
    pub line: usize,
}

/// Parse an environment file's text. Errors are operator-facing and never
/// contain a value.
pub fn parse(text: &str) -> Result<Vec<Assignment>, String> {
    // Raw-byte checks first: a NUL ends systemd's parse and a control or
    // non-ASCII byte makes the two readers disagree about the key.
    //
    // The refusal names the OFFSET and the allowed byte set, never the byte
    // itself: a multi-byte character's first byte (0xc2 for U+00A0) says
    // nothing useful to the operator, and echoing input bytes into a message
    // that lands in a terminal and a log is how secrets leak.
    for (i, b) in text.bytes().enumerate() {
        let ok = b == b'\t' || b == b'\n' || b == b'\r' || (0x20..=0x7e).contains(&b);
        if !ok {
            return Err(format!(
                "unsupported environment file: the byte at offset {i} is outside the accepted set; \
                 only printable ASCII (0x20-0x7e), TAB, CR and LF are allowed. Non-ASCII text \
                 such as a non-breaking space counts as unsupported too"
            ));
        }
    }

    let mut out: Vec<Assignment> = Vec::new();
    // CR terminates a line exactly as LF does.
    for (idx, raw_line) in text.split(['\n', '\r']).enumerate() {
        let lineno = idx + 1;
        let line = raw_line.trim_matches([' ', '\t']);
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!(
                "unsupported line {lineno}: a non-comment line without '=' (write KEY=VALUE)"
            ));
        };
        let key = k.trim_matches([' ', '\t']);
        // A malformed key is refused with a FIXED message that echoes nothing
        // from the line: the offending text may itself be a secret (a pasted
        // export line, a token with an '=' in it), and a refusal is printed to
        // a terminal and often captured in a log.
        if key.is_empty()
            || !key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(format!(
                "unsupported line {lineno}: not a plain KEY=VALUE assignment (a key is letters, \
                 digits and '_', starting with a letter or '_')"
            ));
        }
        let value = v.trim_matches([' ', '\t']);
        if value.contains('"') || value.contains('\'') || value.contains('\\') {
            return Err(format!(
                "unsupported line {lineno}: quoted or escaped values are not supported \
                 (write {key}=<plain value>)"
            ));
        }
        if !ALLOWED_KEYS.contains(&key) {
            // The key passed the plain-name check above, so it is syntax, not
            // data: naming it tells the operator which line to fix.
            return Err(format!(
                "unsupported line {lineno}: '{key}' is not an ebbtop setting the agent reads"
            ));
        }
        // Last assignment wins, as in systemd.
        if let Some(existing) = out.iter_mut().find(|a| a.key == key) {
            existing.value = value.to_string();
            existing.line = lineno;
        } else {
            out.push(Assignment {
                key: key.to_string(),
                value: value.to_string(),
                line: lineno,
            });
        }
    }
    Ok(out)
}

/// Apply parsed assignments to this process's environment. A variable the
/// caller already exported wins, so `EBBTOP_TOKEN=... ebbtop-agent` still
/// overrides the file.
///
/// # Safety of the call site
/// `std::env::set_var` is unsafe in Rust 2024 because it can race another
/// thread's `getenv`. Callers must run this before any thread is spawned —
/// `main` reads `--env-file` before it builds the tokio runtime.
pub fn apply(assignments: &[Assignment]) -> usize {
    let mut applied = 0;
    for a in assignments {
        if std::env::var_os(&a.key).is_none_or(|v| v.is_empty()) {
            // SAFETY: single-threaded by contract (see the doc comment).
            unsafe { std::env::set_var(&a.key, &a.value) };
            applied += 1;
        }
    }
    applied
}

/// Read, validate and apply `path`. Returns how many variables it set.
pub fn load(path: &std::path::Path) -> Result<usize, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read --env-file {}: {e}", path.display()))?;
    let assignments = parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(apply(&assignments))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<(String, String)> {
        parse(text)
            .expect("should parse")
            .into_iter()
            .map(|a| (a.key, a.value))
            .collect()
    }

    /// The shipped agent.env shape: two bind/key lines, TTLs, and the
    /// discovery switch. Parsed in file order, values verbatim.
    #[test]
    fn parses_the_installer_written_file() {
        let text = "EBBTOP_BIND=100.64.0.5\n\
                    EBBTOP_AGENT_PROCESSES_TTL_MS=500\n\
                    EBBTOP_AGENT_METRICS_TTL_MS=100\n\
                    EBBTOP_AGENT_DISCOVERY=0\n\
                    EBBTOP_TOKEN=0123456789abcdef\n";
        assert_eq!(
            keys(text),
            vec![
                ("EBBTOP_BIND".into(), "100.64.0.5".into()),
                ("EBBTOP_AGENT_PROCESSES_TTL_MS".into(), "500".into()),
                ("EBBTOP_AGENT_METRICS_TTL_MS".into(), "100".into()),
                ("EBBTOP_AGENT_DISCOVERY".into(), "0".into()),
                ("EBBTOP_TOKEN".into(), "0123456789abcdef".into()),
            ]
        );
    }

    /// Comments, blank lines, CRLF, indentation and tabs are all accepted,
    /// and the LAST duplicate wins (systemd's rule).
    #[test]
    fn comments_crlf_indent_and_duplicates() {
        let text = "# a comment\r\n; another\r\n\r\n  EBBTOP_BIND = 10.0.0.2 \r\n\
                    \tEBBTOP_BIND\t=\t10.0.0.3\t\r\n";
        assert_eq!(keys(text), vec![("EBBTOP_BIND".into(), "10.0.0.3".into())]);
    }

    /// Every refusal names the line (or offset) and, at most, a well-formed
    /// key, and never echoes a value, a malformed key or a raw input byte:
    /// any of those can be the secret (a pasted `export TOKEN=...` line, a
    /// token containing '=').
    #[test]
    fn refuses_dangerous_or_ambiguous_input_without_leaking_values() {
        let cases = [
            ("EBBTOP_TOKEN=a\0b\n", "outside the accepted set"),
            ("EBBTOP_BIND=1.2.3.4\nnot-an-assignment\n", "without '='"),
            ("LD_PRELOAD=/tmp/x.so\n", "not an ebbtop setting"),
            ("EBBTOP_TOKEN=\"abcdef\"\n", "not supported"),
            ("EBBTOP_TOKEN=a\\b\n", "not supported"),
            ("EBBTOP_BIND\n", "without '='"),
            ("1BAD=x\n", "not a plain KEY=VALUE"),
            // U+00A0 is two bytes (0xc2 0xa0); the refusal reports the offset
            // and the accepted range, not either byte.
            ("EBBTOP_BIND=1.2.3.4\u{a0}\n", "outside the accepted set"),
            ("export EBBTOP_TOKEN=abcdef\n", "not a plain KEY=VALUE"),
        ];
        for (text, needle) in cases {
            let err = parse(text).expect_err(&format!("should refuse {text:?}"));
            assert!(err.contains(needle), "{text:?} -> {err}");
            assert!(!err.contains("abcdef"), "refusal leaked a value: {err}");
            assert!(!err.contains("1.2.3.4"), "refusal leaked a value: {err}");
            // Malformed key text must not come back either.
            assert!(
                !err.contains("export SECRET"),
                "refusal echoed a malformed key: {err}"
            );
            assert!(
                !err.contains("export EBBTOP_TOKEN"),
                "refusal echoed a malformed key: {err}"
            );
            // Nor a raw byte rendered as hex.
            assert!(!err.contains("0xc2"), "refusal echoed an input byte: {err}");
            assert!(!err.contains("0xa0"), "refusal echoed an input byte: {err}");
        }
    }

    /// A non-ASCII character is refused at its first byte's offset, and the
    /// message is identical for any such input (only the offset moves), so it
    /// cannot be used to recover the bytes.
    #[test]
    fn non_ascii_refusal_reports_the_offset_only() {
        // "EBBTOP_BIND=x" is 13 bytes, so U+00A0's first byte (0xc2) is at
        // offset 13.
        let err = parse("EBBTOP_BIND=x\u{a0}\n").unwrap_err();
        assert!(err.contains("offset 13"), "{err}");
        assert!(err.contains("0x20-0x7e"), "{err}");
        assert!(!err.contains("0xc2") && !err.contains("0xa0"), "{err}");
        // A 4-byte character is refused at the same offset, with no byte named.
        let err = parse("EBBTOP_BIND=x\u{1f600}\n").unwrap_err();
        assert!(err.contains("offset 13"), "{err}");
        assert!(!err.contains("0xf0") && !err.contains("0x9f"), "{err}");
    }

    /// The malformed-key refusal is a fixed string: identical for any input
    /// that trips it, so it cannot be used as an oracle for the bytes.
    #[test]
    fn malformed_key_refusal_is_stable_and_value_free() {
        let a = parse("export SECRET=1\n").unwrap_err();
        let b = parse("export OTHER=2\n").unwrap_err();
        let c = parse("1BAD=x\n").unwrap_err();
        let d = parse("a b c=d\n").unwrap_err();
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a, d);
        // Only the line number varies, and it is an index, not content.
        assert_eq!(
            a,
            "unsupported line 1: not a plain KEY=VALUE assignment (a key is letters, digits and '_', starting with a letter or '_')"
        );
    }

    /// A line with only a key and no '=' is a mistake, not an empty value.
    #[test]
    fn empty_file_is_valid_and_empty_value_is_kept() {
        assert!(parse("").unwrap().is_empty());
        assert_eq!(
            keys("EBBTOP_TOKEN=\n"),
            vec![("EBBTOP_TOKEN".into(), "".into())]
        );
    }
}
