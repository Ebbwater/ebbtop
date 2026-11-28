//! Shared agent state: sysinfo handles and hot JSON cache.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, Instant};
use sysinfo::{Components, Disks, Networks, System};
use tokio::sync::Mutex;

use crate::secret;

pub type SharedSystem = Arc<Mutex<System>>;
pub type SharedComponents = Arc<Mutex<Components>>;
pub type SharedDisks = Arc<Mutex<Disks>>;
pub type SharedNetworks = Arc<Mutex<Networks>>;

#[cfg(target_os = "linux")]
#[derive(Default)]
pub struct ProcCpuTracker {
    pub last_total: u64,
    pub last_per_pid: HashMap<u32, u64>,
    /// PID → process name cache. Mirrors the non-Linux `ProcessCache.names`.
    /// On a Pi with ~150-300 mostly-stable processes this avoids re-allocating
    /// the same `String`s on every processes poll (~once per 1.5s).
    pub names: HashMap<u32, String>,
    /// PID → cgroup path. Read once per pid and reused: /proc/<pid>/cgroup is
    /// an extra open per process per poll, and the path only changes for a pid
    /// that is moved between cgroups, which is rare enough to accept a stale
    /// value until the pid exits. Pruned with `names`.
    pub cgroups: HashMap<u32, String>,
}

#[cfg(not(target_os = "linux"))]
pub struct ProcessCache {
    pub names: HashMap<u32, String>,
    pub reusable_vec: Vec<crate::types::ProcessInfo>,
}

