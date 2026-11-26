# EbbTop prebuilt agent installer (`ebbtop-install.sh`)

`deploy/bootstrap/ebbtop-install.sh` is the published one-liner. It installs
the prebuilt agent on **Linux x86_64** (systemd user unit) and on **macOS
Apple Silicon** (per-user LaunchAgent) from the same file, choosing the package
from `uname -s` and `uname -m` before it touches the network.

```sh
curl -fsSL https://github.com/Ebbwater/ebbtop/releases/latest/download/ebbtop-install.sh | bash
```

It is a fetch-and-verify wrapper and installs nothing itself: the install, the
service definition, the token, the login/linger guidance and the self-check all
belong to the packaged `install-agent.sh --prebuilt` from the release archive,
which ships in that archive byte-identical to this repository. The wrapper only
proves that the bytes it hands over are the bytes the release published.

## Order of operations

1. **Platform.** `uname -s` + `uname -m` select the archive; anything else is
   refused with one line and no network access. A Rosetta shell on an arm64 Mac
   is named precisely (`arch -arm64 bash`) instead of being silently served the
   wrong package.
2. **Preflight.** Linux: `x86_64`, glibc ≥ 2.39, a systemd user manager.
   macOS: `arm64`, a logged-in launchd user domain.
3. **Release.** `latest` via the GitHub API, `--version <tag>` to pin,
   `--archive <path>` for a local tarball with no network at all.
4. **Verify.** The archive's SHA-256 must equal the digest the API reports.
   Member names are checked **before** extraction (a traversing or unexpected
   name is refused), then the inner manifest is verified with
   `sha256sum --strict -c SHA256SUMS` on Linux or
   `shasum -a 256 --strict -c SHA256SUMS` on macOS.
5. **Bind.** An existing `agent.env` wins; otherwise `EBBTOP_ADDR`; otherwise a
   prompt defaulting to the host's tailnet/LAN address. The packaged installer
   makes the final decision about whether that address is acceptable.
6. **Install**, then **verify** what landed and print the pairing facts
   (`ws://<bind>:<port>/ws` and the token path — never the token).

## Why one file

Two per-platform copies of these checks drift, and the drift is invisible until
a release ships a package one platform's wrapper cannot verify. The only
platform-specific code here is a `case` on `PLATFORM` in four places: the
package facts, the preflight, the default-address probe, and the final report.
Everything security-relevant — digest, members, manifest, bind validation, the
delegation to the packaged installer — is one implementation both platforms use.

## Bash 3.2

macOS ships Bash 3.2 (a 2007 build) as `/bin/bash`, and this file must run under
it, so it uses no `mapfile`, no associative arrays, no `${var,,}`, and no
`${array[@]}` that could be empty under `set -u`. `test-install-portable.sh`
runs the whole macOS path and a real install under a genuine Bash 3.2 binary.

## Tests

| Script | Covers | Runs on |
|---|---|---|
| `test-install-portable.sh` | platform selection, refusals, dry run, fresh install, probe targeting, upgrade, rollback (binary + plist), activation failure, credentials, binds, lifecycle, rollback command, Bash 3.2 | Linux; the macOS path via `--platform macos` with stubs, and natively in the `macos-agent` workflow |
| `test-negative-controls.sh` | that the harness above FAILS for each injected regression (loopback-hardcoded probe, no plist rollback, activation without rollback, no credential pre-check, token rotation on upgrade, silent no-op installer) | Linux |
| `../linux/test-install-latest.sh` | the Linux path, unchanged in its assertions | Linux |
| `../macos/test-install-macos.sh` | the packaged macOS installer: package refusals, upgrade, rollback, credentials, binds and port ownership, uninstall | Linux with stubs, and natively |

A passing harness proves nothing unless it can fail: `test-negative-controls.sh`
is what makes the green results meaningful.

## Limits of the Linux-run evidence

Running the macOS path on Linux with stubbed `launchctl`/`lsof`/`plutil` proves
the packaging, the verification rules, the bind validation, the lifecycle
sequencing and the failure attribution. It does **not** prove that launchd
loads the job, that the agent binds on a real Mac, or that the Mac's sensors
report what the client expects. The native `macos-agent` workflow runs
`../macos/test-agent-live.sh` for foreground networking and metrics. A separate
logged-in Mac test verifies real launchd operations and startup at login.
