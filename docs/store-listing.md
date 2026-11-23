# EbbTop — Microsoft Store listing pack

Everything Partner Center asks for, ready to paste. The identity values were
issued when the app name was reserved, and are the same ones in
`deploy/windows/msix/AppxManifest.xml`:

| Field | Value |
|---|---|
| Package/Identity Name | `Ebbwater.EbbTop` |
| Package Family Name | `Ebbwater.EbbTop_46f4w9xy8kqdr` |
| Publisher | `CN=32700780-5AAE-4D15-A591-B49543B86C16` |
| Publisher display name | `Ebbwater` |
| Store ID | `9PJJXH1SBQ23` |
| Package | `EbbTop-<version>-x64.msix` (x64, packaged Win32 desktop app) |

These are public distribution identities, not secrets.

## Languages

`deploy/windows/msix/AppxManifest.xml` declares three resource languages,
matching the three UI languages the desktop client ships:

| Manifest tag (BCP-47) | UI language | Stored in-app setting (`language`) |
|---|---|---|
| `en-US` | English (default and fallback) | `en` |
| `zh-CN` | Simplified Chinese (Mainland) | `zh-Hans-CN` |
| `zh-TW` | Traditional Chinese (Taiwan) | `zh-Hant-TW` |

The UI language is chosen inside the app (Settings -> Display), not from the OS
locale: the translations are compiled into `ebbtop.exe` and the package carries
no PRI resource, so the declared languages describe the client's capability to
the Store rather than switching the app by system language.

**The listing text in this document is English only.** Product name, short
description, description, feature list and screenshot captions have not been
translated, and translated Store listing copy (including any Chinese listing)
is out of scope for v0.3.0. Submit the English listing; the extra package
languages add language coverage, not translated storefront text.

## Product name

`EbbTop`

## Short description (max 270 characters)

```
Monitor your own Linux hosts from Windows: CPU, memory, network, disk and a sortable, filterable process table with per-process detail. One small agent per host, a WebSocket over your tailnet, VPN or LAN, and a token you hold. No cloud, no telemetry.
```

## Description

Written in the README's "Start Here" voice: what it does, then what you need.

```
EbbTop is a native, btop-style monitor for the Linux machines you already run.

Point it at your own hosts and watch them live: CPU with per-core meters and temperature, memory and swap, network and disk throughput, GPU load and VRAM, plus a sortable, filterable process table with a per-process detail inspector (command line, working directory, children, threads, CPU and memory history). Processes are grouped by workload, so a systemd service or a container shows up as one row with its own totals.

It is built for a handful of machines you own, not a fleet in someone's cloud.

HOW IT WORKS

A small agent runs on each Linux host. It answers one WebSocket with metrics, the process table and on-demand process detail. The desktop app connects to as many hosts as you like, keeps one token per host in the Windows credential store, and reconnects on its own.

Pairing takes a minute. Run `ebbtop-agent --pair` on the host, type the six-digit code it prints into EbbTop, and the app stores the read-only token it issues. Static addresses and tokens work too, for hosts on a tailnet or another subnet.

WHAT IT DOES NOT DO

- No cloud account, no sign-in, no telemetry. The app talks only to the hosts you configure.
- Read-only monitoring. The one action it can take is an explicit, confirmed SIGTERM/SIGINT to a process owned by your own user, chosen by you in the table. There are no container or service controls.
- No SSH, no agent on the Windows side, no background service on your PC.

WHAT YOU NEED

- Windows 10 version 1809 (build 17763) or newer, or Windows 11, x64.
- The EbbTop agent on each Linux host you want to monitor (x86-64, glibc 2.39 or newer, systemd user session). Install it with the one-line script or the tarball from the project's Releases page; the agent is a normal systemd user service and writes a token only you can read.
- A private path between the PC and the hosts: the same LAN, a VPN, or a tailnet. EbbTop is not a public relay and needs no port forwarding.

Free and open source (MIT). The GitHub Releases page stays available for the Windows ZIP (the same app, without the Store's automatic updates), the Linux agent and the macOS build.
```

## What's new in this version

```
First Store release.

- Live dashboard for every host you add: CPU, memory, network, disk, GPU and workload roll-up.
- Sortable, filterable process table with a per-process detail inspector.
- Pairing with a six-digit code; read-only tokens kept in the Windows credential store.
- The Store build updates through the Microsoft Store.
```

## Product features (up to 20 bullets, Partner Center "Product features")

```
Live CPU, memory, network, disk and GPU panels
Sortable, filterable process table
Per-process detail: command, working directory, children, threads, CPU and memory graphs
Workload grouping by systemd unit and container
Multiple hosts in one window, switchable inline
Pairing with a six-digit code (read-only token)
Tokens kept in the Windows credential store
No account, no cloud relay, no telemetry
Private-network first: LAN, VPN or tailnet
Free and open source (MIT)
```

## Screenshots

Four images, exactly 1366x768, in `docs/store-screenshots/`:

