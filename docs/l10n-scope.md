# Localization scope: the Chinese desktop interface (v0.3.0)

This document states exactly what is localized, what is intentionally still
English, and what is **not** claimed. Read it before reading the build as
"EbbTop speaks Chinese".

Bundled locales: `English` (default and the per-unknown-value fallback),
`zh-Hans-CN` (Mainland Simplified, 简体中文) and `zh-Hant-TW` (Taiwan
Traditional, 繁體中文（台灣）). Hong Kong (`zh-Hant-HK`), Mexican Spanish and
every other language are out of scope for v0.3.0; a stored value this build
does not know resolves to English. The language is stored as the canonical id
in `config.json` and nothing localized is ever written there.

## Localized now (the whole desktop interface)

Every user-facing string of the desktop window comes from
`crates/ebbtop-gui/src/l10n.rs`. The key table is generated as one unit (enum,
`Key::ALL` and `entry()`), so a key cannot exist without its three strings and a
missing locale is a compile error rather than a silent English fallback.

| Surface | Where |
| --- | --- |
| Onboarding wizard: introduction, manual-entry hints, fields, Test / Save and connect / Use a pairing code / Skip, the Save gate, paired-credential and connected notes | `onboarding()`, `onboard_test()`, `onboard_save()`, `onboard_begin_pair()`, `onboard_paired()`, `validate_source()` |
| Settings: title, every tab label, every label/button/hint/tooltip/status line on Sources, Display, Connection and Updates, and the font license notice | `settings()`, `settings_sources()`, `settings_display()`, `settings_connection()`, `settings_updates()`, `save_settings()`, `font_notice()` |
| Discovery scanner: scan/cancel controls, the honest scope limit, the empty state, the per-agent row and "use this host" | `discovery::Discovery::draw()` |
| Pairing prompt: title, address hint, code field and its accessible name, Pair / Cancel, the busy state, and every classified pairing refusal | `discovery::Pairing::draw()`, `l10n::pair_failure_keys` |
| Kill confirmation and its outcome: dialog title, the whole consequence sentence, TERM / INT / Cancel, the queue note and the agent's success/refusal note | `kill_dialog()`, `queue_kill()`, `status_bar()` |
| Status bar: connection state word, host / no-source label, age and its tooltip (lag, sample time, tick p95, reconnects), down-and-retry and attempt notes, the update prompt, Settings / Dense / source-picker labels | `status_bar()` |
| Top panels: CPU, GPU, memory, network and disk titles, cores / load / uptime, meter labels (Used, Swap, Load, VRAM), interface and disk pickers, peak lines, "measuring", "waiting for data" | `cpu_box()`, `gpu_box()`, `mem_box()`, `net_box()`, `disk_box()` |
| Workload roll-up and the workload chip: title, column headers, and the chip's painted close affordance | `workloads_box()`, `proc_box()` |
| Process table and detail pane: filter, count, chip, sample time, column headers, Kill / Copy and the facts rows, children/thread blocks, empty states | `proc_box()`, `proc_detail_box()` |
| Updater dialogs (updater builds only): available / downloading / ready / failed, their bodies, Download update / Later / Restart / Check again, and the status-bar prompt | `update_dialog()`, `settings_updates()`, `status_bar()` |
| GUI-facing support and log actions: "Open logs" failures and the support-summary copy note | `store::open_logs()`, `support_summary_action()`, `open_logs_note()` |
| Bundled font license notice: heading, both faces with their upstream copyright, and the exact checked-in OFL text | `font_notice()` |
| Classified connection failures: headline and next action, chosen by `ProbeFailure` variant (never by matching English prose) | `l10n::failure_keys` |

## Never translated (canonical by design)

- `config.json` keys and values, keyring service/account names, `EBBTOP_TOKEN`
  and other environment-variable names;
- raw painted data: hostnames, process names, users, commands, arguments,
  cwd/exe paths, interface and disk names, GPU models, thread names, PIDs and
  every numeric field;
- the support summary (`support_summary_text` / `ebbtop-support.txt`), the log
  lines (`probe <tag>: <detail>`, `pair <tag>: <detail>`, update notes) and
  `Diagnosis.detail` / `PairError.detail` — the scrubbed raw error chains — so
  a support paste stays readable by whoever receives it;
- updater worker failure details shown inside the update dialog (the localized
  frame reports them verbatim);
- the connection words used by the support summary (`LIVE` / `STALE` /
  `CONNECTING` / `DOWN` stay English there, localized only on screen);
- the brand wordmark `EBBTOP`, the window title, product/CLI names and version
  strings;
