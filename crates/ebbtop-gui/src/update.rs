//! Background release check + update dialog state (spec: hot-updater design, section 1).
//!
//! The check runs on a plain thread (never the UI thread, never blocking
//! startup): `GET .../releases/latest`, `tag_name` out of the JSON body,
//! semver-compare against this build's `CARGO_PKG_VERSION`. A failed check
//! is a quiet status-bar note + log line; the next check runs at the next
//! interval. Accepting records the approved version; `download_verified`
//! streams the asset to `<dest>.tmp`, hash-checks it against `SHA256SUMS`,
//! and renames on success. Windows apply spawns the installer detached
//! (the caller then quits); macOS apply unzips the staged bundle
//! alongside, swaps it in keeping a `.bak` copy, and relaunches with
//! `open` (the caller then quits).

use std::sync::mpsc;
use std::time::{Duration, Instant};

use ebbtop_core::update::{ReleaseAsset, is_update_available, parse_release_assets};

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

/// Outcome of one finished check. `Available` carries the release's
/// asset list so the download step needs no second fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    Available { version: String, assets: Vec<ReleaseAsset> },
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
            Some(tag) if is_update_available(current, &tag) => {
                CheckOutcome::Available { version: tag, assets: parse_release_assets(&body) }
            }
            Some(tag) => CheckOutcome::UpToDate { version: tag },
        },
    }
}

/// Dialog-level status owned by the GUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    Available { version: String, assets: Vec<ReleaseAsset> },
    UpToDate { version: String },
    Failed(String),
    /// User approved this version; the dialog starts the verified
    /// download for it on the next frame.
    Accepted { version: String, assets: Vec<ReleaseAsset> },
    /// Verified download in flight; `done` is bytes written so far.
    Downloading { version: String, done: u64 },
    /// Verified download on disk, ready for the platform apply step.
    Ready { version: String, path: std::path::PathBuf },
}

/// One extra release file fetched (and hash-verified against the same
/// `SHA256SUMS`) before the main asset on the download thread. Windows
/// uses it for `install.ps1`, which is not the restart payload but must be
/// present for the apply step.
#[derive(Debug, Clone)]
pub struct ExtraDownload {
    pub url: String,
    pub sums_url: String,
    pub asset: String,
    pub dest: std::path::PathBuf,
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
    /// In-flight verified download: outcome plus byte-progress channels,
    /// and the version/path they belong to.
    dl_rx: Option<mpsc::Receiver<Result<(), String>>>,
    dl_prog: Option<mpsc::Receiver<u64>>,
    dl_job: Option<(String, std::path::PathBuf)>,
}

impl UpdateCheck {
    pub fn new() -> Self {
        Self::with_interval(CHECK_INTERVAL)
    }

    pub fn with_interval(interval: Duration) -> Self {
        Self {
            status: UpdateStatus::Idle,
            last_check: None,
            interval,
            rx: None,
            notified: None,
            dl_rx: None,
            dl_prog: None,
            dl_job: None,
        }
    }