#[cfg(not(target_os = "linux"))]
impl Default for ProcessCache {
    fn default() -> Self {
        Self {
            names: HashMap::with_capacity(1000), // Pre-allocate for typical modern system process count
            reusable_vec: Vec::with_capacity(1000),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub sys: SharedSystem,
    pub components: SharedComponents,
    pub disks: SharedDisks,
    pub networks: SharedNetworks,
    pub hostname: String,
    /// ebbtop P3a: agent build, reported by `--version`, the discovery
    /// responder and the metrics frame alike.
    pub agent_version: String,
    /// ebbtop P3a: random per-boot nonce. Minted once in `new()`; a client
    /// that sees it change knows the agent restarted.
    pub boot_nonce: String,

    // For correct per-process CPU% using /proc deltas (Linux only path uses this tracker)
    #[cfg(target_os = "linux")]
    pub proc_cpu: Arc<Mutex<ProcCpuTracker>>,

    // Process name caching and vector reuse for non-Linux to reduce allocations
    #[cfg(not(target_os = "linux"))]
    pub proc_cache: Arc<Mutex<ProcessCache>>,

    // Connection tracking (to allow future idle sleeps if desired)
    pub client_count: Arc<AtomicUsize>,

    pub auth_token: Option<String>,
    /// ebbtop P3a: pairing-code and issued-client store (files in
    /// `pairing::state_dir()`). Read on every ws upgrade; writes are atomic
    /// renames, so a reader never sees a half-written store.
    pub pairing: Arc<crate::pairing::PairingStore>,
    /// Serialises `/pair` exchanges so two concurrent requests cannot both
    /// consume one code.
    pub pair_lock: Arc<tokio::sync::Mutex<()>>,
    /// `/pair` per-source and global rate limit: the endpoint is
    /// unauthenticated by design, so a wrong-guess flood must not become a
    /// fsync storm on the state directory.
    pub pair_rate: Arc<std::sync::Mutex<crate::pairing::PairRateLimit>>,
    // GPU negative cache (probe once). gpu_checked=true after first attempt; gpu_present reflects result.
    pub gpu_checked: Arc<AtomicBool>,
    pub gpu_present: Arc<AtomicBool>,

    // Lightweight on-demand caches (TTL based) to cap CPU under bursty polling.
    pub cache_metrics: Arc<Mutex<CacheEntry<crate::types::Metrics>>>,
    pub cache_disks: Arc<Mutex<CacheEntry<Vec<crate::types::DiskInfo>>>>,
    pub cache_processes: Arc<Mutex<CacheEntry<crate::types::ProcessesPayload>>>,

    // Process detail caches (per-PID)
    pub cache_process_metrics:
        Arc<Mutex<HashMap<u32, CacheEntry<crate::types::ProcessMetricsResponse>>>>,
    pub cache_journal_entries: Arc<Mutex<HashMap<u32, CacheEntry<crate::types::JournalResponse>>>>,
}

/// TTL-gated value behind a std Mutex, for `static` caches on hot paths.
/// Replaces the hand-rolled TempCache/GpuCache/refresh-timestamp statics
/// that each reimplemented the same at/value pair.
pub struct TtlCell<T> {
    inner: std::sync::Mutex<CacheEntry<T>>,
}

impl<T: Clone> Default for TtlCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> TtlCell<T> {
    pub const fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(CacheEntry::new()),
        }
    }
    /// The stored value, only while fresh. Poisoned lock reads as a miss.
    pub fn get_fresh(&self, ttl: Duration) -> Option<T> {
        let g = self.inner.lock().ok()?;
        if g.is_fresh(ttl) {
            g.value.clone()
        } else {
            None
        }
    }
    pub fn set(&self, v: T) {
        if let Ok(mut g) = self.inner.lock() {
            g.set(v);
        }
    }
    /// True exactly once per TTL window: restamps and tells the caller to do
    /// the refresh. Atomic check-and-stamp so concurrent callers don't both
    /// refresh.
    pub fn claim_stale(&self, ttl: Duration) -> bool {
        let Ok(mut g) = self.inner.lock() else {
            return false;
        };
        if g.at.is_none_or(|t| t.elapsed() >= ttl) {
            g.at = Some(Instant::now());
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Debug)]
pub struct CacheEntry<T> {
    pub at: Option<Instant>,
    pub value: Option<T>,
}

impl<T> Default for CacheEntry<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> CacheEntry<T> {
    pub const fn new() -> Self {
        Self {
            at: None,
            value: None,
        }
    }
    pub fn is_fresh(&self, ttl: Duration) -> bool {
        self.at.is_some_and(|t| t.elapsed() < ttl) && self.value.is_some()
    }
    pub fn set(&mut self, v: T) {
        self.value = Some(v);
        self.at = Some(Instant::now());
    }
    pub fn get(&self) -> Option<&T> {
        self.value.as_ref()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::with_pairing_dir(crate::pairing::state_dir())
    }