| File | Scene |
|---|---|
| `ebbtop-store-1366x768-dashboard.png` | Dashboard: CPU, memory, network, GPU, disk, process table, LIVE |
| `ebbtop-store-1366x768-processes.png` | Workload roll-up and the sortable process table |
| `ebbtop-store-1366x768-pairing-discovered.png` | Pairing wizard: agents found on this network |
| `ebbtop-store-1366x768-pairing-code.png` | Pairing wizard: the six-digit code step |

They are generated, not captured by hand, from the app's own offscreen
renderer with fixture data (no real host names, addresses or tokens). Exact
commands:

```sh
cd <repo>
mkdir -p docs/store-screenshots
EBBTOP_SHOT_DIR=$PWD/docs/store-screenshots \
  cargo test -p ebbtop-gui store_screenshots -- --ignored
```

Re-render after any UI change and commit the four PNGs in the same change;
a listing screenshot that no longer matches the app is a review finding.

## Age rating (IARC questionnaire guidance)

Answer the questionnaire for what the app actually does. EbbTop is a
monitoring utility with no user-generated content, no communication features
and no purchases:

| Question area | Answer | Why |
|---|---|---|
| App category | Utility / productivity | It shows system metrics; there is no game or social element. |
| Violence, blood, sexual content, nudity, profanity, drugs, gambling, horror | No, all | The app renders numbers, charts and process names. |
| User-generated content shared with others | No | Nothing is uploaded anywhere; there is no feed, chat or profile. |
| Users can communicate with each other | No | No chat, comments, messaging or voice. |
| Shares user location | No | The app never requests location. |
| Shares personal information with third parties | No | There is no third party. Host addresses and tokens stay on the device. |
| Digital purchases, ads, in-app advertising | No | Free and open source; nothing is sold or advertised. |
| Unrestricted internet access | No | The app opens only the hosts the user configures. |
| Microsoft account sign-in | Not required | The Store handles installation; the app itself has no sign-in. |

Expected outcome: the lowest tier for every category (IARC "3+"-class in
most territories). If the questionnaire asks whether the app can display
third-party content, answer no: process names come from the user's own hosts.

## Privacy note (Partner Center privacy policy URL / description)

EbbTop connects only to hosts the user configures. It has no analytics, no
crash reporting, no advertising identifier and no account system, and it
sends nothing to the maintainers.

- **What is stored, and where:** host names, addresses and display settings
  in `%APPDATA%\ebbtop\config.json`; one token per host in the Windows
  credential store (service `ebbtop`, account = the host name you gave);
  a local log in `%LOCALAPPDATA%\ebbtop\ebbtop.log` with connection events and
  one stats line a minute. Nothing leaves the PC unless the user exports it
  (the support summary is written only when the user clicks Copy).
- **What goes over the network:** the WebSocket to each configured host, on
  the address and port the user typed. The Store build makes no other
  outbound request: unlike the GitHub download build, it has no update check
  (the Microsoft Store delivers updates).
- **What is never sent:** tokens, host lists or metrics to the maintainers;
  there is no endpoint to send them to. Log lines scrub URL query strings,
  and the support summary omits the host, address, path and token.
- **Tokens:** read-only for paired clients. The single write path is a
  user-confirmed kill signal to a process owned by the same user.

The project's public site states the same handling at
`https://ebbwater.net/ebbtop/privacy.html`.

## Store listing keywords / search terms

```
system monitor, process monitor, linux, remote monitoring, btop, htop, task
manager, server monitor, homelab, tailscale, process table, resource monitor
```

## Support and additional information

| Field | Value |
|---|---|
| Support contact | `kyler@ebbwater.net` (owner inbox; change if it moves) |
| Website | `https://ebbwater.net/ebbtop` |
| Privacy policy | `https://ebbwater.net/ebbtop/privacy.html` |
| License | MIT (`LICENSE` in the repository) |
| Source code | `https://github.com/Ebbwater/ebbtop` |
| System requirements | Windows 10 1809+ / Windows 11, x64; a Linux host running the EbbTop agent for each machine monitored |
| Age rating | Lowest tier; see the questionnaire guidance above |

## Pricing and availability

Free. Available in every market where the Store lists desktop apps. No
in-app purchases, no subscriptions, no ads.

## Before submitting (checklist)

- [ ] Version bumped in `Cargo.toml` **and**
      `deploy/windows/msix/AppxManifest.xml` (the packaging script refuses a
      mismatch).
- [ ] `EbbTop-<version>-x64.msix` built by the `store-package` job for the
      release tag, and its `sha256` recorded.
- [ ] `wack-gate` job report XML downloaded from the run and read: no FAIL
      rows (see `docs/windows-store.md`).
- [ ] Screenshots re-rendered at 1366x768 from the same commit.
- [ ] Package uploaded **unsigned**: the Store re-signs it.
- [ ] Listing text above pasted; age rating questionnaire answered as above.
