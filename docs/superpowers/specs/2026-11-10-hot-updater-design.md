# EbbTop GUI hot updater — design (2026-11-10)

Approach A (approved 2026-11-10): in-app update reusing the existing
installers. Prompt-to-restart, Windows + macOS, stable GitHub Releases
only. GitHub Releases stay the source of truth; releases are cut by hand
from local git. No release automation exists in the repo today.

## 1. Version check

- Current version: `env!("CARGO_PKG_VERSION")` at compile time (workspace
  `0.2.0`). The GUI does not read its own version today; this adds it.
- Check: `GET https://api.github.com/repos/Ebbwater/ebbtop/releases/latest`
  on launch and every 24h, plus a manual Check for updates button in
  Settings. Compare semver after stripping a leading `v`.
- The `/latest` endpoint already excludes drafts and prereleases, so the
  stable-only rule needs no extra code.
- A failed check never blocks startup: status-bar note + log line, retry
  at the next interval. Unauthenticated rate limit (60/hr) fits a daily
  check.

## 2. Download and verify

- On user approval, download the platform asset to temp with progress in
  the dialog: Windows `ebbtop.exe`, macOS `EbbTop.app.zip`.
- Fetch `SHA256SUMS`, parse the line for the asset, compare hashes. On
  mismatch: delete the download, show a hard error, log. Never touch the
  running install before verification passes.
- `ebbtop-probe.exe` is out of scope: `install.ps1` only installs the GUI
  exe. Downloads run on the existing tokio runtime, never the UI thread.

## 3. Apply and restart

- Windows: run
  `powershell -ExecutionPolicy Bypass -File install.ps1 -Exe <download>`
  detached, then quit. The script already kills the process, keeps
  `.bak`, and swaps the exe. Preserve Autostart by passing `-Autostart`
  when the Startup shortcut exists.
- macOS: unzip alongside, swap the running bundle keeping a `.bak` copy
  of the old app, relaunch with `open`, quit.
- Known wrinkle: API downloads carry `com.apple.quarantine`, so first
  relaunch may need the same right-click Open as manual installs (or an
  `xattr -d` strip — TBD during implementation).
- Rollback stays the existing `.bak` scripts on both platforms. The
  updater never deletes `.bak`.

## 4. Errors and testing

- Network failure: quiet note + log, retry next daily check.
- Hash mismatch / apply failure: dialog error + log, running install
  untouched, `.bak` rollback path pointed at.
- No credentials involved. New network surface is `api.github.com` plus
  release artifact downloads only.
- Tests: unit (semver compare with leading `v` and unequal lengths,
  `SHA256SUMS` parsing, per-platform asset picks); GUI (dialog states,
  Save/Restart button enablement); manual acceptance old-build →
  prompt → restart → new version running, on both platforms.

## Acceptance

1. On an outdated build, launch shows an update prompt naming the new
   version within one check interval.
2. Accept → verified download (hash line in log) → restart → new
   version running, config/sources/token intact.
3. Decline or failed check → app runs as before, note in log.
4. `cargo test --workspace` green, no warnings.
