//! ebbtop P3a: the `ebbtop-agent --pair` code generator.
//!
//! Runs the real binary (`CARGO_BIN_EXE_ebbtop-agent`, the path cargo builds
//! for integration tests), so this covers argument parsing, the state-dir
//! resolution, the file it writes and what it prints.

use std::fs;
use std::process::Command;

fn pair_cmd(dir: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ebbtop-agent"));
    c.arg("--pair")
        .env("EBBTOP_STATE_DIR", dir)
        .env_remove("EBBTOP_TOKEN")
        .env_remove("SOCKTOP_TOKEN");
    c
}

#[test]
fn pair_prints_a_six_digit_code_and_writes_the_pending_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = pair_cmd(dir.path()).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(stdout.contains("Pairing code: "), "stdout: {stdout}");
    assert!(stdout.contains("single use"), "stdout: {stdout}");

    let code = stdout
        .lines()
        .find_map(|l| l.strip_prefix("Pairing code: "))
        .unwrap()
        .trim()
        .to_string();
    assert_eq!(code.len(), 6);
    assert!(code.bytes().all(|b| b.is_ascii_digit()), "code: {code}");

    // The file the running agent consumes holds the same code...
    let raw = fs::read_to_string(dir.path().join("pairing.json")).unwrap();
    let pending: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(pending["code"], code);
    assert_eq!(pending["attempts"], 0);
    assert!(pending["expires_at_ms"].as_u64().unwrap() > 0);

    // ...and the process printed no agent token (it prints no credential at
    // all: only the code the operator asked for).
    assert!(
        !stdout.contains("EBBTOP_TOKEN"),
        "stdout mentions the token env: {stdout}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir.path().join("pairing.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn pair_replaces_a_previous_pending_code() {
    let dir = tempfile::tempdir().unwrap();
    let first = pair_cmd(dir.path()).output().unwrap();
    let second = pair_cmd(dir.path()).output().unwrap();
    let code = |o: &std::process::Output| {
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .find_map(|l| l.strip_prefix("Pairing code: ").map(|s| s.trim().to_string()))
            .unwrap()
    };
    assert!(first.status.success() && second.status.success());
    let on_disk = fs::read_to_string(dir.path().join("pairing.json")).unwrap();
    assert!(
        on_disk.contains(&code(&second)),
        "the newest minted code must be the pending one"
    );
    assert!(
        !on_disk.contains(&code(&first)) || code(&first) == code(&second),
        "the previous code must be replaced"
    );
}

#[test]
fn pair_does_not_require_the_agent_to_be_running() {
    // No daemon, no port, no token: a pure file write.
    let dir = tempfile::tempdir().unwrap();
    let out = pair_cmd(dir.path()).output().unwrap();
    assert!(out.status.success());
    assert!(dir.path().join("pairing.json").exists());
}
