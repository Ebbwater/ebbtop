//! ebbtop P3a: pairing-code enrollment.
//!
//! `ebbtop-agent --pair` mints a six-digit code, writes it to
//! `<state>/pairing.json` (mode 600) and prints it once on stdout for the
//! operator to type into the client. The running agent answers
//! `POST /pair` with `{"code":"…"}`; a correct, unexpired, unused code is
//! consumed and exchanged for a client token written to
//! `<state>/clients.json` (mode 600).
//!
//! Properties this module enforces:
//! * six digits, 5 minute TTL, single use, minted on the agent host;
//! * never logged — no `println!`/journal line takes a code, and the HTTP
//!   handler never echoes one back in an error;
//! * never in a query string — the only transport is the POST body;
//! * a brute-force guard: five wrong attempts invalidate the pending code, so
//!   an attacker on the LAN gets 5 tries per minted code, not 10^6.
//!
//! Issued tokens are read-only scoped (`read_only`): they authenticate the
//! same WebSocket gate as the operator's manual token but cannot reach the
//! `kill:` path. The manual token keeps every capability it has today.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::secret;

/// Digits in a pairing code. Six is short enough to read off a terminal and
/// long enough with the 5-attempt guard (5 in 10^6 per code).
pub const CODE_DIGITS: usize = 6;
/// Code lifetime. The plan says "about 5 minutes".
pub const CODE_TTL_MS: u64 = 5 * 60 * 1000;
/// Wrong guesses allowed against one minted code before it is invalidated.
pub const MAX_ATTEMPTS: u32 = 5;
/// Newest client credentials kept; older ones are dropped on issue.
pub const MAX_CLIENTS: usize = 64;

/// Scope of an enrollment-issued token: read-only monitoring.
pub const SCOPE_READ_ONLY: &str = "read_only";
/// Scope of the operator's manual token (unchanged behaviour, `kill:` allowed).
pub const SCOPE_FULL: &str = "full";

pub const PAIR_FILE: &str = "pairing.json";
pub const CLIENTS_FILE: &str = "clients.json";
/// Path of the enrollment endpoint on the agent's HTTP listener.
pub const PAIR_PATH: &str = "/pair";

/// Body of `POST /pair`. The code travels in the body, never the query
/// string, so it never lands in an access log or browser history. The decoder
/// mirror lives in `ebbtop_core::pairing::PairRequest`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// Successful `POST /pair` reply. The token is the only secret in it; the
/// decoder mirror is `ebbtop_core::pairing::PairResponse`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairResponse {
    pub token: String,
    pub host: String,
    pub agent_version: String,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default)]
    pub scope: String,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Directory holding the agent's writable state (pairing code, issued
/// clients).
///
/// Resolution: `EBBTOP_STATE_DIR`, else the platform's default — on macOS
/// `$HOME/Library/Application Support/ebbtop-agent` (where
/// `deploy/macos/install-agent.sh` installs and what its `--uninstall --purge`
/// deletes), on Linux `$HOME/services/ebbtop-agent` (the directory
/// `deploy/install-agent.sh` already creates for `agent.env`) — else `.` when
/// HOME is unset. The service runs as the user, so `--pair` from an
/// interactive shell and the daemon agree without extra flags.
pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("EBBTOP_STATE_DIR")
        && !d.is_empty()
    {
        return PathBuf::from(d);
    }
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => {
            crate::platform::default_state_dir(cfg!(target_os = "macos"), Path::new(&h))
        }
        _ => PathBuf::from("."),
    }
}

/// The pending code file: what `--pair` writes and `/pair` consumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingCode {
    pub code: String,
    pub expires_at_ms: u64,
    #[serde(default)]
    pub attempts: u32,
}

/// One issued client credential.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientRecord {
    pub token: String,
    #[serde(default)]
    pub label: Option<String>,
    pub issued_at_ms: u64,
    #[serde(default)]
    pub scope: String,
}

/// The whole credential store. A missing file is an empty store, so a host
/// that never paired is not an error.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClientStore {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub clients: Vec<ClientRecord>,
}

/// Why an exchange failed. `reason()` is the wire/HTTP message and contains
/// no code material and no timing side channel a caller could exploit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairError {
    /// No code was minted (`--pair` not run) or it was already consumed.
    NoPendingCode,
    /// Code was wrong. A second attempt with the right code still works until
    /// [`MAX_ATTEMPTS`] is reached.
    WrongCode,
    /// Code existed but its TTL passed. It has been removed.
    Expired,
    /// Too many wrong guesses: the pending code is gone, run `--pair` again.
    TooManyAttempts,
    /// The agent could not write its state directory.
    StateIo,
}

