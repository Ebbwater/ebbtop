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

## Localized listing copy: zh-CN (Simplified, Mainland China)

Paste into the Partner Center listing for `zh-CN`. Terminology follows the
app's own strings (`进程` for process, `工作负载` for workload,
`实时` for LIVE), so the storefront matches what the user sees after install.
The short description is 108 characters, inside the 270 limit.

### zh-CN short description

```
在 Windows 上监控你自己的 Linux 主机：CPU、内存、网络、磁盘，以及可排序、可筛选的进程表和进程详情。每台主机一个小型代理，通过局域网、VPN 或 tailnet 通信，令牌由你保管。无云端、无遥测。
```

### zh-CN description

```
EbbTop 是一款原生的类 btop 监控工具，用于监控你自己运行的 Linux 机器。

连接你自己的主机并实时查看：CPU 每核占用与温度、内存与交换分区、网络与磁盘吞吐、GPU 占用与显存，以及一个可排序、可筛选的进程表，附带进程详情面板（命令行、工作目录、子进程、线程、CPU 与内存历史曲线）。进程按工作负载归组，因此一个 systemd 服务或一个容器会显示为一行，并带有自己的合计值。

它为少数几台你自己拥有的机器而做，而不是为云上的机器集群而做。

工作原理

每台 Linux 主机上运行一个很小的代理，通过一个 WebSocket 返回指标、进程表以及按需请求的进程详情。桌面应用可以连接任意多台主机，每台主机一个令牌，保存在 Windows 凭据管理器中，断线后会自动重连。

配对只需一分钟。在主机上运行 ebbtop-agent --pair，把它打印的六位数字输入 EbbTop，应用会保存它签发的只读令牌。也可以直接填写地址和令牌，适用于 tailnet 或其他网段上的主机。

它不做什么

- 没有云端账号，没有登录，没有遥测。应用只连接你配置的主机。
- 只读监控。唯一能执行的操作，是你在进程表中选中的、属于你自己用户的进程，经过确认后发送 SIGTERM 或 SIGINT。没有任何容器或服务控制功能。
- 没有 SSH，Windows 端不安装代理，你的电脑上也没有后台服务。

你需要准备

- Windows 10 版本 1809（内部版本 17763）或更新版本，或 Windows 11，x64。
- 在每台要监控的 Linux 主机上安装 EbbTop 代理（x86-64、glibc 2.39 或更新版本、systemd 用户会话）。用项目 Releases 页面的一行安装脚本或压缩包安装；代理是一个普通的 systemd 用户服务，它写出的令牌只有你能读取。
- 电脑与主机之间的私有网络通路：同一局域网、VPN 或 tailnet。EbbTop 不是公共中继，也不需要端口转发。

免费且开源（MIT 许可证）。GitHub Releases 页面继续提供 Windows ZIP（同一个应用，但没有 Store 的自动更新）、Linux 代理和 macOS 版本。
```

### zh-CN What's new

```
首个 Store 版本。

- 为每台添加的主机提供实时面板：CPU、内存、网络、磁盘、GPU 与工作负载汇总。
- 可排序、可筛选的进程表，附带进程详情面板。
- 六位数字配对码；只读令牌保存在 Windows 凭据管理器中。
- 界面支持英文、简体中文和繁体中文。
- Store 版本通过 Microsoft Store 更新。
```

## Localized listing copy: zh-TW (Traditional, Taiwan)

Paste into the Partner Center listing for `zh-TW`. Taiwan terminology is used
(`處理程序` not `進程`, `記憶體` not `内存`, `網路` not `网络`), matching the
app's `zh-Hant-TW` strings.

### zh-TW short description

```
在 Windows 上監控你自己的 Linux 主機：CPU、記憶體、網路、磁碟，以及可排序、可篩選的處理程序表與詳細資料。每台主機一個小型代理，透過區域網路、VPN 或 tailnet 通訊，權杖由你保管。無雲端、無遙測。
```

### zh-TW description

