//! Non-secret config in a JSON file; token in the OS credential store
//! (Windows Credential Manager, macOS Keychain, Secret Service on Linux).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Fresh installs start with no sources (onboarding adds the first). This
/// URL only identifies a legacy single-host config during migration.
pub const LEGACY_DEFAULT_URL: &str = "ws://127.0.0.1:45976/ws";
/// First-generation default (pre-scrub private builds). Recognized only so
/// those installs keep their historic source name and keyring entry.
const LEGACY_DEFAULT_URL_V1: &str = "ws://100.99.193.22:45976/ws";
const KEYRING_SERVICE: &str = "ebbtop";

fn default_true() -> bool {
    true
}

fn default_top_frac() -> f32 {
    0.55
}

/// UI language default: English. Stored as a canonical id string so a future
/// build can add locales without a schema change; any value this build does
/// not know resolves to English (see `l10n::Locale::from_config`).
fn default_language() -> String {
    "en".to_string()
}

/// One monitored host. Each source keeps its own token in the OS
/// credential store under its name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Source {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub sources: Vec<Source>,
    /// Index into `sources`. Clamped on load; meaningless while empty.
    pub active: usize,
    /// History window shown in graphs, seconds (60 or 300).
    pub window_s: u32,
    pub iface: Option<String>,
    /// Pinned disk for the disk I/O graphs. None follows the busiest disk.
    pub disk: Option<String>,
    /// Phase 1 UI: tighter spacing for half-screen side-by-side use.
    pub compact: bool,
    /// Phase 1 UI: log scale for network graphs (off = linear).
    pub log_scale: bool,
    /// Phase 2 UI: fraction of central height given to the top boxes.
    #[serde(default = "default_top_frac")]
    pub top_frac: f32,
    /// Phase 2 UI: per-panel visibility (all on by default).
    #[serde(default = "default_true")]
    pub show_cpu: bool,
    #[serde(default = "default_true")]
    pub show_gpu: bool,
    #[serde(default = "default_true")]
    pub show_mem: bool,
    #[serde(default = "default_true")]
    pub show_net: bool,
    #[serde(default = "default_true")]
    pub show_disk: bool,
    /// Collapse state per panel (false = open). Skips content draw, so
    /// frame cost drops and siblings reflow automatically.
    #[serde(default)]
    pub collapsed_cpu: bool,
    #[serde(default)]
    pub collapsed_gpu: bool,
    #[serde(default)]
    pub collapsed_mem: bool,
    #[serde(default)]
    pub collapsed_net: bool,
    #[serde(default)]
    pub collapsed_disk: bool,
    #[serde(default)]
    pub collapsed_proc: bool,
    /// Collapse for the per-process detail pane (shown only while a row is
    /// selected, so this is collapse only, no show flag).
    #[serde(default)]
    pub collapsed_proc_detail: bool,
    /// Collapse for the workload roll-up pane above the process table.
    #[serde(default)]
    pub collapsed_workloads: bool,
    /// Last finished release check, unix millis (0 = never). Persists the
    /// 24 h check cadence across restarts.
    #[serde(default)]
    pub update_last_check_ms: u64,
    /// Release tag the user skipped ("Later"); that tag never re-prompts,
    /// a newer one still does. Empty means none skipped.
    #[serde(default)]
    pub update_skipped: String,
    /// UI language, as a canonical id: `en` (default), `zh-Hans-CN` or
    /// `zh-Hant-TW`. Missing in old files and unknown values both resolve to
    /// English; the raw string is kept as written so a hand edit or a value
    /// from a newer build is never rewritten behind the user's back.
    #[serde(default = "default_language")]
    pub language: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            active: 0,
            window_s: 60,
            iface: None,
            disk: None,
            compact: false,
            log_scale: false,
            top_frac: default_top_frac(),
            show_cpu: true,
            show_gpu: true,
            show_mem: true,
            show_net: true,
            show_disk: true,
            collapsed_cpu: false,
            collapsed_gpu: false,
            collapsed_mem: false,
            collapsed_net: false,
            collapsed_disk: false,
            collapsed_proc: false,
            collapsed_proc_detail: false,
            collapsed_workloads: false,
            update_last_check_ms: 0,
            update_skipped: String::new(),
            language: default_language(),
        }
    }
}

pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ebbtop").join("config.json"))
}

/// Load the config file if it parses, else a fresh default.
/// A file that reads but does not parse (hand-edit typo) falls through;
/// a valid file with zero sources stays authoritative. Split from
/// `load_config` so tests can point it at temp files.
fn load_config_from(primary: Option<PathBuf>) -> Config {
    if let Some(p) = primary
        && let Ok(s) = std::fs::read_to_string(&p)
        && serde_json::from_str::<Config>(&s).is_ok()
    {
        return parse_config(&s);
    }
    Config::default()
}

impl Config {
    /// Currently selected source, if any.
    pub fn active_source(&self) -> Option<&Source> {
        self.sources.get(self.active)
    }

    /// Resolved UI language. Unknown, blank or missing values are English.
    pub fn locale(&self) -> crate::l10n::Locale {
        crate::l10n::Locale::from_config(&self.language)
    }

    fn clamp_active(&mut self) {
        if self.sources.is_empty() {
            self.active = 0;
        } else {
            self.active = self.active.min(self.sources.len() - 1);
        }
    }
}

/// Parse one config file. A legacy single-`url` file (no `sources`)
/// migrates to one source, keeping every display field; anything
/// unparseable is a fresh default.
pub fn parse_config(s: &str) -> Config {
    let mut cfg: Config = serde_json::from_str(s).unwrap_or_default();
    if cfg.sources.is_empty()
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(s)
        && let Some(u) = v
            .get("url")
            .and_then(|u| u.as_str())
            .filter(|u| !u.is_empty())
    {
        cfg.sources = vec![Source {
            name: name_from_url(u),
            url: u.to_string(),
        }];
    }
    cfg.clamp_active();
    cfg
}

/// Display name for a migrated legacy URL: the old default keeps its
/// historic name, anything else takes the URL host.
pub fn name_from_url(u: &str) -> String {
    if u == LEGACY_DEFAULT_URL || u == LEGACY_DEFAULT_URL_V1 {
        return "k-server".to_string();
    }
    let host = u
        .split("://")
        .nth(1)
        .unwrap_or(u)
        .split('/')
        .next()
        .unwrap_or("")
        .split('@')
        .next_back()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    if host.is_empty() {
        "server".to_string()
    } else {
        host.to_string()
    }
}

pub fn load_config() -> Config {
    load_config_from(active_config_path())
}

/// Path the app reads and writes. Tests point this at a temp file
/// (`set_test_config`) so a frame that persists a toggle never edits the
/// operator's own `config.json`.
fn active_config_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(p) = TEST_CONFIG.with(|c| c.borrow().clone()) {
        return Some(p);
    }
    config_path()
}

pub fn save_config(c: &Config) -> anyhow::Result<()> {
    let p = active_config_path().ok_or_else(|| anyhow::anyhow!("no config dir"))?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(c)?)?;
    std::fs::rename(tmp, p)?;
    Ok(())
}

/// Where the token came from, for the settings screen. Never the value.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenSource {
    Env,
    Keyring,
    Missing,
    Error(String),
}

/// `EBBTOP_TOKEN` overrides the store (development and probes).
pub fn load_token_for(user: &str) -> (Option<String>, TokenSource) {
    if let Ok(t) = std::env::var("EBBTOP_TOKEN")
        && !t.trim().is_empty()
    {
        return (Some(t.trim().to_string()), TokenSource::Env);
    }
    #[cfg(test)]
    if let Some(sink) = test_credentials() {
        // The suite never talks to a real credential store (that needs a
        // desktop session); it uses the in-memory sink the test installed.
        let found = sink
            .borrow()
            .iter()
            .rev()
            .find(|(u, _)| u == user)
            .map(|(_, t)| t.clone());
        return match found {
            Some(t) => (Some(t), TokenSource::Keyring),
            None => (None, TokenSource::Missing),
        };
    }
    match keyring::Entry::new(KEYRING_SERVICE, user).and_then(|e| e.get_password()) {
        Ok(t) if !t.is_empty() => (Some(t), TokenSource::Keyring),
        Ok(_) | Err(keyring::Error::NoEntry) => (None, TokenSource::Missing),
        Err(e) => (None, TokenSource::Error(e.to_string())),
    }
}

