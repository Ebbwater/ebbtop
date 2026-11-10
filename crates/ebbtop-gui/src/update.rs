//! Background release check + update dialog state (spec: hot-updater design, section 1).
//!
//! The check runs on a plain thread (never the UI thread, never blocking
//! startup): `GET .../releases/latest`, `tag_name` out of the JSON body,
//! semver-compare against this build's `CARGO_PKG_VERSION`. A failed check
//! is a quiet status-bar note + log line; the next check runs at the next
//! interval. Accepting records the approved version; `download_verified`
//! streams the asset to `<dest>.tmp`, hash-checks it against `SHA256SUMS`,
//! and renames on success. Apply and restart are later tasks.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use ebbtop_core::update::is_update_available;

/// GitHub API endpoint for the newest stable release (`/latest` already
/// excludes drafts and prereleases, so no extra filtering is needed).
pub const RELEASES_LATEST_URL: &str = "https://api.github.com/repos/Ebbwater/ebbtop/releases/latest";

/// Automatic re-check cadence from the spec.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 3600);

/// This build's version (`workspace 0.2.0` today).
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// `tag_name` out of a `/releases/latest` body. `None` when the body is not
/// JSON, carries no string `tag_name`, or the tag is blank.
pub fn parse_latest_tag(body: &str) -> Option<String> {
    let tag = serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("tag_name")?
        .as_str()?
        .trim()
        .to_string();
    (!tag.is_empty()).then_some(tag)
}

/// Outcome of one finished check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    Available { version: String },
    UpToDate { version: String },
    Failed(String),
}

/// Decide the outcome of a check from this build's version and the fetch
/// result (body text or transport error).
pub fn decide(current: &str, fetched: Result<String, String>) -> CheckOutcome {
    match fetched {
        Err(e) => CheckOutcome::Failed(e),
        Ok(body) => match parse_latest_tag(&body) {
            None => CheckOutcome::Failed("unparseable release body".into()),
            Some(tag) if is_update_available(current, &tag) => CheckOutcome::Available { version: tag },
            Some(tag) => CheckOutcome::UpToDate { version: tag },
        },
    }
}

/// Dialog-level status owned by the GUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    Available { version: String },
    UpToDate { version: String },
    Failed(String),
    /// User approved this version; the download task consumes it.
    Accepted { version: String },
}

/// Background check plus prompt bookkeeping. The fetch runs on a spawned
/// thread and reports back over a channel; `poll()` drains it without
/// blocking. `fetch` is injected so tests never touch the network.
pub struct UpdateCheck {
    status: UpdateStatus,
    last_check: Option<Instant>,
    interval: Duration,
    rx: Option<mpsc::Receiver<CheckOutcome>>,
    /// Version already prompted for: decline re-prompts only for a newer tag.
    notified: Option<String>,
}

impl UpdateCheck {
    pub fn new() -> Self {
        Self::with_interval(CHECK_INTERVAL)
    }

    pub fn with_interval(interval: Duration) -> Self {
        Self { status: UpdateStatus::Idle, last_check: None, interval, rx: None, notified: None }
    }

    pub fn status(&self) -> &UpdateStatus {
        &self.status
    }

    /// True when no check is running and the last one is older than the
    /// interval (or never ran).
    pub fn due(&self) -> bool {
        if self.rx.is_some() {
            return false;
        }
        match self.last_check {
            None => true,
            Some(t) => t.elapsed() >= self.interval,
        }
    }

    /// Spawn the check thread. No-op while one is already running. The
    /// thread compares against this build's version and posts the outcome;
    /// the caller never blocks.
    pub fn start(&mut self, fetch: impl FnOnce() -> Result<String, String> + Send + 'static) {
        if self.rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.status = UpdateStatus::Checking;
        let current = current_version().to_string();
        let spawned = std::thread::Builder::new()
            .name("ebbtop-update-check".into())
            .spawn(move || {
                let _ = tx.send(decide(&current, fetch()));
            });
        if spawned.is_err() {
            // Practically unreachable; park the retry at the next interval
            // instead of hot-looping a spawn failure every frame.
            self.rx = None;
            self.last_check = Some(Instant::now());
            self.status = UpdateStatus::Failed("could not start check thread".into());
        }
    }

