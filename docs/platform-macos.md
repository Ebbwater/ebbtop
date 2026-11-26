# EbbTop agent on macOS (Apple Silicon)

The published one-liner installs the same agent on Apple Silicon macOS that it
installs on Linux x86_64. This page is the macOS half of
[Linux agent distribution](../linux/README.md): what the installer does, what
is supported in this release, and — explicitly — what is not.

```sh
curl -fsSL https://github.com/Ebbwater/ebbtop/releases/latest/download/ebbtop-install.sh | bash
```

Nothing about that command changed. The script selects the package from
`uname -s` and `uname -m` before it touches the network, so a Mac fetches
`ebbtop-agent-macos-arm64.tar.gz` and a Linux box still fetches
`ebbtop-agent-linux-x86_64.tar.gz`.

## Supported hosts

| Host | Package | Service | Status |
|---|---|---|---|
| Linux x86_64, glibc ≥ 2.39, systemd user manager | `ebbtop-agent-linux-x86_64.tar.gz` | systemd user unit | existing behaviour, unchanged |
| macOS 13+, Apple Silicon (arm64) | `ebbtop-agent-macos-arm64.tar.gz` | per-user LaunchAgent | new in this release |

Refused, with one line and no network access: any other OS, any other
architecture, an Intel Mac, and a shell running under Rosetta (`uname -m`
reports `x86_64` on an arm64 Mac there — open a native Terminal or run
`arch -arm64 bash`).

## What the installer does

1. **Preflight, before any download.** On a Mac: `Darwin`, `arm64`, and a
   logged-in launchd user domain (`launchctl print gui/$UID`). A Mac reached
   only over SSH with no console login is refused here; this release does not
   install a pre-login launch daemon.
2. **Resolve the release** with the GitHub API, or `--version <tag>` to pin,
   or `--archive <path>` for a local tarball with no network at all.
3. **Download and verify.** The archive is refused unless its SHA-256 equals
   the digest the release API reports for that asset.
4. **Check the members, then extract.** Exactly five bare names
   (`BUILD`, `SHA256SUMS`, `ebbtop-agent`, `ebbtop-agent.plist`,
   `install-agent.sh`); a path or an extra member is refused before extraction.
   Inside the archive, `shasum -a 256 --strict -c SHA256SUMS` must pass.
5. **Choose the bind address.** An existing `agent.env` wins; otherwise
   `EBBTOP_ADDR`; otherwise a prompt defaulting to the Mac's Tailscale address
   (`tailscale ip -4`) or LAN address (`ipconfig getifaddr en0`).
6. **Run the packaged installer** from the extracted directory. It owns
   everything that changes the host.

Nothing is installed by the wrapper itself, so the Linux and macOS paths share
one set of checks rather than two that can drift.

## Where things live

| Path | What |
|---|---|
| `~/Library/Application Support/ebbtop-agent/bin/ebbtop-agent` | the agent |
| `~/Library/Application Support/ebbtop-agent/agent.env` | bind address, token and tuning, mode `0600` |
| `~/Library/Application Support/ebbtop-agent/ebbtop.token` | the token alone, mode `0600` |
| `~/Library/Application Support/ebbtop-agent/pairing.json`, `clients.json` | pairing state (created by `--pair` / `POST /pair`); `agent.env` pins `EBBTOP_STATE_DIR` to this directory so they land beside the token |
| `~/Library/LaunchAgents/com.ebbwater.ebbtop-agent.plist` | the LaunchAgent |
| `~/Library/Logs/ebbtop-agent/agent.log` | the agent's stdout and stderr |

Nothing here is system-wide. No `sudo`, no `/Library/LaunchDaemons`, no
LaunchAgent owned by another account.

`--uninstall --purge` deletes exactly the state directory above, so the token
and the pairing state go together. An agent built for macOS defaults its state
directory to the same path (`platform::default_state_dir`), so a `--pair` run
from a plain shell writes beside the token even before the first install pins it.