pub fn save_token_for(user: &str, token: &str) -> anyhow::Result<()> {
    #[cfg(test)]
    if let Some(sink) = test_credentials() {
        sink.borrow_mut()
            .push((user.to_string(), token.to_string()));
        return Ok(());
    }
    keyring::Entry::new(KEYRING_SERVICE, user)?.set_password(token)?;
    Ok(())
}

/// Drop a source's token. Best effort: a missing entry is not an error.
/// Called when a source is removed so its secret does not linger.
pub fn delete_token_for(user: &str) {
    #[cfg(test)]
    if let Some(sink) = test_credentials() {
        sink.borrow_mut().retain(|(u, _)| u != user);
        return;
    }
    let _ = keyring::Entry::new(KEYRING_SERVICE, user).and_then(|e| e.delete_credential());
}

/// In-memory credential sink for tests: `(user, token)` pairs, newest last.
/// Mirrors the real store's one-entry-per-user shape closely enough to prove
/// the pairing path writes the issued token under the source name.
#[cfg(test)]
pub type TestCredentials = std::rc::Rc<std::cell::RefCell<Vec<(String, String)>>>;

#[cfg(test)]
thread_local! {
    static TEST_CREDS: std::cell::RefCell<Option<TestCredentials>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    static TEST_CONFIG: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Test-only: read and write config here instead of the real config dir.
#[cfg(test)]
pub fn set_test_config(p: Option<PathBuf>) {
    TEST_CONFIG.with(|c| *c.borrow_mut() = p);
}

/// Test-only: replace the credential store for this test thread. Callers
/// remove the sink (pass `None`) when the test is done.
#[cfg(test)]
pub fn set_test_credentials(sink: Option<TestCredentials>) {
    TEST_CREDS.with(|c| *c.borrow_mut() = sink);
}

#[cfg(test)]
fn test_credentials() -> Option<TestCredentials> {
    TEST_CREDS.with(|c| c.borrow().clone())
}

pub fn log_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ebbtop").join("ebbtop.log"))
}

/// The support summary is written beside the log so a user can attach it.
pub fn support_copy_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ebbtop").join("ebbtop-support.txt"))
}

/// OS command that reveals the log file. Pure, so a test can assert the
/// command without launching a file manager.
pub fn opener_command(target: &std::path::Path) -> (String, Vec<String>) {
    let t = target.display().to_string();
    #[cfg(target_os = "windows")]
    {
        ("explorer".to_string(), vec![format!("/select,{t}")])
    }
    #[cfg(target_os = "macos")]
    {
        ("open".to_string(), vec!["-R".to_string(), t])
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = t;
        // xdg-open on the folder: one file has no default handler on a bare
        // desktop, the folder always opens.
        let dir = target.parent().unwrap_or(target).display().to_string();
        ("xdg-open".to_string(), vec![dir])
    }
}

/// Why revealing the log failed, as a value the draw layer can localize.
/// The variants carry only non-localized facts (the handler name and the OS
/// error text), so nothing translated can leak into a log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenLogsError {
    /// No data directory is resolvable on this platform.
    NoDataDir,
    /// The OS handler could not be started.
    Spawn { cmd: String, error: String },
}

/// Reveal the log with the OS handler. Best effort: the caller shows the
/// error as a note. Never fails the app.
pub fn open_logs() -> Result<(), OpenLogsError> {
    let p = log_path().ok_or(OpenLogsError::NoDataDir)?;
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let (cmd, args) = opener_command(&p);
    std::process::Command::new(&cmd)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| OpenLogsError::Spawn {
            cmd,
            error: e.to_string(),
        })
}

/// Write the support summary beside the log. Same no-secret rule as the
/// summary itself: the text is already scrubbed by `support_summary`.
pub fn write_support(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, text)
}

/// Endpoint shape for a support summary: scheme, port class and path only.
/// The host, any user info and the whole query string are dropped, so a
/// `?token=` URL cannot survive.
pub fn endpoint_shape(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("ws", url));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let port = authority
        .rsplit_once(':')
        .map(|(_, p)| p.trim())
        .filter(|p| !p.is_empty());
    let path = rest
        .split_once('/')
        .map(|(_, p)| format!("/{}", p.split(['?', '#']).next().unwrap_or("")))
        .unwrap_or_else(|| "/ws".to_string());
    let port_desc = match port {
        Some("45976") | None => "default port",
        Some(_) => "custom port",
    };
    format!("{scheme}, {port_desc}, {path}")
}

