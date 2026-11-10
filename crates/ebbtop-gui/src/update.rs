//! Background release check + update dialog state (spec: hot-updater design, section 1).
//!
//! The check runs on a plain thread (never the UI thread, never blocking
//! startup): `GET .../releases/latest`, `tag_name` out of the JSON body,
//! semver-compare against this build's `CARGO_PKG_VERSION`. A failed check
//! is a quiet status-bar note + log line; the next check runs at the next
//! interval. Download, verify, apply, and restart are later tasks; accepting
//! here only records the approved version for them to consume.

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

    /// User approved the available version (download is a later task).
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
}
