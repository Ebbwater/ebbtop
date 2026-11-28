# EbbTop 0.3.1

The monitoring agent for Apple Silicon Macs, with the same installer as Linux.

- macOS agent (Apple Silicon). The same `ebbtop-install.sh` now installs on
  Linux x86_64 and macOS arm64: it resolves the release, verifies the agent
  archive's sha256 against the digest the release reports, and runs the
  packaged installer — a per-user LaunchAgent on macOS (starts at sign-in;
  token required; listens on a private or loopback address), the systemd user
  service on Linux as before. Network discovery is off by default on macOS;
  add the agent manually in the desktop app with its private address and
  token. The macOS agent reports CPU, memory, disk, network and processes;
  Linux-specific extensions and Apple GPU/temperature metrics are outside
  this release. See `docs/platform-macos.md` for what the platform reports
  and what it deliberately reports as unavailable.
- Shared installer (`deploy/bootstrap/ebbtop-install.sh`). One script, both
  platforms, with the same curl|bash safety properties: preflight before any
  network, digest taken from the GitHub API and enforced, download URL
  allowlist, tar member-set check before extraction, inner `SHA256SUMS
  --strict -c`, glibc floor re-read from BUILD on Linux. Updating a Linux
  agent is a re-run of the installer; it preserves the existing bind address
  and credentials. Existing agents keep working with their current tokens.
- Release assembly is fail-closed on the new platform: `assemble.sh` refuses
  to assemble a release set without the macOS agent run, so a forgotten macOS
  build stops the release instead of shipping half of it.

Fixes on the way to this release: native macOS foreground qualification, the
disk-capacity check for renamed boot volumes, and macOS upgrade credentials
no longer passed through subprocess arguments.

Upgrading: rerun the installer on a Linux agent to move to this version (it
preserves bind address and credentials); on macOS run the installer on the
new host. Desktop clients on the ZIP channel update in-app as usual. The
desktop app itself is unchanged in this release — no `crates/ebbtop-gui`
change ships here; the Windows build and the MSIX carry the 0.3.0 feature
set rebuilt at this version.