```
EbbTop 是一款原生的類 btop 監控工具，用來監控你自己執行的 Linux 機器。

連線到你自己的主機並即時檢視：CPU 每核心使用率與溫度、記憶體與置換空間、網路與磁碟吞吐量、GPU 使用率與視訊記憶體，以及一個可排序、可篩選的處理程序表，附處理程序詳細資料面板（命令列、工作目錄、子處理程序、執行緒、CPU 與記憶體歷史曲線）。處理程序依工作負載分組，因此一個 systemd 服務或一個容器會顯示為一列，並帶有自己的合計值。

它是為少數幾台你自己擁有的機器而做，而不是為雲端上的機器叢集。

運作方式

每台 Linux 主機上執行一個很小的代理，透過一個 WebSocket 回傳指標、處理程序表以及隨選的處理程序詳細資料。桌面應用程式可以連線任意多台主機，每台主機一個權杖，儲存在 Windows 認證管理員中，連線中斷後會自動重新連線。

配對只需一分鐘。在主機上執行 ebbtop-agent --pair，將它顯示的六位數字輸入 EbbTop，應用程式會儲存它簽發的唯讀權杖。也可以直接填入位址和權杖，適用於 tailnet 或其他網段上的主機。

它不做什麼

- 沒有雲端帳號，沒有登入，沒有遙測。應用程式只連線你設定的主機。
- 唯讀監控。唯一能執行的動作，是你在處理程序表中選取、屬於你自己使用者的處理程序，經確認後送出 SIGTERM 或 SIGINT。沒有任何容器或服務控制功能。
- 沒有 SSH，Windows 端不安裝代理，你的電腦上也沒有背景服務。

你需要準備

- Windows 10 版本 1809（組建 17763）或更新版本，或 Windows 11，x64。
- 在每台要監控的 Linux 主機上安裝 EbbTop 代理（x86-64、glibc 2.39 或更新版本、systemd 使用者工作階段）。用專案 Releases 頁面的一行安裝指令或壓縮檔安裝；代理是一個一般的 systemd 使用者服務，它寫出的權杖只有你能讀取。
- 電腦與主機之間的私有網路通路：同一個區域網路、VPN 或 tailnet。EbbTop 不是公共中繼，也不需要連接埠轉送。

免費且開源（MIT 授權）。GitHub Releases 頁面繼續提供 Windows ZIP（同一個應用程式，但沒有 Store 的自動更新）、Linux 代理和 macOS 版本。
```

### zh-TW What's new