    /// Test/embedding constructor: identical state, explicit pairing
    /// directory instead of the `EBBTOP_STATE_DIR` resolution.
    pub fn with_pairing_dir(dir: impl Into<std::path::PathBuf>) -> Self {
        let sys = System::new();
        let components = Components::new_with_refreshed_list();
        let disks = Disks::new_with_refreshed_list();
        let networks = Networks::new_with_refreshed_list();

        Self {
            sys: Arc::new(Mutex::new(sys)),
            components: Arc::new(Mutex::new(components)),
            disks: Arc::new(Mutex::new(disks)),
            networks: Arc::new(Mutex::new(networks)),
            hostname: System::host_name().unwrap_or_else(|| "unknown".into()),
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
            boot_nonce: crate::secret::nonce(),
            #[cfg(target_os = "linux")]
            proc_cpu: Arc::new(Mutex::new(ProcCpuTracker::default())),
            #[cfg(not(target_os = "linux"))]
            proc_cache: Arc::new(Mutex::new(ProcessCache::default())),
            client_count: Arc::new(AtomicUsize::new(0)),
            auth_token: std::env::var("EBBTOP_TOKEN")
                .or_else(|_| std::env::var("SOCKTOP_TOKEN"))
                .ok()
                .filter(|s| !s.is_empty()),
            pairing: Arc::new(crate::pairing::PairingStore::new(dir)),
            pair_lock: Arc::new(tokio::sync::Mutex::new(())),
            pair_rate: Arc::new(std::sync::Mutex::new(crate::pairing::PairRateLimit::new())),
            gpu_checked: Arc::new(AtomicBool::new(false)),
            gpu_present: Arc::new(AtomicBool::new(false)),
            cache_metrics: Arc::new(Mutex::new(CacheEntry::new())),
            cache_disks: Arc::new(Mutex::new(CacheEntry::new())),
            cache_processes: Arc::new(Mutex::new(CacheEntry::new())),
            cache_process_metrics: Arc::new(Mutex::new(HashMap::new())),
            cache_journal_entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Decide what a connection may do, from the token it presented.
    ///
    /// The operator's manual token keeps exactly the behaviour it had before
    /// P3a (`Some(expected)` and an equal query token) — but the compare is
    /// length-independent constant time (the same helper `/pair` uses), so the
    /// network-facing gate does not leak a prefix match. The only addition is
    /// the enrollment store: a token issued through `/pair` authenticates as
    /// [`AuthScope::ReadOnly`].
    ///
    /// No token configured is no longer the legacy "auth not required, full
    /// scope": it serves read-only only. `platform::startup_policy` already
    /// refuses to start a tokenless agent on anything but loopback, so this
    /// scope only ever applies to a loopback dev run — and even there the
    /// kill path needs a real token.
    pub fn authenticate(&self, presented: Option<&str>) -> Option<AuthScope> {
        match (self.auth_token.as_deref(), presented) {
            (Some(expected), Some(t)) if secret::constant_time_eq(t, expected) => {
                Some(AuthScope::Full)
            }
            (Some(_), Some(t)) if self.pairing.authenticate(t).is_some() => {
                Some(AuthScope::ReadOnly)
            }
            // Tokenless (loopback-only, enforced at startup): read-only.
            (None, _) => Some(AuthScope::ReadOnly),
            _ => None,
        }
    }
}

/// What an authenticated connection may ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthScope {
    /// The operator's manual token: the whole protocol, including `kill:`.
    Full,
    /// A token issued by `/pair`: read-only monitoring, never `kill:`.
    ReadOnly,
}

impl AuthScope {
    pub fn allows_kill(self) -> bool {
        matches!(self, AuthScope::Full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_token_is_full_scope_and_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = AppState::with_pairing_dir(dir.path());
        st.auth_token = Some("manual".into());
        assert_eq!(st.authenticate(Some("manual")), Some(AuthScope::Full));
        assert_eq!(st.authenticate(Some("wrong")), None);
        assert_eq!(st.authenticate(None), None);
        assert!(st.authenticate(Some("manual")).unwrap().allows_kill());
    }

    #[test]
    fn issued_token_is_read_only_and_verified_against_the_same_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = AppState::with_pairing_dir(dir.path());
        st.auth_token = Some("manual".into());
        let rec = st.pairing.issue(Some("laptop")).unwrap();
        let scope = st.authenticate(Some(&rec.token)).unwrap();
        assert_eq!(scope, AuthScope::ReadOnly);
        assert!(!scope.allows_kill());
    }

    #[test]
    fn no_configured_token_serves_read_only_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = AppState::with_pairing_dir(dir.path());
        st.auth_token = None;
        assert_eq!(st.authenticate(None), Some(AuthScope::ReadOnly));
        assert_eq!(st.authenticate(Some("anything")), Some(AuthScope::ReadOnly));
        // The kill path is never reachable without a configured token.
        assert!(!st.authenticate(None).unwrap().allows_kill());
    }

    #[test]
    fn every_process_gets_its_own_nonce() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let sa = AppState::with_pairing_dir(a.path());
        let sb = AppState::with_pairing_dir(b.path());
        assert_eq!(sa.boot_nonce.len(), 32);
        assert_ne!(sa.boot_nonce, sb.boot_nonce);
        assert!(!sa.agent_version.is_empty());
    }
}