## Security behaviour on macOS

The macOS build **fails closed**, and that is not configurable:

- a **nonempty token is required**. The historical Linux behaviour ("no
  `EBBTOP_TOKEN` configured means no authentication required") is deliberately
  not available here: a laptop joins untrusted networks, and an agent that
  answers without a credential on a coffee-shop Wi-Fi is the failure this
  release exists to prevent. `EBBTOP_AGENT_REQUIRE_AUTH` is **ignored** on
  Darwin for exactly that reason;
- the **bind address must be loopback or private** — `127.0.0.1`, `10/8`,
  `172.16/12`, `192.168/16`, `169.254/16`, `100.64/10` (Tailscale/CGNAT),
  `fc00::/7`, `fe80::/10`. `0.0.0.0`, `::` and any public address are refused
  at startup and again by the installer before it writes anything;
- **broadcast discovery is off** in the packaged `agent.env`
  (`EBBTOP_AGENT_DISCOVERY=0`). The UDP responder is unauthenticated by design;
  the Mac pilot does not run it. Setting the variable explicitly turns it on
  for anyone who wants it.

The token lives in `agent.env` (mode `0600`), never in the LaunchAgent plist
and never on the process argument list. launchd has no `EnvironmentFile`
directive, so the plist passes `--env-file <path>`: a path, not a secret. That
file is read by the agent with the same narrow grammar
`deploy/install-agent.sh` validates on Linux (printable ASCII, `KEY=VALUE`,
`#`/`;` comments, last duplicate wins), and a malformed key or value is refused
with a message that never echoes the offending bytes — a pasted
`export TOKEN=...` line is exactly the kind of input that must not come back
out of the log.

## Upgrade, rollback, uninstall

```sh
# upgrade in place: keeps the token, the bind address and any settings
curl -fsSL .../ebbtop-install.sh | bash

# from an extracted package, on the Mac:
bash install-agent.sh --prebuilt     # install or upgrade
bash install-agent.sh --rollback     # restore the previous binary
bash install-agent.sh --status       # what is installed, and is it serving
bash install-agent.sh --uninstall    # stop it; keep token, bind, pairing state
bash install-agent.sh --uninstall --purge   # also delete the state directory
```

A failed upgrade restores **both** the previous binary and the previous
LaunchAgent, then verifies the restored install with an authenticated probe.
The two failure modes are reported separately: a launchd bootstrap that refuses
the job at all (`launchd bootstrap failed`) versus a job that loads but never
serves (`readiness probe failed`).

Readiness is an authenticated WebSocket exchange, not "the process exists":
`ebbtop-agent --probe <bind>:<port> --token-file <path>` sends the same
`get_metrics` request the desktop client sends and prints one line,
`probe: ok frames=N`, `probe: denied` (exit 3) or `probe: failed: ...`
(exit 1). It probes **the address in `agent.env`**, so a tailnet-bound install
is verified at its tailnet address and a local-only install at `127.0.0.1`.

## Metrics: what is reported and what is not

| Metric | macOS behaviour |
|---|---|
| CPU total / per core | reported (sysinfo, arm64) |
| Memory total / used, swap | reported |
| Load average (1/5/15) | reported (`getloadavg`) |
| Uptime | reported |
| Network throughput per interface | reported |
| Disk capacity per volume | reported, **APFS container counted once** |
| CPU model | reported (`Apple M3 Pro`-style brand string) |
| Process list: CPU, memory, user, state, start time | reported |
| Process detail: uid/gid, start time, CPU time (total) | reported |
| Process detail: user/system CPU **split** | not available: macOS exposes one accumulated total, reported in the user field with system as 0 |
| Per-disk throughput (`disk_io`) | **unavailable** — empty, Linux-only in this release |
| CPU / disk temperature | **unavailable** — `null`, not borrowed from another sensor |
| GPU (Apple GPU utilisation, VRAM) | **unavailable** for Apple Silicon in this release |
| cgroups | **unavailable** — no cgroups on macOS |
| Process logs (journald) | **unsupported**: an explicit "not available on this host" notice, not an error |
| Terminate a process (`kill`) | **unsupported on macOS**: "kill unsupported on this platform" |

Reported-as-unavailable is a deliberate choice over a guess: a client that
sees `null` or an empty list knows the platform cannot answer, while a
plausible-looking wrong number is worse than no number.

### APFS capacity

Every volume in an APFS container reports the *container's* total and free
space, and the read-only system snapshot plus the writable Data volume are two
views of one container. Listing them verbatim would report the same gigabytes
several times and disagree with Finder. The agent therefore drops
`/System/Volumes/*` and keeps one row per APFS container (identified by its
unique capacity pair), so `/` is reported once and a second container — a
case-sensitive volume, a USB stick, an HFS+ disk — is reported on its own. The
rule is `platform::keep_macos_volume`, unit-tested against the real Apple
Silicon volume layout.

## Startup at login

The LaunchAgent has `RunAtLoad` and `KeepAlive`, so it starts when the user
logs in and is restarted if it exits. It does **not** start before a login, by
design: a per-user agent must run as the user whose processes it reports, and
this release ships no pre-login launch daemon.

To reach the Mac's metrics from another machine, keep the Mac's Tailscale
client up and point the desktop client at the Mac's tailnet address:

```
ws://100.x.y.z:45976/ws        # the Mac's Tailscale IP, port 45976
```

Paste the token from `~/Library/Application Support/ebbtop-agent/ebbtop.token`
on the Mac into the client's source entry, or mint a short-lived pairing code
on the Mac with `~/Library/Application Support/ebbtop-agent/bin/ebbtop-agent --pair`
and enter that.

## What is verified, and where

| Check | Where | Status |
|---|---|---|
| macOS code compiles for `aarch64-apple-darwin` | Linux, `cargo check --target aarch64-apple-darwin` | run in this repository's CI and locally |
| Agent unit and integration tests | native arm64 (`cargo test --target aarch64-apple-darwin`) | `macos-agent` workflow |
| Package and installer fixtures (packaging, bind refusal, upgrade, rollback, activation failure, credentials, uninstall) | Linux with stubbed launchd **and** native | `deploy/macos/test-install-macos.sh` |
| Bash 3.2 compatibility of the bootstrap and installer | a real Bash 3.2 binary | `deploy/bootstrap/test-install-portable.sh` |
| Real foreground agent, authenticated probe, RAM/disk/process comparison against `sysctl`/`df`/a workload | native arm64 only | `deploy/macos/test-agent-live.sh` |
| launchd load/unload on a real Mac, login startup | native arm64 only | `macos-agent` workflow |
| Keychain / desktop client on the Mac | operator, unchanged | see the main README |

**A Linux run of any of these scripts is not native macOS evidence.** Where a
check needs launchd, a Mach-O binary or the Mac's own sensors, the script says
so and either runs under a stub (packaging and lifecycle logic) or skips with a
reason. The `macos-agent` workflow is what produces the native evidence, on
the exact commit the release is assembled from; `deploy/release/assemble.sh`
refuses a Mac archive whose `BUILD` stamp does not name that commit.

## Known gaps in this release

- **Code signing and notarisation are not done.** The agent is a bare
  executable inside a tarball, not an app bundle, so it is not subject to
  Gatekeeper's launch check; the LaunchAgent runs it as the user. If native
  testing shows macOS requiring a signature for this path, signing becomes a
  distribution decision and is not to be worked around with weaker Gatekeeper
  settings.
- **No Intel Mac support.** The archive is `arm64` only.
- **No pre-login startup.** A LaunchAgent needs a logged-in user.
- **No Apple GPU or thermal parity**, as listed in the metrics table.
- **Discovery is off by default**, so a Mac does not appear in the client's
  LAN scan; add it by tailnet/LAN address with the token or a pairing code.