    /// Drain a finished check into `status`. Returns true on a transition.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        match rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.rx = None;
                self.last_check = Some(Instant::now());
                self.status = UpdateStatus::Failed("check thread ended".into());
                true
            }
            Ok(outcome) => {
                self.rx = None;
                self.last_check = Some(Instant::now());
                self.status = match outcome {
                    CheckOutcome::Available { version } => UpdateStatus::Available { version },
                    CheckOutcome::UpToDate { version } => UpdateStatus::UpToDate { version },
                    CheckOutcome::Failed(e) => UpdateStatus::Failed(e),
                };
                true
            }
        }
    }

    /// True when the dialog should pop: an unprompted available version.
    pub fn should_prompt(&self) -> bool {
        match &self.status {
            UpdateStatus::Available { version } => self.notified.as_ref() != Some(version),
            _ => false,
        }
    }

    /// The dialog popped for the current available version.
    pub fn mark_notified(&mut self) {
        if let UpdateStatus::Available { version } = &self.status {
            self.notified = Some(version.clone());
        }
    }

    /// User approved the available version; `download_verified` consumes it.
    pub fn accept(&mut self) {
        if let UpdateStatus::Available { version } = &self.status {
            self.status = UpdateStatus::Accepted { version: version.clone() };
        }
    }

    /// User declined the available version; stays quiet until a newer tag.
    /// Status keeps the version so the Updates tab can still offer it.
    pub fn decline(&mut self) {
        self.mark_notified();
    }
}

impl Default for UpdateCheck {
    fn default() -> Self {
        Self::new()
    }
}

/// Default fetch: body text of `RELEASES_LATEST_URL`, or a transport error.
/// Plain thread + blocking client; never called on the UI thread. GitHub's
/// API requires a User-Agent; without one it answers 403.
pub fn fetch_latest_release() -> Result<String, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(format!("ebbtop/{}", current_version()))
        .build()
        .map_err(|e| e.to_string())?
        .get(RELEASES_LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .map_err(|e| e.to_string())
}

/// Sibling temp path for a verified download: `<dest>.tmp` in the same
/// directory, so the final rename is atomic.
fn download_tmp_path(dest: &std::path::Path) -> std::path::PathBuf {
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".tmp");
    std::path::PathBuf::from(tmp)
}

/// URL with any `?query` dropped. Update artifacts are public, but callers
/// may reuse token-carrying URLs; errors and logs must never carry them.
fn scrub_url(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// Blocking download client shared by the verified-download path. Plain
/// thread use only, never the UI thread.
fn download_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(format!("ebbtop/{}", current_version()))
        .build()
        .map_err(|e| e.to_string())
}