pub fn log_bytes() -> Option<u64> {
    log_path()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
}

/// Facts a support summary may contain. Every field is chosen to be
/// non-secret and non-identifying: no host, address, user name, path or
/// token, per E4 and the release rule on support exports.
pub struct SupportFacts<'a> {
    pub version: &'a str,
    pub platform: &'a str,
    pub sources: usize,
    /// From [`endpoint_shape`]: scheme, port class, path.
    pub endpoint: &'a str,
    pub token_source: &'a TokenSource,
    /// LIVE / STALE / CONNECTING / DOWN.
    pub connection: &'a str,
    /// Headline and scrubbed detail of the last failed Test, if any.
    pub failure: Option<(&'a str, &'a str)>,
    pub log_bytes: Option<u64>,
    pub config_ok: bool,
}

/// Narrow support summary: version, platform, endpoint shape, credential
/// source, connection state and the classified failure. `token` is used
/// only to scrub an accidental appearance; it is never emitted.
pub fn support_summary(f: &SupportFacts<'_>, token: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str("EbbTop support summary\n");
    out.push_str(&format!("version: {}\n", f.version));
    out.push_str(&format!("platform: {}\n", f.platform));
    out.push_str(&format!("sources: {}\n", f.sources));
    out.push_str(&format!("endpoint: {}\n", f.endpoint));
    out.push_str(&format!(
        "token: {}\n",
        match f.token_source {
            TokenSource::Env => "from environment variable",
            TokenSource::Keyring => "in OS credential store",
            TokenSource::Missing => "none saved",
            // The credential store error text is local and token-free, but
            // keep it short: no detail is needed to help.
            TokenSource::Error(_) => "credential store error (details omitted)",
        }
    ));
    out.push_str(&format!("connection: {}\n", f.connection));
    match f.failure {
        Some((headline, detail)) => {
            out.push_str(&format!("last failure: {headline}\n"));
            out.push_str(&format!("detail: {detail}\n"));
        }
        None => out.push_str("last failure: none\n"),
    }
    match f.log_bytes {
        Some(n) => out.push_str(&format!("log: ebbtop.log present ({} KiB)\n", n / 1024)),
        None => out.push_str("log: ebbtop.log not written yet\n"),
    }
    out.push_str(&format!(
        "config: {}\n",
        if f.config_ok {
            "present"
        } else {
            "not written yet"
        }
    ));
    out.push_str("note: no host, address, path, token or token-bearing URL is included.\n");
    scrub_urls(&scrub_secret(&out, token))
}

/// Replace the address part of every URL with `<endpoint>`. Error chains can
/// quote the connect URL; a support summary must not carry a host or a
/// token-bearing query, so the whole authority and query go.
pub fn scrub_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let scheme_start = rest[..i]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.'))
            .map(|j| j + 1)
            .unwrap_or(0);
        out.push_str(&rest[..scheme_start]);
        out.push_str(&rest[scheme_start..i + 3]);
        out.push_str("<endpoint>");
        let after = &rest[i + 3..];
        let end = after
            .find(|c: char| c.is_whitespace() || c == ')' || c == '"' || c == '\'')
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Belt and braces for anything that leaves the app: the token value and
/// any `token=` pair become `<token>`. Idempotent.
pub fn scrub_secret(text: &str, token: Option<&str>) -> String {
    let once = ebbtop_core::client::redact(text.to_string(), token);
    ebbtop_core::client::scrub(&once)
}

const LOG_MAX: u64 = 2 * 1024 * 1024;

// Path the app appends to. Unit tests point this at a temp file so the
// suite never writes into the operator's real log.
#[cfg(test)]
thread_local! {
    static TEST_LOG: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Test-only: redirect `append_log` for this test thread.
#[cfg(test)]
pub fn set_test_log(p: Option<PathBuf>) {
    TEST_LOG.with(|c| *c.borrow_mut() = p);
}

fn active_log_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        // Tests opt in with `set_test_log`: the suite must never append to
        // the operator's real log.
        TEST_LOG.with(|c| c.borrow().clone())
    }
    #[cfg(not(test))]
    {
        log_path()
    }
}

