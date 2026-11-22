# Localization scope: the first Chinese desktop slice

This document states exactly what is localized in this candidate, what is still
English, and that **whole-app localization is not claimed**. Read it before
reading the slice as "EbbTop speaks Chinese".

Bundled locales: `English` (default), `zh-Hans-CN` (Mainland Simplified, 简体中文)
and `zh-Hant-TW` (Taiwan Traditional, 繁體中文（台灣）). Hong Kong
(`zh-Hant-HK`), Mexican Spanish and every other language are out of scope for
v0.3.0; a stored value this build does not know resolves to English.

## Localized in this slice

Four surfaces, by function. All of their user-facing strings and the AccessKit
names those controls expose come from `crates/ebbtop-gui/src/l10n.rs`:

1. **Onboarding wizard** — `onboarding()`, plus its validation and test paths
   `onboard_test()`, `onboard_save()`, `onboard_begin_pair()`,
   `onboard_paired()`, `validate_source()`: window title, introduction, the
   manual-address hints, the Name/URL/Token fields and their hints/accessible
   names, Test / Save and connect / Use a pairing code / Skip, the Save gate
   notes, the paired-credential note, the connected note and the classified
   failure block.
2. **Settings window** — `settings()` (title and tab labels),
   `settings_sources()`, `settings_display()` (including the new Language
   picker), `settings_connection()`, `settings_updates()` (updater builds only),
   `save_settings()` messages: every label, button, hint, tooltip, status line
   and message on those tabs.
3. **User-facing connection errors** — the classified failure headline and next
   action, chosen by `ebbtop_core::client::ProbeFailure` variant through
   `l10n::failure_keys` (never by matching English prose), the `detail:` label,
   and the displayed connection-state word (`LIVE` / `STALE` / `CONNECTING` /
   `DOWN`) in `status_bar()`. `Diagnosis.detail` itself — the scrubbed raw error
   chain — stays verbatim English.
4. **Process table and detail pane** — `proc_box()` and `proc_detail_box()`:
   placeholder, filter hint and accessible name, the count, the workload chip,
   the sample time, column headers, and in the detail pane the Kill button and
   its hover text, Copy, the facts rows (threads / fds / ppid / cpu / vmem /
   shared / r·w / started / cwd), the children and thread blocks, and the
   `no detail` / `waiting for data` states.

Raw data painted by those surfaces — hostnames, process names, users, commands,
args, cwd/exe paths, disk and interface names, GPU models, thread names, PIDs
and every numeric field — is **never** translated and is painted verbatim in
every locale. The bundled CJK faces exist so that this data renders even in the
English locale.

## Still English (remaining surfaces)

Approximate remaining string counts, by function, after this slice. They are
deliberately untouched: this card localizes four surfaces, it does not sweep the
app.

| Surface | Where | ~strings left |
| --- | --- | ---: |
| Kill confirmation dialog | `kill_dialog()` | 7 |
| Kill queue note | `queue_kill()` | 3 |
| Status-bar chrome (picker, age, counters, tooltips, kill note) | `status_bar()` | 18 |
| Top boxes: CPU panel | `cpu_box()` | 3 |
| Top boxes: GPU panel | `gpu_box()` | 2 |
| Top boxes: memory panel | `mem_box()` | 2 |
| Top boxes: network panel | `net_box()` | 3 |
| Top boxes: disk panel | `disk_box()` | 4 |
| Workload roll-up | `workloads_box()` | 3 |
| Update dialog and its flow | `update_dialog()`, `begin_update_download()`, `accept_update()`, `decline_update()`, `restart_for_update()` | 24 |
| Host discovery scanner and pairing-code prompt | `discovery::Discovery::draw()`, `discovery::Pairing::draw()` | 12 |
| Shared widgets | `widgets::error_block()` detail label, meter/panel chrome | 5 |
| Support summary, log notes and OS-handler errors | `store::support_summary()`, `store::open_logs()` | 12 |
| Window title / brand | `update_title()`, `main.rs` (`EBBTOP`, `EBBTOP <source>`) | never translated (product name) |

Two of those deserve emphasis because they are visible *inside* this slice:

- **The discovery scanner and pairing prompt** (`discovery.rs`) are drawn inside
  the onboarding wizard and the Settings > Sources tab, so a user of the wizard
  sees English there even in Chinese. `discovery.rs` is outside this card's
  permitted file set, so its strings were left as they are and are reported here
  rather than translated.
- **The status bar around the connection word** stays English: only the
  state word itself (`LIVE`/`STALE`/`CONNECTING`/`DOWN`) is localized.

## Not claimed

Whole-app localization is **not claimed** by this candidate. The four surfaces
above are the entire localized slice. Store listing text, installer text,
in-product documentation, voice/announcement text, the kill dialog, the updater
UI, discovery, the top boxes and the status-bar chrome remain English.

## Internal identifiers and canonical text

Never translated or routed through the locale table:

- `config.json` keys and values (the language is stored as the canonical id
  `en` / `zh-Hans-CN` / `zh-Hant-TW`; nothing localized is ever written to it);
- `Source.name` and URLs, keyring service/account names, `EBBTOP_TOKEN` and env
  var names;
- log lines (`probe <tag>: <detail>`, update/pairing notes), `Diagnosis.detail`
  and the support summary (`support_summary_text` / `ebbtop-support.txt`), which
  stay English so a support paste is readable by whoever receives it;
- the brand wordmark `EBBTOP`, the window title, product/CLI names and version
  strings.

Localization happens at the draw layer only; `ebbtop-core` remains the source of
canonical terms, and the only error key is the `ProbeFailure` variant.

## Observed coverage limit (recorded, not hidden)

`U+2715 MULTIPLICATION X` — the cross in the process-table workload chip
(`workload: … ✕`) — has **no glyph** in the shipped `epaint_default_fonts` set
and none in the bundled Noto Sans SC/TC faces either. This is a pre-existing
base rendering gap: the English chip draws the missing-glyph box today, and the
English string is left byte-identical here. Both locales paint the chip text
correctly; only that one symbol stays a blank box, and nothing crashes. A future
font or string change should revisit it (the coverage test pins the gap so it
cannot drift silently).
