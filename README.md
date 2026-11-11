# EbbTop

Native btop-style window showing remote hosts live: CPU, memory, network,
disk and a sortable, filterable process table, with a per-process detail
inspector. 1 s tick over a plain WebSocket, no SSH. Run it on a tailnet,
VPN, or LAN; the token is the gate.

The client is multi-host with onboarding, Settings and a source picker
(token per host, kept in the OS credential store).

## Parts

| Part | Where | What |
|---|---|---|
| `crates/ebbtop-agent` | monitored Linux host | Owned agent. Same WebSocket wire protocol; `SOCKTOP_*` env names still accepted as fallback for `EBBTOP_*`. |
| `crates/ebbtop-core` | any OS | Wire decode, reconnecting poll loop, history and tick stats, view helpers, `ebbtop-probe`. No GUI types; a Tauri or TUI front end can reuse it. |
| `crates/ebbtop-gui` | Windows and Mac | `ebbtop` exe, egui 0.37 / eframe (glow). |
| `deploy/` | | Agent user unit and install script, agent-host acceptance script, Windows/macOS build scripts, log parsers. |

### Agent extensions (marked `ebbtop` in source)

1. `EBBTOP_BIND` / `--bind` (`SOCKTOP_BIND` still works): listen address.
2. `Processes.sampled_at_ms` (proto field 3): each process list carries its sample time.
3. `Process.user` (5) and `Process.state` (6) for the table.
4. Metrics JSON gains `load_avg`, `uptime_s` and `disk_io` (per whole disk, cumulative bytes from `/proc/diskstats`; loop, ram, zram and dm-* skipped so LVM is not double-counted).
5. GPU collection enabled (`--features gpu`) with gfxinfo's AMD/libdrm path off. NVML tries the versioned `libnvidia-ml.so.1` soname, then an optional explicit `EBBTOP_AGENT_NVML_LIB` path. `GpuMetrics` gains an additive `temperature_c` field (null when unavailable).
6. Phase 2 kill (see below): `Process.start_time_ms` (proto field 7) and a `kill:<pid>:<start_time_ms>:<TERM|INT>` request handler with journal audit.
7. `Process.cgroup` (proto field 8): the process's cgroup path, unified v2 `0::` path or the v1 `name=systemd` path, with the `0::` prefix stripped so both hierarchies look the same. Read once per pid and cached until the pid exits. `ebbtop-core` turns it into a workload with `view::workload_key`, which prefers the nearest systemd `.service` unit (a container's own unit on a quadlet host), then a `libpod-<id>` scope, then a `.scope`, else the host. `view::container_id` pulls the id out of a podman scope so a client can label it with a name and image; `view::group_by_workload` sums pids, CPU and RSS per workload. The path is data, not policy: grouping stays in the client so one agent serves every view.

New proto fields are additive. The client decodes a legacy agent too, with empty user/state/cgroup and no process timestamp.

### Contract

- Endpoint: `ws://<agent-host>:45976/ws?token=<token>`. Port 45976; check with `ss -tlnp`.
- Token: 32 random bytes hex, in `~/services/ebbtop-agent/agent.env` and `ebbtop.token` (mode 600). Client keeps one token per host in the OS credential store (service `ebbtop`, user = source name). `EBBTOP_TOKEN` env overrides for tests.
- Request/response, one in flight: `get_metrics` gives JSON with `sampled_at_ms`; `get_processes` gives protobuf (each row carries `cgroup`, field 8, for client-side workload grouping); `get_disks` gives JSON; Phase 2 `kill:<pid>:<start_time_ms>:<TERM|INT>` (on demand, queued between ticks on the same connection) gives a small JSON reply. Frames over 768 bytes are gzipped binary.
- Client tick: 1000 ms (`MissedTickBehavior::Skip`), disks every 5 ticks, 3 s reply deadline, reconnect backoff 0.5 s doubling to 4 s, forever.
- Agent metrics cache TTL: 100 ms (`SOCKTOP_AGENT_METRICS_TTL_MS`, upstream 250). In a 15 min soak at 250 ms, one delayed poll followed by a fast one got a cached repeat, giving one 2047 ms interval.
- Agent process cache TTL: 500 ms (`SOCKTOP_AGENT_PROCESSES_TTL_MS`). At 900 ms a 1 s poll sometimes got a cached repeat (process interval p95 2002 ms in the first smoke run), so 500 ms it is.
- Transport: plain `ws://` on a private network (tailnet, VPN, or LAN). The token travels in the query string, so do not expose the port to the open internet; the agent has no TLS listener.