/// Append one timestamped line. Rotates to ebbtop.log.1 at 2 MiB. Never
/// fails the caller. Callers must not pass secrets.
pub fn append_log(line: &str) {
    let Some(p) = active_log_path() else { return };
    append_log_to(&p, line);
}

/// The writer itself, split out so tests exercise the real append and
/// rotation on a temp file.
pub fn append_log_to(p: &std::path::Path, line: &str) {
    use std::io::Write;
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if std::fs::metadata(p)
        .map(|m| m.len() > LOG_MAX)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(p, p.with_extension("log.1"));
    }
    let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%z");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let _ = writeln!(f, "{ts} {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_config_gets_phase2_defaults() {
        // Config file written before compact/log/top_frac/show_*/collapsed_* existed.
        let json = r#"{"url":"ws://127.0.0.1:45976/ws","window_s":300,"iface":null}"#;
        let c = parse_config(json);
        assert_eq!(c.sources.len(), 1);
        assert_eq!(c.sources[0].url, "ws://127.0.0.1:45976/ws");
        assert_eq!(c.window_s, 300);
        assert!(!c.compact && !c.log_scale);
        assert!((c.top_frac - 0.55).abs() < 1e-6, "top_frac {}", c.top_frac);
        assert!(c.show_cpu && c.show_gpu && c.show_mem && c.show_net && c.show_disk);
        assert!(
            !c.collapsed_cpu
                && !c.collapsed_gpu
                && !c.collapsed_mem
                && !c.collapsed_net
                && !c.collapsed_disk
                && !c.collapsed_proc
        );
    }

    #[test]
    fn fresh_default_has_no_sources() {
        let c = Config::default();
        assert!(c.sources.is_empty());
        assert_eq!(c.active, 0);
        assert!(c.active_source().is_none());
        assert!(c.disk.is_none());
    }

    #[test]
    fn legacy_url_migrates_to_single_source() {
        let c = parse_config(r#"{"url":"ws://192.168.1.10:45976/ws","window_s":300,"iface":null}"#);
        assert_eq!(c.sources.len(), 1);
        assert_eq!(c.sources[0].url, "ws://192.168.1.10:45976/ws");
        assert_eq!(c.sources[0].name, "192.168.1.10");
        assert_eq!(c.active, 0);
        assert_eq!(c.window_s, 300);
    }

    #[test]
    fn legacy_default_url_keeps_kserver_name() {
        let c = parse_config(&format!(r#"{{"url":"{LEGACY_DEFAULT_URL}"}}"#));
        assert_eq!(c.sources.len(), 1);
        assert_eq!(c.sources[0].name, "k-server");
    }

    #[test]
    fn active_index_clamps_on_load() {
        let c = parse_config(r#"{"sources":[{"name":"a","url":"ws://a/ws"}],"active":9}"#);
        assert_eq!(c.active, 0);
        let c = parse_config(r#"{"sources":[],"active":3}"#);
        assert_eq!(c.active, 0);
    }

    #[test]
    fn multi_source_config_roundtrips() {
        let c = Config {
            sources: vec![
                Source {
                    name: "k-server".into(),
                    url: "ws://10.0.0.1:1/ws".into(),
                },
                Source {
                    name: "lab".into(),
                    url: "ws://10.0.0.2:1/ws".into(),
                },
            ],
            active: 1,
            disk: Some("nvme0n1".into()),
            ..Config::default()
        };
        let back = parse_config(&serde_json::to_string(&c).expect("serialize"));
        assert_eq!(c, back);
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ebbtop-test-{}-{tag}.json", std::process::id()))
    }

    #[test]
    fn config_and_log_paths_use_ebbtop() {
        let c = config_path().expect("config path");
        assert!(c.ends_with("ebbtop/config.json"), "got {}", c.display());
        assert_eq!(c.file_name().unwrap(), "config.json");
        let l = log_path().expect("log path");
        assert_eq!(l.file_name().unwrap(), "ebbtop.log");
        assert!(
            l.parent().unwrap().ends_with("ebbtop"),
            "got {}",
            l.display()
        );
    }

    #[test]
    fn corrupt_config_falls_back_to_default() {
        let primary = temp_path("primary-corrupt");
        std::fs::write(&primary, r#"{"sources": [oops"#).expect("write primary");
        let c = load_config_from(Some(primary.clone()));
        assert!(c.sources.is_empty());
        let _ = std::fs::remove_file(&primary);
    }

    #[test]
    fn old_config_gets_update_defaults() {
        let c = parse_config(r#"{"sources":[]}"#);
        assert_eq!(c.update_last_check_ms, 0);
        assert!(c.update_skipped.is_empty());
    }

    #[test]
    fn old_config_gets_english_language() {
        let c = parse_config(r#"{"sources":[]}"#);
        assert_eq!(c.language, "en");
        assert_eq!(c.locale(), crate::l10n::Locale::English);
        assert_eq!(Config::default().language, "en");
    }

    #[test]
    fn known_and_unknown_language_values() {
        let c = parse_config(r#"{"sources":[],"language":"zh-Hant-TW"}"#);
        assert_eq!(c.locale(), crate::l10n::Locale::ZhHantTw);
        // An unknown value (hand edit, future build, Hong Kong tag) resolves
        // to English without dropping the rest of the config, and the raw
        // string is kept as written.
        let c = parse_config(
            r#"{"sources":[{"name":"a","url":"ws://h:1/ws"}],"language":"zh-Hant-HK"}"#,
        );
        assert_eq!(c.locale(), crate::l10n::Locale::English);
        assert_eq!(c.sources.len(), 1);
        assert_eq!(c.language, "zh-Hant-HK");
        // Nothing localized is ever written into the config.
        let c = Config {
            language: "zh-Hans-CN".into(),
            ..Config::default()
        };
        assert!(!serde_json::to_string(&c).expect("serialize").contains('中'));
    }

    #[test]
    fn skipped_version_roundtrips() {
        let c = Config {
            update_last_check_ms: 1700000000000,
            update_skipped: "v0.3.0".into(),
            ..Default::default()
        };
        let back = parse_config(&serde_json::to_string(&c).expect("serialize"));
        assert_eq!(c, back);
    }

    #[test]
    fn config_roundtrip_keeps_phase2() {
        let c = Config {
            top_frac: 0.3,
            show_net: false,
            show_disk: false,
            collapsed_cpu: true,
            collapsed_proc: true,
            ..Config::default()
        };
        let json = serde_json::to_string(&c).expect("serialize");
        let back: Config = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(c, back);
    }

    // ---- E4: support summary ---------------------------------------------

    /// Dummy secret. It must never appear in a summary, a copied file or a
    /// log line.
    const SENTINEL: &str = "DUMMYSENTINEL0123456789abcdef0123456789abcdef0123456789abcdef0123";

    #[test]
    fn endpoint_shape_drops_host_and_query() {
        assert_eq!(
            endpoint_shape("ws://10.9.9.9:45976/ws?token=x"),
            "ws, default port, /ws"
        );
        assert_eq!(
            endpoint_shape("wss://some.host:9443/custom"),
            "wss, custom port, /custom"
        );
        assert_eq!(endpoint_shape("barehost"), "ws, default port, /ws");
        assert!(!endpoint_shape("ws://10.9.9.9:45976/ws?token=x").contains("10.9.9.9"));
    }

    #[test]
    fn support_summary_keeps_token_address_and_path_out() {
        let cfg = Config {
            sources: vec![Source {
                name: "k-server".into(),
                url: format!("ws://10.9.9.9:45976/ws?token={SENTINEL}"),
            }],
            ..Config::default()
        };
        let endpoint = endpoint_shape(&cfg.sources[0].url);
        let detail = format!("get_metrics: ws://10.9.9.9/ws?token={SENTINEL}");
        let facts = SupportFacts {
            version: "0.2.7",
            platform: "linux x86_64",
            sources: cfg.sources.len(),
            endpoint: &endpoint,
            token_source: &TokenSource::Keyring,
            connection: "DOWN",
            failure: Some(("Closed right after the handshake", &detail)),
            log_bytes: Some(4096),
            config_ok: true,
        };
        let s = support_summary(&facts, Some(SENTINEL));
        assert!(!s.contains(SENTINEL), "token leaked:\n{s}");
        assert!(!s.contains("10.9.9.9"), "address leaked:\n{s}");
        assert!(!s.contains("k-server"), "source name leaked:\n{s}");
        assert!(!s.contains("/home/"), "path leaked:\n{s}");
        assert!(!s.contains("?token="), "query leaked:\n{s}");
        assert!(
            s.contains("detail: get_metrics: ws://<endpoint>"),
            "detail not reduced:\n{s}"
        );
        // Still useful: the facts a maintainer needs.
        assert!(s.contains("version: 0.2.7"));
        assert!(s.contains("connection: DOWN"));
        assert!(s.contains("last failure: Closed right after the handshake"));
        assert!(s.contains("log: ebbtop.log present (4 KiB)"));
    }

    #[test]
    fn scrub_urls_keeps_the_scheme_and_drops_the_authority() {
        assert_eq!(
            scrub_urls("ws://10.9.9.9:45976/ws?token=abc failed"),
            "ws://<endpoint> failed"
        );
        assert_eq!(scrub_urls("(wss://h/x)"), "(wss://<endpoint>)");
        assert_eq!(scrub_urls("no urls here"), "no urls here");
    }

    #[test]
    fn support_summary_reports_the_credential_source_without_the_error_text() {
        let facts = SupportFacts {
            version: "1.0.0",
            platform: "macos aarch64",
            sources: 0,
            endpoint: "none configured",
            token_source: &TokenSource::Error("libsecret exploded: token=leak".into()),
            connection: "CONNECTING",
            failure: None,
            log_bytes: None,
            config_ok: false,
        };
        let s = support_summary(&facts, None);
        assert!(
            s.contains("credential store error (details omitted)"),
            "{s}"
        );
        assert!(
            !s.contains("libsecret"),
            "credential error text leaked:\n{s}"
        );
        assert!(!s.contains("leak"), "credential error text leaked:\n{s}");
        assert!(s.contains("last failure: none"));
    }

    #[test]
    fn scrub_secret_is_idempotent() {
        let once = scrub_secret(&format!("x?token={SENTINEL}&y=1"), Some(SENTINEL));
        assert_eq!(once, "x?token=<token>&y=1");
        assert_eq!(scrub_secret(&once, Some(SENTINEL)), once);
    }

    #[test]
    fn support_copy_is_written_where_the_caller_asks() {
        let dir = std::env::temp_dir().join(format!("ebbtop-support-test-{}", std::process::id()));
        let p = dir.join("ebbtop-support.txt");
        let _ = std::fs::remove_dir_all(&dir);
        write_support(&p, "version: 0.2.7\n").expect("write support copy");
        assert_eq!(
            std::fs::read_to_string(&p).expect("read back"),
            "version: 0.2.7\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_writer_appends_and_rotates() {
        let dir = std::env::temp_dir().join(format!("ebbtop-log-test-{}", std::process::id()));
        let p = dir.join("ebbtop.log");
        let _ = std::fs::remove_dir_all(&dir);
        append_log_to(
            &p,
            "probe auth-refused: get_metrics: agent closed (wrong token?)",
        );
        let one = std::fs::read_to_string(&p).expect("log line");
        assert!(one.contains("probe auth-refused"), "line missing: {one}");
        assert!(!one.contains(SENTINEL));
        // Above the cap the file is rotated, not truncated away.
        std::fs::write(&p, "x".repeat((LOG_MAX + 1) as usize)).expect("seed big log");
        append_log_to(&p, "after rotation");
        assert!(p.with_extension("log.1").exists(), "no rotation");
        assert!(
            std::fs::read_to_string(&p)
                .expect("new log")
                .contains("after rotation")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_opener_is_platform_native() {
        let p = std::path::Path::new("/tmp/ebbtop-data/ebbtop.log");
        let (cmd, args) = opener_command(p);
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            assert_eq!(cmd, "xdg-open");
            assert_eq!(
                args,
                vec!["/tmp/ebbtop-data".to_string()],
                "open the folder, not the bare file"
            );
        }
        #[cfg(target_os = "macos")]
        {
            assert_eq!(cmd, "open");
            assert_eq!(args, vec!["-R".to_string(), p.display().to_string()]);
        }
        #[cfg(target_os = "windows")]
        {
            assert_eq!(cmd, "explorer");
            assert_eq!(args.len(), 1);
            assert!(args[0].starts_with("/select,"), "select the log: {args:?}");
        }
        let _ = (cmd, args);
    }
}