    /// Restore persisted check state: `last_check_ms` is unix millis
    /// (0 = never checked), `skipped` a declined release tag. A future
    /// timestamp (clock skew) counts as due rather than parking checks.
    pub fn with_persisted(last_check_ms: u64, skipped: String, now_ms: u64) -> Self {
        let mut u = Self::new();
        if last_check_ms != 0
            && let Some(age_ms) = now_ms.checked_sub(last_check_ms)
        {
            u.last_check = Some(Instant::now() - Duration::from_millis(age_ms));
        }
        if !skipped.is_empty() {
            u.notified = Some(skipped);
        }
        u
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

    /// Drain a finished check or download into `status`. Returns true on
    /// a transition (including download progress, so the dialog repaints).
    pub fn poll(&mut self) -> bool {
        if self.poll_download() {
            return true;
        }
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
                    CheckOutcome::Available { version, assets } => {
                        UpdateStatus::Available { version, assets }
                    }
                    CheckOutcome::UpToDate { version } => UpdateStatus::UpToDate { version },
                    CheckOutcome::Failed(e) => UpdateStatus::Failed(e),
                };
                true
            }
        }
    }

    /// Drain the in-flight download: progress bytes refresh `Downloading`,
    /// the outcome lands in `Ready` or `Failed`. True on any change.
    fn poll_download(&mut self) -> bool {
        let mut changed = false;
        if let Some(prog) = &self.dl_prog
            && let Ok(done) = prog.try_recv()
        {
            // Keep only the latest count; the dialog reads it per frame.
            let mut last = done;
            while let Ok(n) = prog.try_recv() {
                last = n;
            }
            if let UpdateStatus::Downloading { done: d, .. } = &mut self.status {
                *d = last;
                changed = true;
            }
        }
        let done = match &self.dl_rx {
            Some(rx) => match rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("download thread ended".to_string()))
                }
                Ok(r) => Some(r),
            },
            None => None,
        };
        if let Some(outcome) = done {
            self.dl_rx = None;
            self.dl_prog = None;
            let (version, path) = self.dl_job.take().unwrap_or_default();
            self.status = match outcome {
                Ok(()) => UpdateStatus::Ready { version, path },
                Err(e) => UpdateStatus::Failed(e),
            };
            changed = true;
        }
        changed
    }

    /// Start the verified download on a thread; status moves to
    /// `Downloading` at once. No-op while one is already running. When
    /// `extra` is present (Windows `install.ps1`), it is verified first on
    /// the same thread; the outcome still lands in `Ready`/`Failed` as one
    /// job, and a failed main asset removes the extra again so a failed
    /// update leaves no files behind.
    pub fn start_download(
        &mut self,
        version: String,
        asset_url: String,
        sums_url: String,
        asset: String,
        dest: std::path::PathBuf,
        extra: Option<ExtraDownload>,
    ) {
        if self.dl_rx.is_some() {
            return;
        }
        let (out_tx, out_rx) = mpsc::channel();
        let (prog_tx, prog_rx) = mpsc::channel();
        self.dl_rx = Some(out_rx);
        self.dl_prog = Some(prog_rx);
        self.dl_job = Some((version.clone(), dest.clone()));
        self.status = UpdateStatus::Downloading { version, done: 0 };
        std::thread::Builder::new()
            .name("ebbtop-update-download".into())
            .spawn(move || {
                let outcome = match &extra {
                    Some(x) => {
                        // Progress of the helper file would pollute the
                        // dialog's MB counter; verify it on a dropped
                        // receiver instead.
                        let (null_tx, _) = mpsc::channel();
                        match download_verified(&x.url, &x.sums_url, &x.asset, &x.dest, &null_tx) {
                            Err(e) => Err(e),
                            Ok(()) => download_verified(&asset_url, &sums_url, &asset, &dest, &prog_tx)
                                .map_err(|e| {
                                    let _ = std::fs::remove_file(&x.dest);
                                    e
                                }),
                        }
                    }
                    None => download_verified(&asset_url, &sums_url, &asset, &dest, &prog_tx),
                };
                let _ = out_tx.send(outcome);
            })
            .ok();
    }

    /// True when the dialog should pop: an unprompted available version.
    pub fn should_prompt(&self) -> bool {
        match &self.status {
            UpdateStatus::Available { version, .. } => self.notified.as_ref() != Some(version),
            _ => false,
        }
    }

    /// The dialog popped for the current version (Available prompt or
    /// Ready restart): that tag stays quiet until a newer one appears.
    pub fn mark_notified(&mut self) {
        match &self.status {
            UpdateStatus::Available { version, .. } | UpdateStatus::Ready { version, .. } => {
                self.notified = Some(version.clone());
            }
            _ => {}
        }
    }

    /// User approved the available version; the dialog starts the verified
    /// download for it on the next frame.
    pub fn accept(&mut self) {
        if let UpdateStatus::Available { version, assets } = &self.status {
            self.status = UpdateStatus::Accepted { version: version.clone(), assets: assets.clone() };
        }
    }

    /// Test hook: place the state machine in any status.
    #[cfg(test)]
    pub fn test_set_status(&mut self, status: UpdateStatus) {
        self.status = status;
    }

    /// Move to `Failed` with a caller-made message (unserved platform,
    /// missing bundle, apply refusal). The dialog shows it with a retry.
    pub(crate) fn set_failed(&mut self, msg: String) {
        self.status = UpdateStatus::Failed(msg);
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

/// Download destination for a release asset: the OS temp dir, named
/// `ebbtop-update-<version>-<asset>` with the version sanitized to
/// filename-safe characters (a hostile tag must not escape the name).
pub fn download_dest_path(version: &str, asset: &str) -> std::path::PathBuf {
    let safe: String = version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = if safe.is_empty() { "update".to_string() } else { safe };
    std::env::temp_dir().join(format!("ebbtop-update-{name}-{asset}"))
}

/// Bundle dir for a running executable: the nearest ancestor ending in
/// `.app`, or `None` when not running from a bundle (dev builds, tests).
/// Takes the exe path so tests can pass fake ones on any platform.
pub fn app_bundle_dir(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    exe.ancestors()
        .find(|a| a.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
        .map(|a| a.to_path_buf())
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

/// Startup shortcut name managed by `deploy/windows/install.ps1`.
pub const STARTUP_SHORTCUT_NAME: &str = "EbbTop.lnk";

/// Installer script asset in a release (Windows only). It is hash-covered
/// by the same `SHA256SUMS` file as the exe, so the updater verifies it
/// the same way. The running install does not keep a copy (the script only
/// copies the exe into `%LOCALAPPDATA%\ebbtop`), so the updater downloads
/// it fresh from the release into the temp dir next to the exe.
pub const INSTALL_PS1_ASSET: &str = "install.ps1";

/// Temp path for the downloaded installer script of a release version.
/// Same scheme as the exe download, so restart derives it from the version
/// alone without carrying a second path through `UpdateStatus`.
pub fn windows_installer_dest(version: &str) -> std::path::PathBuf {
    download_dest_path(version, INSTALL_PS1_ASSET)
}

/// Extra download for the Windows installer script of a release: the
/// script's URL out of the release asset list, verified against the same
/// `sums_url` as the exe, landing at [`windows_installer_dest`]. `None`
/// when the release carries no `install.ps1` — the caller then fails the
/// update instead of restarting without an installer.
pub fn windows_installer_extra(
    version: &str,
    assets: &[ReleaseAsset],
    sums_url: &str,
) -> Option<ExtraDownload> {
    let url = assets.iter().find(|a| a.name == INSTALL_PS1_ASSET)?.url.clone();
    Some(ExtraDownload {
        url,
        sums_url: sums_url.to_string(),
        asset: INSTALL_PS1_ASSET.to_string(),
        dest: windows_installer_dest(version),
    })
}

/// Full path of the Startup shortcut for a config dir (on Windows:
/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\EbbTop.lnk`).
/// Takes the dir so tests can pass a scratch dir on any platform.
pub fn startup_shortcut_path(config_dir: &std::path::Path) -> std::path::PathBuf {
    config_dir
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join(STARTUP_SHORTCUT_NAME)
}

/// True when the Startup shortcut exists under `config_dir`.
pub fn autostart_enabled_in(config_dir: &std::path::Path) -> bool {
    startup_shortcut_path(config_dir).is_file()
}

/// True when the Startup shortcut exists (spec section 3: preserve
/// Autostart by passing `-Autostart` to `install.ps1`).
pub fn autostart_enabled() -> bool {
    dirs::config_dir().map(|d| autostart_enabled_in(&d)).unwrap_or(false)
}

/// argv for the Windows apply step (spec design, section 3): `powershell
/// -ExecutionPolicy Bypass -File <install.ps1> -Exe <downloaded exe>`,
/// plus `-Autostart` when the Startup shortcut exists so the swapped
/// install keeps its autostart shortcut. `argv[0]` is the program; the
/// rest are its arguments.
pub fn windows_apply_argv(
    install_ps1: &std::path::Path,
    downloaded_exe: &std::path::Path,
    autostart: bool,
) -> Vec<String> {
    let mut argv = vec![
        "powershell".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        install_ps1.to_string_lossy().into_owned(),
        "-Exe".to_string(),
        downloaded_exe.to_string_lossy().into_owned(),
    ];
    if autostart {
        argv.push("-Autostart".to_string());
    }
    argv
}

/// Detached child spawn: the installer outlives this process (it kills the
/// running exe and swaps in the verified download). On Windows the child gets
/// no console window, its own process group, and breaks away from any job so it
/// survives this GUI app exiting; its stdio is nulled because a GUI parent has
/// no console to inherit. Elsewhere a plain spawn (the apply step only runs on
/// Windows in practice).
fn spawn_detached(program: &str, args: &[String]) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(
                CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
            )
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new(program).args(args).spawn().map(|_| ())
    }
}

/// Windows apply step (spec design, section 3): run
/// `powershell -ExecutionPolicy Bypass -File install.ps1 -Exe <download>`
/// detached so the script can kill this process, keep `.bak`, and swap the
/// exe; the caller then quits and the relaunched install takes over.
/// `-Autostart` is added when the Startup shortcut exists. Both files must
/// already exist — the download must be hash-verified first; nothing
/// spawns otherwise, so a bad path never kills the running app. The
/// updater never deletes `.bak`; rollback stays the script's `-Rollback`
/// path.
pub fn apply_update_windows(
    downloaded_exe: &std::path::Path,
    install_ps1: &std::path::Path,
) -> Result<(), String> {
    if !downloaded_exe.is_file() {
        return Err(format!("update file missing: {}", downloaded_exe.display()));
    }
    if !install_ps1.is_file() {
        return Err(format!("installer script missing: {}", install_ps1.display()));
    }
    let autostart = autostart_enabled();
    let argv = windows_apply_argv(install_ps1, downloaded_exe, autostart);
    spawn_detached(&argv[0], &argv[1..]).map_err(|e| format!("could not start installer: {e}"))
}

/// Staged bundle name inside the verified macOS download.
pub const MACOS_STAGED_APP: &str = "EbbTop.app";

/// Backup path for the running bundle: `<app>.bak` alongside it (the
/// rollback copy; the updater leaves it in place, like `install.ps1`).
pub fn macos_backup_path(running_app: &std::path::Path) -> std::path::PathBuf {
    let mut backup = running_app.as_os_str().to_owned();
    backup.push(".bak");
    std::path::PathBuf::from(backup)
}

/// Scratch dir for unpacking the staged bundle: alongside the running app
/// so the final renames stay on one filesystem. Owned by the updater; a
/// stale one from an interrupted update is cleared before reuse.
pub fn macos_stage_dir(running_app: &std::path::Path) -> std::path::PathBuf {
    running_app
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(".ebbtop-update-stage")
}

/// argv to unpack the verified download into the stage dir. `unzip` ships
/// with macOS, so the updater needs no extra dependency; `-o` refreshes a
/// reused stage dir without prompting. `argv[0]` is the program; the rest
/// are its arguments.
pub fn macos_unzip_argv(
    staged_zip: &std::path::Path,
    stage_dir: &std::path::Path,
) -> Vec<String> {
    vec![
        "unzip".to_string(),
        "-o".to_string(),
        "-q".to_string(),
        staged_zip.to_string_lossy().into_owned(),
        "-d".to_string(),
        stage_dir.to_string_lossy().into_owned(),
    ]
}

/// argv to relaunch the swapped bundle: `open <app>`. Spawned detached;
/// the caller then quits and the new bundle takes over.
pub fn macos_relaunch_argv(running_app: &std::path::Path) -> Vec<String> {
    vec!["open".to_string(), running_app.to_string_lossy().into_owned()]
}

/// Run a macOS apply helper (`unzip`), mapping failures to updater errors
/// that name the step, never the full command line.
fn run_update_helper(program: &str, args: &[String], what: &str) -> Result<(), String> {
    let status = std::process::Command::new(program)
        .args(args)
        .status()
        .map_err(|e| format!("{what}: could not start {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what}: {program} exited with {status}"))
    }
}

