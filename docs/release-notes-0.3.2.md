# EbbTop 0.3.2

The macOS desktop app ships on Releases for the first time, and the agent's
network surface fails closed.

- macOS desktop build. Every release from this one on carries `EbbTop.app.zip`,
  built and packaged on Apple Silicon CI and verified at assembly time against
  the tagged commit (Info.plist version, arm64 binary, bundle shape, digest
  sidecar). The desktop updater on macOS downloads exactly this asset name and
  verifies it against the release's `SHA256SUMS`, so in-app updates work on
  macOS for the first time. The bundle is ad-hoc signed: first launch may need
  System Settings > Privacy & Security > Open Anyway (macOS Sequoia removed
  the right-click > Open bypass, and the docs now say so).
- Third-party license attribution. `THIRD-PARTY-NOTICES` ships beside
  `ebbtop.exe` (loose and in the ZIP), inside the MSIX, and inside both agent
  archives: every crate compiled into the shipped binaries, with each license
  text. Generation fails on any license outside the accepted list, so a
  dependency changing its license is a reviewable event.
- Agent, fail closed on a missing token. An agent started with no
  `EBBTOP_TOKEN` serves read-only only (the kill path is unreachable) and
  refuses to start on any bind address except loopback, so a bare `cargo run`
  keeps working locally while a tokenless agent can never answer on a
  network. Every shipped install path mints a token and is unaffected.
- Agent, credential and audit hardening. The manual token's compare is
  constant time (the helper `/pair` already used); a kill attempt from a
  read-only (paired) client is now journalled like every other kill-shaped
  frame, so the operator's audit trail covers all of them.
- Agent, kill checks run twice against the pinned pid. `pidfd_open` pins the
  pid first, `/proc` is re-read and the same checks re-run before the signal
  is sent — a pid recycled between the first read and the pin is refused, not
  signalled.
- Agent, `/pair` is rate-limited. One exchange per second per source address
  (and a global cap), with 429 refused before any state write, and the
  exchange itself runs on the blocking pool, so a flood of wrong guesses can
  no longer stall the metrics path with fsyncs. Labels are capped at 128
  characters.
- Desktop, IO churn fixed. Config is written once per splitter drag (on
  release) instead of per frame, and the update check time is persisted only
  when a check actually finished — not during download progress.
- Release engineering. CI now gates on `cargo clippy -D warnings` and
  `cargo fmt --check` (with the components self-provisioned on the runner);
  the systemd unit gains `MemoryDenyWriteExecute`, `RestrictSUIDSGID`,
  `RestrictRealtime` and an empty capability bounding set;
  `docs/SIGNING.md` records the 0.3.1 certification result and the published
  MSIX digest.

Upgrading: Windows clients on the ZIP channel update in-app as usual;
settings and stored credentials are preserved and the previous executable is
kept as a `.bak`. On macOS, download `EbbTop.app.zip` and replace the app
(from this release forward the app offers in-app updates); the agent is
updated by rerunning the installer, which preserves the bind address and
credentials. Existing agents keep working with their current tokens; a
tokenless agent, if anyone runs one, now serves read-only only.