## Install

Prebuilt binaries live on the [Releases page](https://github.com/Ebbwater/ebbtop/releases): `ebbtop.exe` + `ebbtop-probe.exe` + `install.ps1` (Windows), `EbbTop.app.zip` (macOS). The agent binary is built on the host itself by `install-agent.sh` (one Rust toolchain, no download).

### Agent host (Linux)

```
EBBTOP_ADDR=<this host's tailnet/LAN IP> ~/ebbtop/deploy/install-agent.sh          # build, install, enable, start; keeps an existing token
~/ebbtop/deploy/install-agent.sh --rollback
```

`EBBTOP_ADDR` is only needed for a fresh install (bind address for
`agent.env`). The unit is `~/.config/systemd/user/ebbtop-agent.service`:
`Restart=always`, `StartLimitIntervalSec=0`, so it rebinds when the address
appears after boot. Enable linger so it starts without a login
(`loginctl enable-linger $USER`).

A second host is one command (no Rust needed there — the binary ships from
your machine, x86_64 Linux targets only):

```
EBBTOP_SSH_OPTS="-i ~/.ssh/mykey" ~/ebbtop/deploy/install-agent-remote.sh user@host
```

It checks the arch, detects the tailnet bind address (or pass it explicitly),
installs and starts the user unit on port 45976, keeps an existing token,
and probes the agent before printing the client setup. Then add the host in
the client via Settings/source picker, one token per host
(`ssh user@host cat services/ebbtop-agent/ebbtop.token`).

### Windows client

Download `ebbtop.exe`, `ebbtop-probe.exe`, `install.ps1` and `SHA256SUMS` from the [Releases page](https://github.com/Ebbwater/ebbtop/releases) and compare hashes. In PowerShell on the desktop:

```
mkdir $env:TEMP\ebbtop; cd $env:TEMP\ebbtop
# (place the four downloaded files here)
Get-FileHash ebbtop.exe -Algorithm SHA256      # compare with SHA256SUMS
powershell -ExecutionPolicy Bypass -File install.ps1 -Exe .\ebbtop.exe -Autostart
```

Or fetch the same files from the build host with
`scp "<user>@<agent-host>:ebbtop/dist/*" .` (same hashes as the release).
Fetch the agent token with
`ssh <user>@<agent-host> cat services/ebbtop-agent/ebbtop.token | Set-Clipboard`.

Then click `EbbTop` on the Desktop: the onboarding wizard takes the host
URL and token once (Test checks the connection before saving).

Only build from source if you are changing the client (about 1 GB toolchain, no Visual Studio, no C dependencies in the Windows graph):

```
winget install Rustlang.Rustup                       # once
git clone <your-fork-or-mirror-url>
cd ebbtop
powershell -ExecutionPolicy Bypass -File deploy\windows\build.ps1 -Install -Autostart
```

`install.ps1` copies the exe to `%LOCALAPPDATA%\ebbtop\ebbtop.exe`, keeps `.bak`, makes `Desktop\EbbTop.lnk` (one click) and, with `-Autostart`, a Startup shortcut. No admin.

Config: `%APPDATA%\ebbtop\config.json` (sources, graph window, interface, disk pin, panels). Log: `%LOCALAPPDATA%\ebbtop\ebbtop.log`, one stats line a minute plus every disconnect, rotated at 2 MiB.

### macOS client

Easiest: download `EbbTop.app.zip` from the [Releases page](https://github.com/Ebbwater/ebbtop/releases), unzip, and `open EbbTop.app` (or drag it to /Applications; first launch may need right-click > Open — the bundle is ad-hoc signed). Then onboard with the host URL and token as below.

Or build from source. Same workspace, no VM and no exe change: `ebbtop` builds natively on Apple
Silicon (`aarch64-apple-darwin`). `ebbtop-core` has no
Windows-only APIs (tokio/tungstenite, prost, serde, flate2 only); the only
OS-specific code in `ebbtop-gui` is the `windows_subsystem` cfg (a no-op on
macOS) plus the `keyring` v4 crate, which already maps to the macOS Keychain
(service `ebbtop`, one entry per host), and `dirs` for `~/Library/Application
Support/ebbtop/` (config.json, ebbtop.log). `cargo check --target
aarch64-apple-darwin` is clean on this branch (type-check only, not linked).

On the Mac:

```
git clone <your-fork-or-mirror-url>
cd ebbtop
bash deploy/mac/check.sh        # rustup, cargo check (Mac target), cargo test, release build
```

That produces `target/release/ebbtop` (and `ebbtop-probe`). Fetch the token with
`ssh <user>@<agent-host> cat services/ebbtop-agent/ebbtop.token | pbcopy`, launch
`./target/release/ebbtop`, and the onboarding wizard takes it from there. No
`.dmg`; run the binary directly.

## Window

- Status bar: `LIVE` / `STALE` (no new sample for 2 s or more) / `CONNECTING` / `DOWN` with reason and retry time; agent sample time; age; tick p95; reconnect count. Hover age for lag and the full stats line.
- Staleness uses only the desktop clock (time since a *new* sample arrived), so clock drift between the machines cannot cause a false STALE. Lag (agent sample time to now) needs synced clocks and is shown as information only.
- CPU graph plus per-core meters, temperature, load average and uptime; memory and swap; network rx/tx rates per interface (busiest first, `lo` last); disk usage and per-disk read/write rates; 60 s or 5 min window.
- GPU panel when the agent reports a GPU: name, utilization history graph, load meter, VRAM used/total and edge temperature. Omitted entirely on hosts with no usable GPU.
- Processes: PID, name, user (root in amber), state letter, CPU%, MEM%, RSS. Click headers to sort; filter matches name, user or PID; Esc clears. CPU% is the share of the whole machine (btop's default, not per core).
- Workloads: a roll-up pane above the process table groups the sample by the cgroup boundary that owns each process (the container's own systemd unit, a `libpod-<id>` scope for rootless containers with no unit, a transient scope, or the host) with PID count, CPU% and RSS summed, biggest memory first. Click a workload to filter the table to it; click it again, or the chip in the proc header, to clear. The process table also carries a Workload column. An agent without proto field 8 reports no cgroup, so everything lands in `(host)`; the pane is skipped when the window leaves less than 240 px for it so the table keeps its rows.
- Selecting a row opens the detail inspector: full command with Copy, CPU time, vmem/shared, start time, cwd, exe, children (click to jump), top threads, and CPU+RSS history graphs.
- Sources: the status-bar picker switches hosts (token per host); Settings manages sources, display and connection. The disk graphs follow the busiest disk or a pinned one.
- At under 1100 px wide it switches to a stacked layout. The upper boxes scroll so the process table keeps about 45% of the height down to the 480x420 minimum.

## Evidence

| Check | Result | How |
|---|---|---|
| Agent tests | 25 passed (includes proc parsers) | `cd crates/ebbtop-agent && cargo test --release --locked --no-default-features` |
| Client tests | 17 core + 4 GUI passed | `cargo test --workspace` |
| Agent killed 10 s, restarted | 1 reconnect, data back 2.5 s after the agent returned (12.5 s gap), no token prompt | 60 s `ebbtop-probe` on loopback |
| Agent frozen 15 s (SIGSTOP) | `no reply within 3000 ms`, then `connect timed out` retries, recovered on SIGCONT (17.5 s gap) | 45 s `ebbtop-probe` on loopback |
| Process freshness at TTL 500 ms | process interval p50 1000 / p95 1091 ms, 0 repeats | same runs |
| Loopback 15 s probe, patched agent | metrics interval p50 1000 / p95 1093 ms, lag p95 1 ms, 0 gaps, 0 repeats; wrong token rejected; token absent from agent and probe logs | `ebbtop-probe` against `127.0.0.1:45977` (TTL 900 ms in that run) |
| Real render | PNGs at 960x1040, 1920x1080, 640x520 reviewed; fixed meter text overlap, header truncation, a picker with no accessible name, and an unusable table at small heights | `EBBTOP_SHOT_DIR=... cargo test -p ebbtop-gui screenshot -- --ignored` (egui_kittest + wgpu, software Vulkan) |
| Frame cost | 0.69 ms p50, 1.04 ms p95 CPU per frame, 400 processes, 960x1040 | `cargo test --release -p ebbtop-gui frame_cost -- --ignored --nocapture` |
| Windows target | `cargo check --target x86_64-pc-windows-gnu` clean | type-check only; not linked here |
| Mac target | `cargo check --target aarch64-apple-darwin` clean | type-check only |
| Linux exe size | 12.3 MB stripped | `target/release/ebbtop` |
| Agent service live | `ebbtop-agent.service` active, listening on the tailnet/LAN address only; loopback refused; wrong token rejected; `agent.env` and `ebbtop.token` mode 600 | `deploy/install-agent.sh`, 2026-11-07 |
| Private-network probe, 30 s, agent host to its own address | metrics interval p50 1000 / p95 1097 / max 1106 ms; process interval p95 1101 ms; 0 gaps, 0 repeats, 0 reconnects | `deploy/acceptance-kserver.sh 30` |
| 15 min soak, live agent over tailnet, metrics TTL 100 ms | 900 frames, max interval 1487 ms, 0 gaps over 2000 ms, 0 repeats, 0 disconnects; agent RSS about 8 MB, 27 s CPU (about 3% of one core) | `ebbtop-probe --seconds 900`, 2026-11-07 10:38-10:53 |
| Windows exe | `ebbtop.exe` 8.97 MB (GUI subsystem), `ebbtop-probe.exe` 1.38 MB; imports only system DLLs | `deploy/cross-build-windows.sh`; hashes in `dist/SHA256SUMS` |
| First run on the Windows desktop | window LIVE, data matches the agent host. Agent side: one established connection from the desktop to :45976. First build had no clipboard (paste into Token did nothing); fixed in `621eba8` (eframe `clipboard` feature) | operator report, `ss -tn state established sport = :45976`, 2026-11-07 |
| GPU path | Agent builds with `--features gpu` and no libdrm; agent tests 25 passed; core 19 + GUI 5 passed; synthetic renders at 960x1040, 1920x1080 and 640x520 show the panel. Live before reboot: cached 595.91.07 NVML pinned, `gpu: NVIDIA GeForce GTX 1650 util=0% vram=381 MiB/4.0 GiB temp=54C` over the private network. Panel visually confirmed on the desktop after installing `ebbtop.exe` `e6a13225…` | `cargo build/test` above; `LD_LIBRARY_PATH=.../nvml-595.91.07 nvidia-smi`; `ebbtop-probe --seconds 5`; operator report, 2026-11-08 |
| Phase 2 kill | Agent test incl. real SIGTERM to an owned `sleep` child (stale-start refusal first, child alive until correct start_time); journal lines verified via `journalctl -g "ebbtop kill"`; core 22 + GUI 11 passed (kill grammar/decode, dialog text, status-bar result); frame cost 0.59/0.62 ms p50/p95; Windows check clean. No live agent was restarted and nothing was deployed | `cargo test --workspace`; `cd crates/ebbtop-agent && cargo test --release --locked --no-default-features`; `cargo test --release -p ebbtop-gui frame_cost -- --ignored --nocapture`; `cargo check --target x86_64-pc-windows-gnu`, 2026-11-08 |

| EbbTop 0.2.0 suite (`ebbtop-distributable`) | core 36 + GUI 59 passed, 2 ignored; no warnings | `cargo test --workspace`, 2026-11-08 |
| EbbTop 0.2.0 targets | `cargo check --target x86_64-pc-windows-gnu` and `--target aarch64-apple-darwin` clean | type-check only, 2026-11-08 |
| EbbTop 0.2.0 renders | PNGs at 960x1040, 1920x1080, 640x520 plus detail, onboarding and settings shots reviewed; fixed a nameless password field, thread `?` states, and a hardcoded host fallback | `EBBTOP_SHOT_DIR=... cargo test -p ebbtop-gui screenshot -- --ignored`, 2026-11-08 |
| EbbTop 0.2.0 live probe | 15 s over tailnet: 1 s ticks, GPU line, process list, no errors | `ebbtop-probe --seconds 15`, 2026-11-08 |
| EbbTop 0.2.0 frame cost | 0.63 ms p50, 0.67 ms p95 CPU per frame, 400 processes, 960x1040 | `cargo test --release -p ebbtop-gui frame_cost -- --ignored --nocapture`, 2026-11-08 |
| EbbTop 0.2.0 Linux exes | `ebbtop` 15.8 MB, `ebbtop-probe` 1.6 MB, stripped, no warnings | `cargo build --release`, 2026-11-08 |
| EbbTop acceptance-kserver, 30 s (on the agent host) | agent `active`, listening on its tailnet/LAN address only; 30/30 distinct samples, 0 repeats, 0 gaps, 0 reconnects; metrics interval p50 1000 / p95 1210 / max 1327 ms; GPU line present (`GTX 1650 ... temp=58C`) | `deploy/acceptance-kserver.sh 30`, 2026-11-09 |
| EbbTop macOS native build (Apple Silicon, aarch64) | `deploy/mac/check.sh` clean; first launch: wizard took `ws://server.ebbwater.net:45976/ws` + token cleanly, `LIVE`, hovered `interval_p95` = 1004 ms, no visual glitches | operator report, 2026-11-09 |
| EbbTop macOS bundle + Keychain | `deploy/mac/package.sh` builds `dist/EbbTop.app` (new-mark Dock icon, launches with no terminal); quit-and-relaunch reconnects with no token prompt (Keychain save under service `ebbtop` confirmed by the one-time access prompt) | operator report, 2026-11-09 |
| EbbTop brand fold-in | pack vendored under `crates/ebbtop-gui/assets/brand/`; window icon bytes replaced (new rgba byte-identical to the pack's 64 px PNG decode; Pillow `tobytes` is straight alpha, matching `egui::IconData`); `build.rs` links the pack's 8-frame `ebbtop.ico` instead of generating a 64-only one; new `deploy/mac/package.sh` assembles `dist/EbbTop.app` (iconutil `.icns` from the hinted PNG ladder, ad-hoc sign fail-soft). Suite still green: core 36 + GUI 59, 2 ignored; Windows check clean (soft-fail warning path exercised) | `cargo test --workspace`, `cargo check --target x86_64-pc-windows-gnu`, 2026-11-09 |
| Owned ebbtop-agent cutover | `agent/socktop_agent` deleted; `crates/ebbtop-agent` live via `deploy/install-agent.sh`: unit `active`, listening on its tailnet/LAN address only. Agent tests 15 + 7 integration pass; loopback probe (existing client, unchanged) clean; token gate verified (right token data, wrong token no samples); 30 s tailnet acceptance 30/30 samples, metrics p50 1000 / p95 1002 ms, GPU line present (`GTX 1650 ... temp=48C`), top ground truth matches. Old `socktop_agent` binary kept at `$SVC/bin/` for `--rollback` only | `cargo test -p ebbtop-agent`, `deploy/install-agent.sh`, `deploy/acceptance-kserver.sh 30`, 2026-11-09 |

Done on the Windows desktop: steps 1–6 pass (reboot launch, stacked layout, LIVE in 1 s, p95 1138 ms, btop match, no-SSH LIVE, reboot reconnect with no token prompt). Step 7 (overnight log) is still pending.

## Acceptance (brief steps 1 to 7)

1. Reboot desktop, click `EbbTop` on the Desktop.
2. Snap to half screen. The stacked layout applies below 1100 px.
3. Within 10 s: `LIVE`. Hover age for `interval_p95`; pass is under 2000 ms (1 s design target).
4. Accuracy: compare with btop on the agent host (its default CPU% is share of the whole machine, like EbbTop). Or run `deploy/acceptance-kserver.sh 30`, but note `top` shows per-process %CPU per core: divide by the host's thread count to compare. Pass: total CPU and memory % within 5 points, same top name/PID with EbbTop sorted by CPU%.
5. No SSH windows open; still `LIVE`.
6. Reboot, one click, reconnects with no token prompt.
7. Overnight: next morning check `%LOCALAPPDATA%\ebbtop\ebbtop.log`. Each minute line has `gaps=`, `reconnects=`, `interval_p95=`. A `disconnected` line during sleep followed by recovery is expected.

## Remaining acceptance

Only step 3 is evidenced so far (LIVE after launch, see Evidence). Steps 1, 2,
4, 5, 6, 7 are not yet recorded. Run
`powershell -ExecutionPolicy Bypass -File deploy\acceptance-desktop.ps1` on the
desktop: it prints the step 7 log summary, the step 4 btop snippet, and this
checklist. Tailnet-only: it reads the local log, never prints the token, and
opens nothing.

| Step | Status | How to record it |
|---|---|---|
| 1. Reboot desktop, one-click launch | Pass (2026-11-09) | Reboot, click `EbbTop`. Note whether it launches with no dialog, date/time |
| 2. Snap to half screen | Pass (2026-11-09) | Snap under 1100 px; note the stacked layout applies |
| 3. LIVE within 10 s, `interval_p95 < 2000 ms` | Pass: LIVE in 1 s, p95 1138 ms (2026-11-09) | Hover age after step 1/6 and note the value |
| 4. btop accuracy | Pass (2026-11-09) | `btop` on the agent host side by side with EbbTop, or `deploy/acceptance-kserver.sh 30`; `top` numbers are per-core, divide by the host's thread count. Record total CPU/mem deltas (pass within 5 points) and top name/PID match with EbbTop sorted by CPU% |
| 5. No SSH windows open, still LIVE | Pass (2026-11-09) | Close every SSH window first, then note LIVE |
| 6. Reboot, one-click reconnect, no token prompt | Pass (2026-11-09) | Reboot, one click; reconnected with no token prompt, or not |
| 7. Overnight log check | Not recorded | Next morning run the helper above (or `deploy/parse-ebbtop-log.sh <log>` on a copied log). Paste the VERDICT plus `gaps`/`reconnects`/`interval_p95` maxima. A `disconnected` line during sleep with recovery is expected |

NVML pin note (resolved 2026-11-09): the host rebooted onto 595.99.02,
the pin was deleted from `agent.env`, and the agent reports the GPU through
the normal `libnvidia-ml.so.1` soname (`gpu: NVIDIA GeForce GTX 1650 ...
temp=58C` over the tailnet probe).

## Known gaps

- If the host's loaded NVIDIA module ever drifts from its installed libs (`nvidia-smi` reports a driver/library mismatch), the normal `libnvidia-ml.so.1` lookup fails; pin a known-good library via `EBBTOP_AGENT_NVML_LIB` in `agent.env` until the next reboot, then delete the pin.
- Only `device_by_index(0)` is queried; a multi-GPU host would show the first GPU only.
- GPU VRAM "used" comes from NVML v1, which includes the driver-reserved pool (about 379 MiB idle on this 4 GiB card). `nvidia-smi`'s "Used" excludes it; btop uses the same v1 call, so the panel matches btop.
- Upstream token check is a plain string compare and the token travels in the query string. Acceptable inside the tailnet; noted.

## Phase 2 kill: implemented

- Request, on demand only (never per-tick): `kill:<pid>:<start_time_ms>:<TERM|INT>`, nothing else. The agent refuses any other `kill…` text with an error reply and a journal line.
- Agent checks, in order (token auth is the existing ws upgrade; unchanged transport, plain string compare, token in query string): pid is not 1, not the agent's own pid, not a kernel thread (ppid 0); process uid equals the agent uid (`kyler`), so root-owned processes are refused; `start_time_ms` matches the process's current start time (PID-reuse guard; the process table carries it as protobuf field 7, computed from `btime` + `/proc/<pid>/stat` starttime); then the signal is sent with `pidfd_open` + `pidfd_send_signal` (raw `syscall` instruction, no new deps). Non-Linux returns `kill unsupported on this platform`.
- Reply: small JSON text frame `{"ok":bool,"pid":n,"signal":"TERM|INT","error":null|"reason"}`. Stable reasons: `bad request grammar`, `no such process`, `pid 1`, `agent itself`, `kernel thread`, `uid mismatch`, `stale start_time`, plus `pidfd_open: …` / `pidfd_send_signal: …` on syscall failure.
- Journal: every request and its result is written to journald over the native journal socket (`/run/systemd/journal/socket`, priority 6; stderr fallback), one line per event: `ebbtop kill request pid=… start_time_ms=… signal=…`, `ebbtop kill result pid=… signal=… ok=true|false err="…"`, and `ebbtop kill request raw="…" ok=false err="…"` for malformed requests. Query with `journalctl --no-pager -g "ebbtop kill"`. The token never appears in any log.
- Client: the GUI queues the job on a channel and the client sends it between ticks on the same connection (one request in flight; a kill reply timeout drops the connection, reconnecting to re-establish frame alignment). The Kill button is disabled for root-owned rows and for rows with no reported start time (unpatched agent). The confirm dialog names the process, pid and host (`Send SIGTERM to "…"?`, buttons Send TERM / Send INT / Cancel); the result shows in the status bar (`kill <pid> TERM sent` / `kill <pid> refused: <reason>`) and in `%LOCALAPPDATA%\ebbtop\ebbtop.log`. Tailnet-only; no new public exposure.