/// macOS apply step (spec design, section 3): unpack the verified
/// `EbbTop.app.zip` alongside the running bundle, swap it in keeping a
/// `.bak` copy of the old app, relaunch with `open`, and return so the
/// caller can quit. The previous `.bak` is superseded by the
/// just-replaced app (like `install.ps1 -Force`), so the rollback path
/// always points at the prior build; the updater never removes `.bak`
/// afterwards. Both inputs must already exist — the download must be
/// hash-verified first; nothing spawns otherwise, so a bad path never
/// kills the running app. Reached from the dialog's Restart button; the
/// live run happens on a Mac.
pub fn apply_update_macos(
    staged_zip: &std::path::Path,
    running_app: &std::path::Path,
) -> Result<(), String> {
    if !staged_zip.is_file() {
        return Err(format!("update file missing: {}", staged_zip.display()));
    }
    if !running_app.is_dir() {
        return Err(format!("running app missing: {}", running_app.display()));
    }
    let stage = macos_stage_dir(running_app);
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)
        .map_err(|e| format!("could not stage update in {}: {e}", stage.display()))?;
    let unzip = macos_unzip_argv(staged_zip, &stage);
    run_update_helper(&unzip[0], &unzip[1..], "could not unpack update")?;
    let staged_app = stage.join(MACOS_STAGED_APP);
    if !staged_app.is_dir() {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(format!("update is missing {}", MACOS_STAGED_APP));
    }
    let backup = macos_backup_path(running_app);
    let _ = std::fs::remove_dir_all(&backup);
    std::fs::rename(running_app, &backup)
        .map_err(|e| format!("could not back up running app: {e}"))?;
    if let Err(e) = std::fs::rename(&staged_app, running_app) {
        // Best-effort restore so the old bundle stays launchable.
        let _ = std::fs::rename(&backup, running_app);
        return Err(format!("could not install update: {e}"));
    }
    let _ = std::fs::remove_dir_all(&stage);
    let relaunch = macos_relaunch_argv(running_app);
    spawn_detached(&relaunch[0], &relaunch[1..]).map_err(|e| format!("could not relaunch: {e}"))
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
    fn seeded_skip_suppresses_prompt_for_same_version() {
        let mut u = UpdateCheck::with_persisted(0, "v0.3.0".into(), 1_700_000_000_000);
        u.status = UpdateStatus::Available { version: "v0.3.0".into(), assets: Vec::new() };
        assert!(!u.should_prompt(), "skipped version must not re-prompt after restart");
        u.status = UpdateStatus::Available { version: "v0.4.0".into(), assets: Vec::new() };
        assert!(u.should_prompt(), "a newer version must still prompt");
    }

    #[test]
    fn seeded_check_time_drives_due() {
        let now = 1_700_000_000_000u64;
        let fresh = UpdateCheck::with_persisted(now - 3_600_000, String::new(), now);
        assert!(!fresh.due(), "a check 1h ago is not due");
        let stale = UpdateCheck::with_persisted(now - 25 * 3_600_000, String::new(), now);
        assert!(stale.due(), "a check 25h ago is due");
        let never = UpdateCheck::with_persisted(0, String::new(), now);
        assert!(never.due(), "no persisted check is due");
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
            CheckOutcome::Available { version: "v0.3.0".into(), assets: Vec::new() }
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
    fn decide_carries_asset_urls() {
        let body = r#"{"tag_name":"v9.9.9","assets":[{"name":"ebbtop.exe","browser_download_url":"http://x/e"},{"name":"SHA256SUMS","browser_download_url":"http://x/s"}]}"#;
        match decide("0.2.0", Ok(body.into())) {
            CheckOutcome::Available { version, assets } => {
                assert_eq!(version, "v9.9.9");
                assert_eq!(
                    ebbtop_core::update::release_asset_url(&assets, "ebbtop.exe").as_deref(),
                    Some("http://x/e")
                );
                assert_eq!(
                    ebbtop_core::update::release_asset_url(
                        &assets,
                        ebbtop_core::update::SUMS_ASSET
                    )
                    .as_deref(),
                    Some("http://x/s")
                );
            }
            other => panic!("expected Available, got {other:?}"),
        }
    }

    #[test]
    fn download_job_reaches_ready() {
        let mut routes = std::collections::HashMap::new();
        routes.insert("/pkg.bin".to_string(), FIXTURE_BYTES.to_vec());
        routes.insert(
            "/SHA256SUMS".to_string(),
            format!("{FIXTURE_SHA256}  pkg.bin\n").into_bytes(),
        );
        let base = loopback_server(routes, None, 2);
        let dir = scratch_dir("job-ready");
        let dest = dir.join("pkg.bin");
        let mut c = UpdateCheck::new();
        c.start_download(
            "v9.9.9".into(),
            format!("{base}/pkg.bin"),
            format!("{base}/SHA256SUMS"),
            "pkg.bin".into(),
            dest.clone(),
            None,
        );
        assert!(matches!(c.status(), UpdateStatus::Downloading { .. }));
        for _ in 0..500 {
            c.poll();
            if matches!(c.status(), UpdateStatus::Ready { .. }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        match c.status() {
            UpdateStatus::Ready { version, path } => {
                assert_eq!(version, "v9.9.9");
                assert_eq!(path, &dest);
                assert!(dest.is_file(), "verified file landed");
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn download_job_failure_cleans_up() {
        let mut routes = std::collections::HashMap::new();
        routes.insert("/pkg.bin".to_string(), FIXTURE_BYTES.to_vec());
        routes.insert(
            "/SHA256SUMS".to_string(),
            format!("{}  pkg.bin\n", "0".repeat(64)).into_bytes(),
        );
        let base = loopback_server(routes, None, 2);
        let dir = scratch_dir("job-fail");
        let dest = dir.join("pkg.bin");
        let mut c = UpdateCheck::new();
        c.start_download(
            "v9.9.9".into(),
            format!("{base}/pkg.bin"),
            format!("{base}/SHA256SUMS"),
            "pkg.bin".into(),
            dest.clone(),
            None,
        );
        for _ in 0..500 {
            c.poll();
            if matches!(c.status(), UpdateStatus::Failed(_)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(c.status(), UpdateStatus::Failed(_)), "got {:?}", c.status());
        assert!(!dest.exists(), "no file left behind");
        assert!(dir_entries(&dir).is_empty(), "partial removed");
    }

    #[test]
    fn download_extra_helper_lands_with_main() {
        let mut routes = std::collections::HashMap::new();
        routes.insert("/pkg.bin".to_string(), FIXTURE_BYTES.to_vec());
        routes.insert("/helper.ps1".to_string(), FIXTURE_BYTES.to_vec());
        routes.insert(
            "/SHA256SUMS".to_string(),
            format!("{FIXTURE_SHA256}  pkg.bin\n{FIXTURE_SHA256}  helper.ps1\n").into_bytes(),
        );
        // Four hits: sums + file for the helper, then sums + file again
        // for the main asset (each verified download re-fetches SHA256SUMS).
        let base = loopback_server(routes, None, 4);
        let dir = scratch_dir("extra-ok");
        let dest = dir.join("pkg.bin");
        let helper = dir.join("helper.ps1");
        let mut c = UpdateCheck::new();
        c.start_download(
            "v9.9.9".into(),
            format!("{base}/pkg.bin"),
            format!("{base}/SHA256SUMS"),
            "pkg.bin".into(),
            dest.clone(),
            Some(ExtraDownload {
                url: format!("{base}/helper.ps1"),
                sums_url: format!("{base}/SHA256SUMS"),
                asset: "helper.ps1".into(),
                dest: helper.clone(),
            }),
        );
        for _ in 0..500 {
            c.poll();
            if matches!(c.status(), UpdateStatus::Ready { .. }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(c.status(), UpdateStatus::Ready { .. }), "got {:?}", c.status());
        assert_eq!(std::fs::read(&dest).expect("main asset"), FIXTURE_BYTES);
        assert_eq!(std::fs::read(&helper).expect("helper asset"), FIXTURE_BYTES);
    }

    #[test]
    fn download_extra_failure_leaves_nothing() {
        let mut routes = std::collections::HashMap::new();
        routes.insert("/pkg.bin".to_string(), FIXTURE_BYTES.to_vec());
        routes.insert("/helper.ps1".to_string(), FIXTURE_BYTES.to_vec());
        routes.insert(
            "/SHA256SUMS".to_string(),
            format!("{FIXTURE_SHA256}  pkg.bin\n{}  helper.ps1\n", "0".repeat(64)).into_bytes(),
        );
        let base = loopback_server(routes, None, 2);
        let dir = scratch_dir("extra-bad");
        let dest = dir.join("pkg.bin");
        let mut c = UpdateCheck::new();
        c.start_download(
            "v9.9.9".into(),
            format!("{base}/pkg.bin"),
            format!("{base}/SHA256SUMS"),
            "pkg.bin".into(),
            dest,
            Some(ExtraDownload {
                url: format!("{base}/helper.ps1"),
                sums_url: format!("{base}/SHA256SUMS"),
                asset: "helper.ps1".into(),
                dest: dir.join("helper.ps1"),
            }),
        );
        for _ in 0..500 {
            c.poll();
            if matches!(c.status(), UpdateStatus::Failed(_)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(c.status(), UpdateStatus::Failed(_)), "got {:?}", c.status());
        assert!(dir_entries(&dir).is_empty(), "no file or partial left behind");
    }

    #[test]
    fn windows_installer_extra_resolves_script_url() {
        let assets = vec![
            ReleaseAsset { name: "ebbtop.exe".into(), url: "http://x/e".into() },
            ReleaseAsset { name: "SHA256SUMS".into(), url: "http://x/s".into() },
            ReleaseAsset { name: INSTALL_PS1_ASSET.into(), url: "http://x/i".into() },
        ];
        let extra = windows_installer_extra("v0.3.0", &assets, "http://x/s")
            .expect("release carries the installer script");
        assert_eq!(extra.url, "http://x/i");
        assert_eq!(extra.sums_url, "http://x/s");
        assert_eq!(extra.asset, INSTALL_PS1_ASSET);
        assert_eq!(extra.dest, windows_installer_dest("v0.3.0"));
    }

    #[test]
    fn windows_installer_extra_missing_script_is_none() {
        let assets = vec![
            ReleaseAsset { name: "ebbtop.exe".into(), url: "http://x/e".into() },
            ReleaseAsset { name: "SHA256SUMS".into(), url: "http://x/s".into() },
        ];
        assert!(windows_installer_extra("v0.3.0", &assets, "http://x/s").is_none());
    }

    #[test]
    fn windows_installer_dest_names_versioned_script() {
        let p = windows_installer_dest("v0.3.0");
        assert_eq!(p.file_name().unwrap(), "ebbtop-update-v0.3.0-install.ps1");
        assert_eq!(
            p.parent(),
            download_dest_path("v0.3.0", "ebbtop.exe").parent(),
            "installer sits next to the exe download"
        );
    }

    #[test]
    fn download_dest_names_version_and_asset() {
        let p = download_dest_path("v0.3.0", "ebbtop.exe");
        assert_eq!(p.file_name().unwrap(), "ebbtop-update-v0.3.0-ebbtop.exe");
        let evil = download_dest_path("../../etc/passwd", "ebbtop.exe");
        let name = evil.file_name().unwrap().to_string_lossy();
        assert!(!name.contains('/'), "sanitized: {name}");
    }

    #[test]
    fn app_bundle_dir_finds_dot_app() {
        let exe = std::path::Path::new("/Applications/EbbTop.app/Contents/MacOS/ebbtop");
        assert_eq!(
            app_bundle_dir(exe).as_deref(),
            Some(std::path::Path::new("/Applications/EbbTop.app"))
        );
        assert_eq!(app_bundle_dir(std::path::Path::new("/usr/local/bin/ebbtop")), None);
    }

    #[test]
    fn check_is_due_until_interval_passes() {
        let mut c = UpdateCheck::with_interval(Duration::from_secs(60));
        assert!(c.due(), "never checked: due");
        c.start(|| Ok(r#"{"tag_name": "v99.9.9"}"#.into()));
        assert!(!c.due(), "running check: not due");
        assert!(poll_settled(&mut c), "finished check transitions");
        assert_eq!(
            c.status(),
            &UpdateStatus::Available { version: "v99.9.9".into(), assets: Vec::new() }
        );
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
        assert_eq!(
            c.status(),
            &UpdateStatus::Accepted { version: "v99.1.0".into(), assets: Vec::new() }
        );
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

    // --- Windows apply and restart (spec section 3, Windows) ---

    #[test]
    fn windows_apply_argv_preserves_autostart_flag() {
        let ps1 = std::path::Path::new("C:\\Tools\\install.ps1");
        let exe = std::path::Path::new("C:\\Temp\\ebbtop.exe");
        assert_eq!(
            windows_apply_argv(ps1, exe, true),
            ["powershell", "-ExecutionPolicy", "Bypass", "-File", "C:\\Tools\\install.ps1", "-Exe", "C:\\Temp\\ebbtop.exe", "-Autostart"]
                .map(String::from)
                .to_vec()
        );
        assert_eq!(
            windows_apply_argv(ps1, exe, false),
            ["powershell", "-ExecutionPolicy", "Bypass", "-File", "C:\\Tools\\install.ps1", "-Exe", "C:\\Temp\\ebbtop.exe"]
                .map(String::from)
                .to_vec()
        );
    }

    #[test]
    fn startup_shortcut_path_points_at_startup_folder() {
        let dir = std::path::Path::new("/cfg");
        assert_eq!(
            startup_shortcut_path(dir),
            std::path::PathBuf::from(
                "/cfg/Microsoft/Windows/Start Menu/Programs/Startup/EbbTop.lnk"
            )
        );
    }

    #[test]
    fn autostart_detected_from_shortcut_presence() {
        let dir = scratch_dir("autostart");
        assert!(!autostart_enabled_in(&dir), "no shortcut yet");
        let startup = dir
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup");
        std::fs::create_dir_all(&startup).expect("startup dir");
        std::fs::write(startup.join("EbbTop.lnk"), "link").expect("shortcut file");
        assert!(autostart_enabled_in(&dir), "shortcut present");
    }

    #[test]
    fn apply_windows_refuses_missing_files_before_spawning() {
        let dir = scratch_dir("applymissing");
        let err = apply_update_windows(&dir.join("ebbtop.exe"), &dir.join("install.ps1"))
            .expect_err("missing files must fail before any spawn");
        assert!(
            err.contains("ebbtop.exe") || err.contains("install.ps1"),
            "error names the missing file: {err}"
        );
    }

    // --- macOS staged bundle update (spec section 3, macOS) ---

    #[test]
    fn macos_backup_path_appends_bak() {
        let app = std::path::Path::new("/Applications/EbbTop.app");
        assert_eq!(
            macos_backup_path(app),
            std::path::PathBuf::from("/Applications/EbbTop.app.bak")
        );
    }

    #[test]
    fn macos_stage_dir_sits_alongside_bundle() {
        let app = std::path::Path::new("/Applications/EbbTop.app");
        assert_eq!(
            macos_stage_dir(app),
            std::path::PathBuf::from("/Applications/.ebbtop-update-stage")
        );
    }

    #[test]
    fn macos_unzip_argv_extracts_zip_to_stage_dir() {
        let zip = std::path::Path::new("/tmp/EbbTop.app.zip");
        let stage = std::path::Path::new("/Applications/.ebbtop-update-stage");
        assert_eq!(
            macos_unzip_argv(zip, stage),
            ["unzip", "-o", "-q", "/tmp/EbbTop.app.zip", "-d", "/Applications/.ebbtop-update-stage"]
                .map(String::from)
                .to_vec()
        );
    }

    #[test]
    fn macos_relaunch_argv_opens_bundle() {
        let app = std::path::Path::new("/Applications/EbbTop.app");
        assert_eq!(
            macos_relaunch_argv(app),
            ["open", "/Applications/EbbTop.app"].map(String::from).to_vec()
        );
    }

    #[test]
    fn apply_macos_refuses_missing_files_before_spawning() {
        let dir = scratch_dir("applymacosmissing");
        let err = apply_update_macos(&dir.join("EbbTop.app.zip"), &dir.join("EbbTop.app"))
            .expect_err("missing files must fail before any spawn");
        assert!(
            err.contains("EbbTop.app.zip") || err.contains("EbbTop.app"),
            "error names the missing file: {err}"
        );
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