- units and identifiers: `CPU`, `GPU`, `PID`, `CPU%`, `MEM%`, `TERM`, `INT`,
  `5m` / `60s`, `SIGTERM`, `ws://`, `UDP 45978`, `EBBTOP_AGENT_DISCOVERY`, and
  every command the user is told to run.

Localization happens at the draw layer only; `ebbtop-core` remains the source
of canonical terms, and the only error keys are the `ProbeFailure` and
`PairFailure` variants.

## Bundled fonts and the license notice

Two Noto Sans CJK regional faces (SIL OFL 1.1) are compiled in with
`include_bytes!` and appended *behind* the shipped Latin faces, so English
metrics are unchanged while any CJK text renders offline. Bytes are pinned and
hash-recorded in `crates/ebbtop-gui/assets/fonts/PROVENANCE.md`; this build does
not modify, rename or subset them.

The license and the attributable font names are readable **offline from the
executable**: `Settings > Display > Fonts and license` opens a notice that shows
both faces with their upstream copyright line and the exact checked-in
`LICENSE-OFL.txt` text, compiled in with `include_str!`. The notice test compares
the compiled constant to the file on disk byte for byte, so the displayed text
cannot drift from the checked-in license.

**Packaging note for the integration stage (not done in this card):** the notice
makes the license readable inside the app, but the OFL also expects the license
to accompany a redistribution of the font files. The MSIX staging step should
copy `crates/ebbtop-gui/assets/fonts/LICENSE-OFL.txt` into the package directory
that receives `ebbtop.exe`, and the release check should read it back from the
installed package.

## Observed coverage limit: the workload chip (resolved here)

The earlier slice recorded that `U+2715 MULTIPLICATION X` — the cross in the
process-table workload chip — has no glyph in the shipped
`epaint_default_fonts` set nor in the bundled Noto Sans SC/TC faces, so the
English chip drew a missing-glyph box, and the English string was left
byte-identical to avoid papering over it.

v0.3.0 resolves it without touching the fonts: the chip text is
`workload: <label>` / `工作負載：<label>` and the cross is **painted** beside it
(two line segments — the same no-glyph-dependency approach as the status-bar
connection dot). The painted close target carries its own AccessKit name
(`Clear workload filter` / `清除工作负载筛选` / `清除工作負載篩選`), is
keyboard-operable like any focused button, and the chip label itself stays
clickable, so click-to-clear behaves as before. The glyph-gap test still pins the
font fact (U+2715 has no glyph in these faces) and now also asserts that no UI
string types that codepoint, so the gap cannot come back through text.

## Not claimed

- **Regional language review.** The Mainland and Taiwan strings are the
  producer's technical translation; a native reader per region has **not**
  reviewed them in this card, and this document does not claim otherwise.
  `frozen-strings.tsv` (produced by `cargo test -p ebbtop-gui
  export_frozen_strings -- --ignored` with `EBBTOP_L10N_EXPORT` set) is the
  review input; the terminology pairs below are listed for that review.
- **Native Windows verification.** UIA exposure of the localized AccessKit
  names, CJK rasterization through Windows/DirectX, the localized MSIX and a
  fresh install/WACK run remain integration work (Captain + the Windows lane).
- **Store listing, installer text, in-product documentation** and any
  announcement/voice text.
- **Locale autodetection.** The language is a stored setting with an explicit
  picker; the OS locale is not consulted.
- **Spanish and Hong Kong.** Deferred; a stored value for either resolves to
  English.

## Terminology choices for the regional review

| English | zh-Hans-CN (Mainland) | zh-Hant-TW (Taiwan) |
| --- | --- | --- |
| process | 进程 | 處理程序 |
| thread | 线程 | 執行緒 |
| memory | 内存 | 記憶體 |
| network | 网络 | 網路 |
| disk | 磁盘 | 磁碟 |
| interface | 网络接口 | 網路介面 |
| token | 令牌 | 權杖 |
| credential / credential store | 凭据 / 凭据存储 | 認證 / 認證存放區 |
| address | 地址 | 位址 |
| port | 端口 | 連接埠 |
| agent | 代理 | 代理程式 |
| save | 保存 | 儲存 |
| settings | 设置 | 設定 |
| support | 支持摘要 | 支援摘要 |
| log | 日志 | 記錄檔 |
| load (CPU) | 负载 | 負載 |
| swap | 交换 | 置換 |
| VRAM | 显存 | 視訊記憶體 |
| default | 默认 | 預設 |
| client | 客户端 | 用戶端 |
| session | 会话 | 工作階段 |
| firewall | 防火墙 | 防火牆 |
| font / license | 字体 / 许可证 | 字型 / 授權 |

The pairs are internally consistent with each other and with the first slice.
They are a language question, not a technical one: a Mainland reader and a
Taiwan reader should each confirm their own column before release.
