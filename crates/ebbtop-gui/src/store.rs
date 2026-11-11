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
        && let Some(u) = v.get("url").and_then(|u| u.as_str()).filter(|u| !u.is_empty())
    {
        cfg.sources = vec![Source { name: name_from_url(u), url: u.to_string() }];
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
    if host.is_empty() { "server".to_string() } else { host.to_string() }
}

pub fn load_config() -> Config {
    load_config_from(config_path())
}

pub fn save_config(c: &Config) -> anyhow::Result<()> {
    let p = config_path().ok_or_else(|| anyhow::anyhow!("no config dir"))?;
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
    match keyring::Entry::new(KEYRING_SERVICE, user).and_then(|e| e.get_password()) {
        Ok(t) if !t.is_empty() => (Some(t), TokenSource::Keyring),
        Ok(_) | Err(keyring::Error::NoEntry) => (None, TokenSource::Missing),
        Err(e) => (None, TokenSource::Error(e.to_string())),
    }
}

pub fn save_token_for(user: &str, token: &str) -> anyhow::Result<()> {
    keyring::Entry::new(KEYRING_SERVICE, user)?.set_password(token)?;
    Ok(())
}

/// Drop a source's token. Best effort: a missing entry is not an error.
/// Called when a source is removed so its secret does not linger.
pub fn delete_token_for(user: &str) {
    let _ = keyring::Entry::new(KEYRING_SERVICE, user).and_then(|e| e.delete_credential());
}

pub fn log_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ebbtop").join("ebbtop.log"))
}

const LOG_MAX: u64 = 2 * 1024 * 1024;

/// Append one timestamped line. Rotates to ebbtop.log.1 at 2 MiB. Never
/// fails the caller. Callers must not pass secrets.
pub fn append_log(line: &str) {
    use std::io::Write;
    let Some(p) = log_path() else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if std::fs::metadata(&p).map(|m| m.len() > LOG_MAX).unwrap_or(false) {
        let _ = std::fs::rename(&p, p.with_extension("log.1"));
    }
    let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%z");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
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
        assert!(!c.collapsed_cpu && !c.collapsed_gpu && !c.collapsed_mem && !c.collapsed_net && !c.collapsed_disk && !c.collapsed_proc);
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
                Source { name: "k-server".into(), url: "ws://10.0.0.1:1/ws".into() },
                Source { name: "lab".into(), url: "ws://10.0.0.2:1/ws".into() },
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
        assert!(l.parent().unwrap().ends_with("ebbtop"), "got {}", l.display());
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
    fn skipped_version_roundtrips() {
        let mut c = Config::default();
        c.update_last_check_ms = 1700000000000;
        c.update_skipped = "v0.3.0".into();
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
}