/// Download the release asset at `url`, verify its SHA-256 against the
/// `asset` line of the `SHA256SUMS` text at `sums_url`, and move it to
/// `dest` (spec design, section 2).
///
/// Bytes stream to `<dest>.tmp` with cumulative byte counts on `progress`
/// (a dropped receiver is not an error); the hash is computed over the
/// stream and compared before anything touches `dest`. Any failure —
/// sums fetch, missing sums line, transport abort, hash mismatch, rename —
/// deletes the partial and returns an error that never contains a URL
/// query (where tokens travel).
pub fn download_verified(
    url: &str,
    sums_url: &str,
    asset: &str,
    dest: &std::path::Path,
    progress: &mpsc::Sender<u64>,
) -> Result<(), String> {
    let tmp = download_tmp_path(dest);
    let result = download_verified_inner(url, sums_url, asset, &tmp, progress);
    match result {
        Ok(()) => std::fs::rename(&tmp, dest)
            .map_err(|e| format!("download {}: {e}", scrub_url(url))),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn download_verified_inner(
    url: &str,
    sums_url: &str,
    asset: &str,
    tmp: &std::path::Path,
    progress: &mpsc::Sender<u64>,
) -> Result<(), String> {
    use ebbtop_core::update::{find_hash_for_asset, hashes_match};
    use sha2::Digest;
    use std::io::{Read, Write};

    let client = download_client()?;
    let sums = client
        .get(sums_url)
        .send()
        .map_err(|e| format!("sums fetch {}: {e}", scrub_url(sums_url)))?
        .error_for_status()
        .map_err(|e| format!("sums fetch {}: {e}", scrub_url(sums_url)))?
        .text()
        .map_err(|e| format!("sums fetch {}: {e}", scrub_url(sums_url)))?;
    let expected = find_hash_for_asset(&sums, asset).ok_or_else(|| {
        format!("no hash for {asset} in {}", scrub_url(sums_url))
    })?;

    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| format!("download {}: {e}", scrub_url(url)))?
        .error_for_status()
        .map_err(|e| format!("download {}: {e}", scrub_url(url)))?;
    let mut file =
        std::fs::File::create(tmp).map_err(|e| format!("download {}: {e}", scrub_url(url)))?;
    let mut hasher = sha2::Sha256::new();
    let mut done: u64 = 0;
    let mut chunk = [0u8; 65536];
    loop {
        let n = resp
            .read(&mut chunk)
            .map_err(|e| format!("download {}: {e}", scrub_url(url)))?;
        if n == 0 {
            break;
        }
        file.write_all(&chunk[..n])
            .map_err(|e| format!("download {}: {e}", scrub_url(url)))?;
        hasher.update(&chunk[..n]);
        done += n as u64;
        let _ = progress.send(done);
    }
    file.flush()
        .map_err(|e| format!("download {}: {e}", scrub_url(url)))?;
    drop(file);
    let digest = hasher.finalize();
    let mut actual = String::with_capacity(64);
    for b in digest.iter() {
        actual.push_str(&format!("{b:02x}"));
    }
    if !hashes_match(&expected, &actual) {
        return Err(format!("hash mismatch for {asset} from {}", scrub_url(url)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `poll()` is non-blocking by design (the UI calls it every frame), so
    /// give the spawned check thread a moment to report before asserting.
    fn poll_settled(c: &mut UpdateCheck) -> bool {
        for _ in 0..500 {
            if c.poll() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn latest_tag_parses_tag_name() {
        let body = r#"{"tag_name": "v0.3.0", "name": "EbbTop 0.3.0"}"#;
        assert_eq!(parse_latest_tag(body).as_deref(), Some("v0.3.0"));
    }

    #[test]
    fn latest_tag_rejects_missing_or_blank() {
        assert_eq!(parse_latest_tag(r#"{"name": "no tag"}"#), None);
        assert_eq!(parse_latest_tag("not json at all"), None);
        assert_eq!(parse_latest_tag(r#"{"tag_name": "  "}"#), None);
        assert_eq!(parse_latest_tag(r#"{"tag_name": 42}"#), None);
    }

    #[test]
    fn decide_available_only_when_newer() {
        assert_eq!(
            decide("0.2.0", Ok(r#"{"tag_name": "v0.3.0"}"#.into())),
            CheckOutcome::Available { version: "v0.3.0".into() }
        );
        assert_eq!(
            decide("0.3.0", Ok(r#"{"tag_name": "v0.3.0"}"#.into())),
            CheckOutcome::UpToDate { version: "v0.3.0".into() }
        );
        assert_eq!(
            decide("0.4.0", Ok(r#"{"tag_name": "v0.3.0"}"#.into())),
            CheckOutcome::UpToDate { version: "v0.3.0".into() }
        );
        assert_eq!(
            decide("0.2.0", Ok("garbage".into())),
            CheckOutcome::Failed("unparseable release body".into())
        );
        assert_eq!(
            decide("0.2.0", Err("dns down".into())),
            CheckOutcome::Failed("dns down".into())
        );
    }

    #[test]
    fn check_is_due_until_interval_passes() {
        let mut c = UpdateCheck::with_interval(Duration::from_secs(60));
        assert!(c.due(), "never checked: due");
        c.start(|| Ok(r#"{"tag_name": "v99.9.9"}"#.into()));
        assert!(!c.due(), "running check: not due");
        assert!(poll_settled(&mut c), "finished check transitions");
        assert_eq!(c.status(), &UpdateStatus::Available { version: "v99.9.9".into() });
        assert!(!c.due(), "just checked: not due");
    }

    #[test]
    fn failed_check_still_retries_next_interval() {
        let mut c = UpdateCheck::with_interval(Duration::from_secs(60));
        c.start(|| Err("offline".into()));
        assert!(poll_settled(&mut c), "finished check transitions");
        assert_eq!(c.status(), &UpdateStatus::Failed("offline".into()));
    }

    #[test]
    fn prompt_once_per_version_until_newer() {
        let mut c = UpdateCheck::with_interval(Duration::from_secs(60));
        c.start(|| Ok(r#"{"tag_name": "v0.3.0"}"#.into()));
        assert!(poll_settled(&mut c));
        assert!(c.should_prompt(), "fresh available version prompts");
        c.mark_notified();
        assert!(!c.should_prompt(), "prompted version stays quiet");
        c.decline();
        assert!(!c.should_prompt(), "declined version stays quiet");
        // A newer tag re-prompts even after a decline.
        c.start(|| Ok(r#"{"tag_name": "v0.4.0"}"#.into()));
        assert!(poll_settled(&mut c));
        assert!(c.should_prompt(), "newer version re-prompts");
    }

    #[test]
    fn accept_records_version_for_download() {
        let mut c = UpdateCheck::with_interval(Duration::from_secs(60));
        c.start(|| Ok(r#"{"tag_name": "v99.1.0"}"#.into()));
        assert!(poll_settled(&mut c));
        c.accept();
        assert_eq!(c.status(), &UpdateStatus::Accepted { version: "v99.1.0".into() });
        assert!(!c.should_prompt(), "accepted version does not prompt");
    }

    // --- Verified download (spec section 2) ---

    /// Body bytes for the download fixtures. The expected digest below is
    /// the well-known SHA-256 of `"abc"`, independent of this crate.
    const FIXTURE_BYTES: &[u8] = b"abc";
    const FIXTURE_SHA256: &str =
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    /// Minimal loopback HTTP server for download tests: `routes` maps a
    /// request path to its canned body. When `cut` names a path, that path
    /// instead advertises the full length but sends only `cut.1` bytes and
    /// drops the connection (mid-stream abort). Serves `hits` requests on a
    /// spawned thread, then exits. Query strings are ignored when matching.
    fn loopback_server(
        routes: std::collections::HashMap<String, Vec<u8>>,
        cut: Option<(String, usize)>,
        hits: usize,
    ) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("loopback addr");
        std::thread::spawn(move || {
            for _ in 0..hits {
                let Ok((mut stream, _)) = listener.accept() else { break };
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let raw = head.split_whitespace().nth(1).unwrap_or("/");
                let path = raw.split('?').next().unwrap_or("/").to_string();
                if let Some((ref cut_path, at)) = cut
                    && *cut_path == path
                {
                    let full = routes.get(&path).cloned().unwrap_or_default();
                    let end = at.min(full.len());
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        full.len()
                    );
                    let _ = stream.write_all(&full[..end]);
                    let _ = stream.flush();
                    // Drop the connection mid-body: truncation, not a page.
                    continue;
                }
                match routes.get(&path) {
                    Some(body) => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(body);
                    }
                    None => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                }
            }
        });
        format!("http://{addr}")
    }

    /// Fresh scratch dir per test so parallel tests never share a file.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ebbtop-update-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// Entries (file names) currently in `dir`.
    fn dir_entries(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("read scratch dir")
            .map(|e| e.expect("dir entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn download_verifies_good_file() {
        let mut routes = std::collections::HashMap::new();
        routes.insert("/ebbtop.exe".to_string(), FIXTURE_BYTES.to_vec());
        routes.insert(
            "/SHA256SUMS".to_string(),
            format!("{FIXTURE_SHA256}  ebbtop.exe\n").into_bytes(),
        );
        let base = loopback_server(routes, None, 2);
        let dir = scratch_dir("good");
        let dest = dir.join("ebbtop.exe");
        let (tx, rx) = mpsc::channel();
        download_verified(
            &format!("{base}/ebbtop.exe"),
            &format!("{base}/SHA256SUMS"),
            "ebbtop.exe",
            &dest,
            &tx,
        )
        .expect("good download verifies");
        assert_eq!(std::fs::read(&dest).expect("read back download"), FIXTURE_BYTES);
        assert_eq!(dir_entries(&dir), vec!["ebbtop.exe"], "no temp left behind");
        let progress: Vec<u64> = rx.try_iter().collect();
        assert!(!progress.is_empty(), "progress reported during download");
        assert_eq!(*progress.last().unwrap(), FIXTURE_BYTES.len() as u64);
    }

    #[test]
    fn download_rejects_bad_hash() {
        let mut routes = std::collections::HashMap::new();
        routes.insert("/ebbtop.exe".to_string(), FIXTURE_BYTES.to_vec());
        routes.insert(
            "/SHA256SUMS".to_string(),
            format!("{}  ebbtop.exe\n", "0".repeat(64)).into_bytes(),
        );
        let base = loopback_server(routes, None, 2);
        let dir = scratch_dir("badsums");
        let dest = dir.join("ebbtop.exe");
        let (tx, _rx) = mpsc::channel();
        // A token-looking query rides along to prove errors never carry it.
        let err = download_verified(
            &format!("{base}/ebbtop.exe?token=s3cr3t"),
            &format!("{base}/SHA256SUMS?token=s3cr3t"),
            "ebbtop.exe",
            &dest,
            &tx,
        )
        .expect_err("tampered sums must fail");
        assert!(!err.contains("s3cr3t"), "error leaked token: {err}");
        assert!(dir_entries(&dir).is_empty(), "no file or partial left behind");
    }

    #[test]
    fn download_abort_removes_partial() {
        let mut routes = std::collections::HashMap::new();
        routes.insert("/ebbtop.exe".to_string(), FIXTURE_BYTES.to_vec());
        routes.insert(
            "/SHA256SUMS".to_string(),
            format!("{FIXTURE_SHA256}  ebbtop.exe\n").into_bytes(),
        );
        // Sums fetch succeeds (1 hit); the asset stream drops after 1 byte.
        let base = loopback_server(routes, Some(("/ebbtop.exe".to_string(), 1)), 2);
        let dir = scratch_dir("abort");
        let dest = dir.join("ebbtop.exe");
        let (tx, _rx) = mpsc::channel();
        download_verified(
            &format!("{base}/ebbtop.exe"),
            &format!("{base}/SHA256SUMS"),
            "ebbtop.exe",
            &dest,
            &tx,
        )
        .expect_err("truncated download must fail");
        assert!(dir_entries(&dir).is_empty(), "partial download removed");
    }
}