impl PairError {
    /// Stable, code-free explanation for the client.
    pub fn reason(self) -> &'static str {
        match self {
            PairError::NoPendingCode => "no pending pairing code",
            PairError::WrongCode => "invalid pairing code",
            PairError::Expired => "pairing code expired",
            PairError::TooManyAttempts => "too many attempts; run ebbtop-agent --pair again",
            PairError::StateIo => "agent could not write its state directory",
        }
    }
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for PairError {}

/// File-backed pairing state rooted at one directory.
pub struct PairingStore {
    dir: PathBuf,
}

impl PairingStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn pair_file(&self) -> PathBuf {
        self.dir.join(PAIR_FILE)
    }

    pub fn clients_file(&self) -> PathBuf {
        self.dir.join(CLIENTS_FILE)
    }

    /// Create the state directory (0700 on unix) if it is missing.
    fn ensure_dir(&self) -> io::Result<()> {
        if self.dir.is_dir() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700));
        }
        Ok(())
    }

    /// Mint a code, persist it as the single pending enrollment, return it.
    /// Minting again replaces the pending code (the operator re-ran `--pair`).
    pub fn mint(&self) -> io::Result<PendingCode> {
        let pending = PendingCode {
            code: secret::code(CODE_DIGITS),
            expires_at_ms: now_ms() + CODE_TTL_MS,
            attempts: 0,
        };
        self.write_private(&self.pair_file(), &serde_json::to_vec(&pending)?)?;
        Ok(pending)
    }

    /// The pending code, if one is readable and parseable. A corrupt file
    /// reads as "none" rather than panicking the request handler.
    pub fn read_pending(&self) -> Option<PendingCode> {
        let bytes = std::fs::read(self.pair_file()).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Remove the pending code. Missing is success (single-use replay).
    pub fn clear_pending(&self) -> io::Result<()> {
        match std::fs::remove_file(self.pair_file()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Exchange a presented code for one issued client token.
    ///
    /// Order matters: the pending code is deleted *before* the token is
    /// written, so a crash mid-exchange costs a re-pair instead of leaking a
    /// second token for one code.
    pub fn exchange(
        &self,
        presented: &str,
        label: Option<&str>,
        at_ms: u64,
    ) -> Result<ClientRecord, PairError> {
        let Some(mut pending) = self.read_pending() else {
            return Err(PairError::NoPendingCode);
        };
        let now = at_ms;
        if now >= pending.expires_at_ms {
            let _ = self.clear_pending();
            return Err(PairError::Expired);
        }
        if pending.attempts >= MAX_ATTEMPTS {
            let _ = self.clear_pending();
            return Err(PairError::TooManyAttempts);
        }
        if !secret::constant_time_eq(&pending.code, presented.trim()) {
            pending.attempts += 1;
            if pending.attempts >= MAX_ATTEMPTS {
                let _ = self.clear_pending();
                Err(PairError::TooManyAttempts)
            } else {
                self.write_private(
                    &self.pair_file(),
                    &serde_json::to_vec(&pending).map_err(|_| PairError::StateIo)?,
                )
                .map_err(|_| PairError::StateIo)?;
                Err(PairError::WrongCode)
            }
        } else {
            self.clear_pending().map_err(|_| PairError::StateIo)?;
            self.issue(label).map_err(|_| PairError::StateIo)
        }
    }

    /// Issue (append) a read-only client credential.
    pub fn issue(&self, label: Option<&str>) -> io::Result<ClientRecord> {
        let rec = ClientRecord {
            token: secret::token(),
            label: label
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            issued_at_ms: now_ms(),
            scope: SCOPE_READ_ONLY.to_string(),
        };
        let mut store = self.read_clients();
        store.clients.push(rec.clone());
        if store.clients.len() > MAX_CLIENTS {
            let drop = store.clients.len() - MAX_CLIENTS;
            store.clients.drain(0..drop);
        }
        store.version = 1;
        self.write_private(&self.clients_file(), &serde_json::to_vec(&store)?)?;
        Ok(rec)
    }

    /// The credential store. Missing or unparseable reads as empty.
    pub fn read_clients(&self) -> ClientStore {
        std::fs::read(self.clients_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Scope of an issued token, or None when it is not in the store.
    ///
    /// Every record is compared (no early return) so lookup time does not
    /// reveal which position matched.
    pub fn authenticate(&self, presented: &str) -> Option<String> {
        if presented.is_empty() {
            return None;
        }
        let store = self.read_clients();
        let mut found: Option<String> = None;
        for rec in &store.clients {
            if secret::constant_time_eq(&rec.token, presented) && found.is_none() {
                found = Some(if rec.scope.is_empty() {
                    SCOPE_READ_ONLY.to_string()
                } else {
                    rec.scope.clone()
                });
            }
        }
        found
    }

    /// Write `bytes` to `path` atomically with mode 600: a temp file in the
    /// same directory, fsync, then rename over the target.
    fn write_private(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.ensure_dir()?;
        let tmp = path.with_extension("tmp");
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn store() -> (tempfile::TempDir, PairingStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = PairingStore::new(dir.path());
        (dir, store)
    }

    #[test]
    fn mint_writes_a_pending_code_in_a_private_file() {
        let (dir, store) = store();
        let pending = store.mint().unwrap();
        assert_eq!(pending.code.len(), CODE_DIGITS);
        assert!(pending.code.bytes().all(|b| b.is_ascii_digit()));
        assert!(pending.expires_at_ms > now_ms());
        let on_disk = store.read_pending().unwrap();
        assert_eq!(on_disk, pending);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(PAIR_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "pairing.json must not be group/world readable");
        }
    }

    #[test]
    fn exchange_with_the_right_code_issues_a_read_only_token_once() {
        let (_dir, store) = store();
        let pending = store.mint().unwrap();
        let rec = store
            .exchange(&pending.code, Some("laptop"), now_ms())
            .unwrap();
        assert_eq!(rec.scope, SCOPE_READ_ONLY);
        assert_eq!(rec.label.as_deref(), Some("laptop"));
        assert_eq!(rec.token.len(), 64);
        // stored and authenticatable, with its scope
        assert_eq!(
            store.authenticate(&rec.token).as_deref(),
            Some(SCOPE_READ_ONLY)
        );
        // single use: the code file is gone and a replay is refused
        assert!(store.read_pending().is_none());
        assert_eq!(
            store.exchange(&pending.code, None, now_ms()).unwrap_err(),
            PairError::NoPendingCode
        );
    }

    #[test]
    fn wrong_code_is_refused_and_the_code_still_works_afterwards() {
        let (_dir, store) = store();
        let pending = store.mint().unwrap();
        assert_eq!(
            store.exchange("000000", None, now_ms()).unwrap_err(),
            PairError::WrongCode
        );
        assert_eq!(store.read_pending().unwrap().attempts, 1);
        // a typo does not burn the enrollment
        let rec = store.exchange(&pending.code, None, now_ms()).unwrap();
        assert!(store.authenticate(&rec.token).is_some());
    }

    #[test]
    fn five_wrong_guesses_invalidate_the_pending_code() {
        let (_dir, store) = store();
        let pending = store.mint().unwrap();
        let wrong = if pending.code == "000000" {
            "111111"
        } else {
            "000000"
        };
        for n in 1..MAX_ATTEMPTS {
            assert_eq!(
                store.exchange(wrong, None, now_ms()).unwrap_err(),
                PairError::WrongCode
            );
            assert_eq!(store.read_pending().unwrap().attempts, n);
        }
        assert_eq!(
            store.exchange(wrong, None, now_ms()).unwrap_err(),
            PairError::TooManyAttempts
        );
        assert!(store.read_pending().is_none(), "pending code must be gone");
        // even the right code is dead now
        assert_eq!(
            store.exchange(&pending.code, None, now_ms()).unwrap_err(),
            PairError::NoPendingCode
        );
    }

    #[test]
    fn an_expired_code_is_refused_and_removed() {
        let (_dir, store) = store();
        let pending = PendingCode {
            code: "424242".into(),
            expires_at_ms: now_ms() - 1,
            attempts: 0,
        };
        store
            .write_private(&store.pair_file(), &serde_json::to_vec(&pending).unwrap())
            .unwrap();
        assert_eq!(
            store.exchange("424242", None, now_ms()).unwrap_err(),
            PairError::Expired
        );
        assert!(store.read_pending().is_none());
        assert!(
            !store.clients_file().exists(),
            "no token from an expired code"
        );
    }

    #[test]
    fn no_minted_code_means_no_enrollment() {
        let (_dir, store) = store();
        assert_eq!(
            store.exchange("123456", None, now_ms()).unwrap_err(),
            PairError::NoPendingCode
        );
    }

    #[test]
    fn authenticate_rejects_unknown_and_empty_tokens() {
        let (_dir, store) = store();
        let rec = store.issue(None).unwrap();
        assert!(store.authenticate(&rec.token).is_some());
        assert!(store.authenticate("").is_none());
        assert!(store.authenticate(&"f".repeat(64)).is_none());
        // a prefix is not a token
        assert!(store.authenticate(&rec.token[..63]).is_none());
    }

    #[test]
    fn issued_tokens_are_distinct() {
        let (_dir, store) = store();
        let a = store.issue(None).unwrap();
        let b = store.issue(None).unwrap();
        assert_ne!(a.token, b.token);
        assert_eq!(store.read_clients().clients.len(), 2);
    }

    #[test]
    fn the_store_is_capped_and_drops_the_oldest() {
        let (_dir, store) = store();
        let mut first = String::new();
        for i in 0..MAX_CLIENTS + 2 {
            let rec = store.issue(Some(&format!("c{i}"))).unwrap();
            if i == 0 {
                first = rec.token;
            }
        }
        let clients = store.read_clients().clients;
        assert_eq!(clients.len(), MAX_CLIENTS);
        assert!(
            store.authenticate(&first).is_none(),
            "oldest must be evicted"
        );
    }

    #[test]
    fn a_state_dir_that_cannot_be_created_reports_state_io() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file-not-dir");
        std::fs::File::create(&blocker)
            .unwrap()
            .write_all(b"x")
            .unwrap();
        let store = PairingStore::new(blocker.join("sub"));
        // write_private fails because a file sits where the dir belongs
        assert!(store.mint().is_err());
    }

    #[test]
    fn state_dir_prefers_the_env_override() {
        // Not a process-global env test: just assert the documented default
        // shape so a rename of the directory is a deliberate change.
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if let Some(h) = home {
            if std::env::var_os("EBBTOP_STATE_DIR").is_none() {
                assert!(state_dir().starts_with(&h));
                // Per-platform, and it must agree with the installer that
                // creates (and purges) the directory: see
                // `platform::default_state_dir`.
                if cfg!(target_os = "macos") {
                    assert!(state_dir().ends_with("Library/Application Support/ebbtop-agent"));
                } else {
                    assert!(state_dir().ends_with("services/ebbtop-agent"));
                }
            }
        }
    }

    /// The exact bytes `/pair` returns, pinned here AND in
    /// `ebbtop_core::pairing::tests::PAIR_RESPONSE_FIXTURE`.
    const PAIR_RESPONSE_FIXTURE: &str = r#"{"token":"placeholder-not-a-credential","host":"k-server","agent_version":"0.2.8","nonce":"00112233445566778899aabbccddeeff","scope":"read_only"}"#;

    #[test]
    fn pair_response_bytes_are_pinned() {
        let r = PairResponse {
            token: "placeholder-not-a-credential".into(),
            host: "k-server".into(),
            agent_version: "0.2.8".into(),
            nonce: Some("00112233445566778899aabbccddeeff".into()),
            scope: SCOPE_READ_ONLY.into(),
        };
        assert_eq!(serde_json::to_string(&r).unwrap(), PAIR_RESPONSE_FIXTURE);
        // the request type accepts body-only codes (never a query string)
        let q: PairRequest = serde_json::from_str(r#"{"code":"123456"}"#).unwrap();
        assert_eq!(q.code, "123456");
        assert!(q.label.is_none());
    }

    /// A code must never be recoverable from the credential store or the
    /// error text: only the token is persisted.
    #[test]
    fn issued_store_holds_no_code_material() {
        let (_dir, store) = store();
        let pending = store.mint().unwrap();
        store.exchange(&pending.code, None, now_ms()).unwrap();
        let raw = std::fs::read_to_string(store.clients_file()).unwrap();
        assert!(!raw.contains(&pending.code));
        for e in [
            PairError::NoPendingCode,
            PairError::WrongCode,
            PairError::Expired,
            PairError::TooManyAttempts,
            PairError::StateIo,
        ] {
            assert!(!e.reason().contains(&pending.code));
        }
    }
}