```
首個 Store 版本。

- 為每台新增的主機提供即時面板：CPU、記憶體、網路、磁碟、GPU 與工作負載彙總。
- 可排序、可篩選的處理程序表，附處理程序詳細資料面板。
- 六位數字配對碼；唯讀權杖儲存在 Windows 認證管理員中。
- 介面支援英文、簡體中文和繁體中文。
- Store 版本透過 Microsoft Store 更新。
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

Eleven images, all exactly 1366x768, in `docs/store-screenshots/`. The English
scene set is what the `en-US` listing uses; the six localized ones carry the
same scenes in the language of each storefront listing, so a Chinese listing
never shows an English screenshot.

Partner Center takes a caption per image. Use these — they describe what the
scene shows, in the language of the listing they belong to.

**en-US**

| Image | Caption |
|---|---|
| `dashboard-en.png` | Live dashboard for one host: CPU with per-core meters and temperature, memory and swap, network and disk throughput, GPU load and VRAM, the workload roll-up, and the process table. Status reads LIVE. |
| `process-detail-en.png` | The process detail inspector: command line with Copy, CPU time, virtual memory, start time, working directory, executable, child processes, top threads, and CPU and memory history graphs for the selected process. |
| `pairing-discovered.png` | First run: EbbTop finds the agents on your own network, showing each host name, agent version and address. |
| `pairing-code.png` | Pairing with a six-digit code: the app exchanges the code for a read-only token and stores it, so no token is typed by hand. |

**zh-CN**

| Image | Caption |
|---|---|
| `dashboard-zh-cn.png` | 单台主机的实时面板：CPU 每核占用与温度、内存与交换分区、网络与磁盘吞吐、GPU 占用与显存、工作负载汇总，以及进程表。状态显示为“实时”。 |
| `process-detail-zh-cn.png` | 进程详情面板：命令行与复制按钮、CPU 时间、虚拟内存、启动时间、工作目录、可执行文件、子进程、主要线程，以及所选进程的 CPU 与内存历史曲线。 |

**zh-TW**

| Image | Caption |
|---|---|
| `dashboard-zh-tw.png` | 單台主機的即時面板：CPU 每核心使用率與溫度、記憶體與置換空間、網路與磁碟吞吐量、GPU 使用率與視訊記憶體、工作負載彙總，以及處理程序表。狀態顯示為「即時」。 |
| `process-detail-zh-tw.png` | 處理程序詳細資料面板：命令列與複製按鈕、CPU 時間、虛擬記憶體、啟動時間、工作目錄、執行檔、子處理程序、主要執行緒，以及所選處理程序的 CPU 與記憶體歷史曲線。 |

| File | Scene | Listing language |
|---|---|---|
| `ebbtop-store-1366x768-dashboard-en.png` | Dashboard: CPU, memory, network, GPU, disk, workloads, process table, LIVE | en-US |
| `ebbtop-store-1366x768-process-detail-en.png` | A selected process: detail inspector with command line, CPU time, memory, cwd/exe, children, threads and two history graphs | en-US |
| `ebbtop-store-1366x768-pairing-discovered.png` | Pairing wizard: agents found on this network | en-US |
| `ebbtop-store-1366x768-pairing-code.png` | Pairing wizard: the six-digit code step | en-US |
| `ebbtop-store-1366x768-dashboard-zh-cn.png` | Dashboard, Simplified Chinese interface | zh-CN |
| `ebbtop-store-1366x768-process-detail-zh-cn.png` | Process detail inspector, Simplified Chinese | zh-CN |
| `ebbtop-store-1366x768-dashboard-zh-tw.png` | Dashboard, Traditional Chinese interface | zh-TW |
| `ebbtop-store-1366x768-process-detail-zh-tw.png` | Process detail inspector, Traditional Chinese | zh-TW |

The four unsuffixed files (`dashboard.png`, `processes.png`, `pairing-code.png`,
`pairing-discovered.png`) are the original English scene set. `dashboard.png`
and `dashboard-en.png` are the same scene; `processes.png` is the table without
a selection. Keep them because the README and older notes reference them, but
the Store listing uses the suffixed set.

Every image is generated, not captured by hand, from the app's own offscreen
renderer with fixture data (no real host names, addresses or tokens). Exact
commands:

```sh
cd <repo>
mkdir -p docs/store-screenshots
EBBTOP_SHOT_DIR=$PWD/docs/store-screenshots \
  cargo test -p ebbtop-gui --locked store_screenshots -- --ignored
```

The localized scenes set `language` in the `Config` they render with, which is
the same field the app reads at draw time, so these are real localizations and
not overlaid text. The bundled Noto faces are what make the Chinese render
offline.

Re-render after any UI change and commit the PNGs in the same change;
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

## Accessibility statement (Partner Center "Accessibility" / product features)

Claims only what `docs/accessibility-audit.md` proves. The audit's own draft is
the source; this is that text with the two findings added since (A-11, no
language hint; and the note that the picker labels are localized) and it
deliberately does not claim full screen-reader support.

```
EbbTop is built to work with the keyboard and with Windows assistive technology.

Keyboard: the dashboard, the process table and the first-run wizard are operable
without a pointer. Tab reaches the resource panes, the filter and Settings; the
process table is driven by the arrow keys (Up/Down move the selection through the
visible, filtered and sorted rows) and the selected process opens a detail pane
whose Kill action is reachable with Tab and opened with Space. In the kill
confirmation, Tab and Space choose the signal and run it. Every step of the
first-run wizard (host name, address, token, pairing code) can be reached and
activated from the keyboard.

Screen readers: buttons, text fields, check boxes, drop-downs, sliders and dialog
titles are exposed with their names and roles through the Windows UI Automation
tree (AccessKit). Connection state is shown as a word ("LIVE", "STALE",
"CONNECTING", "DOWN"), never by colour alone.

Display: the interface scales with Ctrl/Cmd + and -, and the chosen scale is
remembered between launches. Body text, headings, buttons, warnings and the
connection badges meet WCAG 2.2 AA contrast on the dark theme panels and
dialogs; two pairs of secondary grey text sit just under the bar (4.3-4.4:1) and
the selected-row highlight is below it (secondary cells at 1.98:1 on the
selection fill).

Language: the interface is available in English, Simplified Chinese and
Traditional Chinese, chosen in Settings > Display and remembered between
launches.

Known limitations, stated rather than glossed over: the process table's rows are
not yet announced individually by screen readers (the row data is read cell by
cell), the painted resource gauges (per-core CPU, memory/swap/disk totals, GPU
and network values) are not yet exposed to screen readers, and at 150-200% zoom
the memory and disk panes keep some values below the visible area, where only a
mouse-wheel scroll reaches them. A change of state (a connection going STALE, a
kill being refused) is not announced automatically, and no language hint is
passed to the screen reader, so a reader may use the wrong voice for Chinese
text. Each limitation is a recorded gap with a retained test in the audit behind
this statement.
```

Do not use "fully accessible", "WCAG 2.2 AA compliant" or "screen-reader
friendly": A-1 and A-2 make a blanket claim false.

## Notes for certification

The one intake risk for a client-server app: a tester installs on a clean machine
with no EbbTop agent anywhere and sees only the first-run wizard. Paste this into
**Submission options -> Notes for certification** so the reviewer knows why.

```
EbbTop is a monitor for the customer's own Linux machines, not a hosted service.
The Windows app has no account, no sign-in and no server of its own: it connects
over the customer's LAN, VPN or tailnet to a small agent the customer installs on
each Linux host.

A reviewer testing on a clean machine therefore sees the first-run wizard with no
hosts listed, which is the correct empty state. Nothing is wrong and no
configuration is missing on Microsoft's side. To see a populated dashboard:

1. Use any Linux machine (a VM is fine) on the same network as the test PC.
2. On that Linux host, run the one-line installer from our public repository:
   curl -fsSL https://github.com/Ebbwater/ebbtop/releases/latest/download/ebbtop-install.sh | bash
   It installs a systemd user service and prints a host address and a token path.
3. On the Windows PC, in the EbbTop wizard, type that address (format
   ws://<address>:45976/ws) and paste the token from the printed path, then Test
   and Save and connect. The dashboard reads LIVE within about a second.

Without a Linux host, the four screenshots on the listing show the populated
dashboard, the process table, and both steps of the pairing wizard; they are
rendered from the shipping build, not mocked up.

The app makes no outbound connection except to the hosts the customer configures.
The Store build has no update check and no in-app updater, because the Store
delivers updates; the GitHub download build is the one with the updater. There is
no telemetry, no analytics and no crash reporting in either build.

The one write action in the app is an explicit, confirmed SIGTERM or SIGINT sent
to a process owned by the same user who runs the agent; it cannot signal root
processes, PID 1, kernel threads, or another user's processes, and Store-era
clients receive read-only tokens that are refused that action.
```

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
      rows on a non-optional test (`deploy/windows/msix/parse-wack-report.ps1`
      is the same reader the job runs; `OPTIONAL="TRUE"` failures are findings,
      not blockers).
- [ ] Screenshots re-rendered at 1366x768 from the same commit.
- [ ] Package uploaded **unsigned**: the Store re-signs it.
- [ ] Listing text above pasted; age rating questionnaire answered as above.
