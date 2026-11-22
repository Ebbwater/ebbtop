# EbbTop accessibility audit

Baseline: **WCAG 2.2 AA, adapted to a native desktop app**. That is the practical
bar for a Windows Store listing and the frame Microsoft reviewers use when an
app claims accessibility. Every claim below is produced by a headless test in
this tree; the command is quoted with each finding, and the tests are retained
in the suite so the claims cannot drift.

- Base commit: `2b83e0c77f9b6e6a8cbf29fdd746ec0dbf794bad` (main)
- Branch: `audit/a11y` (local only; nothing pushed, no live agent touched)
- Platform of record: Linux host, headless egui (`ctx.run_ui`), egui 0.37 /
  AccessKit 0.25. The Windows Store build uses the same egui/eframe code path
  with eframe's `accesskit` feature, which wires this tree to UIA.

## What was audited

| Screen | Fixture |
|---|---|
| Dashboard (all top panes, workloads, process table) | 401 processes, 12 cores, GPU, two NICs, one disk |
| Dashboard with a selected process (detail pane) | pid 4242 with 30 s of detail history |
| Onboarding wizard (fresh install) | no sources |
| Onboarding wizard, pairing-code step | address `127.0.0.1:1`, code typed |
| Settings: Sources / Display / Connection / Updates | one source |
| Kill confirm dialog | pid 4242, `hottest-proc` |
| Update dialog | `v99.7.0` available |

The tree is what egui hands a platform adapter: `Context::enable_accesskit()`
plus `FullOutput::platform_output.accesskit_update`. No test drives a pointer
unless it says so; the audit is about what a keyboard and a screen reader get.

```
cargo test -p ebbtop-gui --locked a11y::      # 12 tests
cargo test -p ebbtop-gui --locked wcag        #  4 tests
```

## Findings by severity

### Blocker — a screen reader cannot read the dashboard's numbers (A-1)

`w::meter` and `w::cpu_bar_cell` paint their label and value with
`painter.galley`, which never creates an AccessKit node, so the resource panes
are silent: per-core CPU (C0…C11 and their %), load average, GPU load, VRAM,
memory/swap totals, disk usage, network rate, and every row's CPU% cell.

Measured inventory (36 distinct strings painted, absent from the tree):

```
cargo test -p ebbtop-gui --locked a11y_painted_metrics_are_not_in_the_tree
```

```
"0.1", "1.0 GiB / 4.0 GiB", "1.0 GiB / 8.0 GiB", "11%", "12.0 GiB / 32.0 GiB",
"127.5", "18%", "25%", "32%", "37%", "39%", "4%", "46%", "512 GiB / 1.0 TiB",
"69%", "76%", "83%", "87.5", "90%", "97%", "C0"…"C11", "Load", "Swap", "Used",
"VRAM"
```

Everything drawn through egui widgets *is* exposed (labels, pane titles, table
cells, badges, dialog text), so the gap is confined to the two painted widgets.
Fix: expose each meter/bar as a named node (`Context::accesskit_node_builder`,
`Role::Label`/`Meter`, `set_label`/`set_numeric_value`) or wrap the text in a
`Label` the way the rest of the UI does. The test flips from "absent" to
"present" when the values are exposed.

### Blocker — table rows are anonymous, unlocated, and dominate the tab order (A-2)

`egui_extras::TableRow` reports nothing: the row `Ui` senses clicks (so it is
focusable and clickable) but the node has role `GenericContainer`, no name and
**no bounds** (`Rect::NOTHING`). A screen reader announces "group" with nothing
else; UIA has no rectangle to hit-test or highlight. The workload roll-up rows
and the process rows fail the same way.

```
cargo test -p ebbtop-gui --locked a11y_table_rows_and_splitter_have_no_role_or_name
```

The same node class is what Tab walks through, so the tab order is the table: over
400 Tab presses, **337 land on unnamed nodes, 332 of them table rows** (84 %).
The census is pinned in the retained test, which prints it with `--nocapture`:

```
$ cargo test -p ebbtop-gui --locked a11y_tab_order_starts_in_the_status_bar_and_wraps -- --nocapture
tab census: 337/400 unnamed stops, 332 of them table rows (wrap at press 137)
```

Documented tab order (dashboard, no selection), 13 named stops before the table:

1. `Settings` 2. `Dense` 3. `5m` 4. `60s` 5. pane title `v cpu · <model>`
6. `v gpu · <model>` 7. `v mem` 8. `v disk` 9. `v net` 10. network-interface
picker `ComboBox` (shows the fixture NIC) 11. `log scale` check box 12. the
splitter (anonymous, see A-8) 13. `v workloads`, then the workload rows, the
filter field, the sort headers and every visible process row. The disk picker
(`ComboBox`, per-volume) only appears when the host reports more than one
volume, so the one-disk fixture never renders it.

With a selection the detail pane appears before the table, so `v proc-detail` is
stop 13 and `Kill` is stop 14 (`Tab` index 13).

Fix (structural, not in this slice): give rows `Role::Row`/`Cell`, a name from
the row's text, selection state, bounds, and a row focus model
(`set_active_descendant` on a `Grid`/`Table` container is the UIA-friendly
shape).

### Major — confirming a kill takes 78 Tab presses (A-3)

egui windows are not modal and focus is not moved into them on open. The kill
confirm is drawn last in the frame, so its buttons come after every visible
table row:

```
cargo test -p ebbtop-gui --locked a11y_keyboard_selects_a_row_and_reaches_kill
```

Measured: `Kill` is Tab stop 14; from there `Send TERM` is **press 78**. The
action is reachable without a pointer (proved end to end in that test: arrow
selection → Tab → Space → Tab → Space), but the focus order is not the one a
dialog needs (WCAG 2.4.3). Fix: request focus on `Send TERM` (or the dialog's
first control) when the confirm opens; the test's pinned 78 then becomes 1.

### Major — the selected-row palette fails AA where it matters most (A-4)

Measured with WCAG 2.2 relative luminance (`cargo test -p ebbtop-gui --locked
wcag_aa_contrast_gaps_are_pinned`):

| Pair | Ratio | Required |
|---|---|---|
| Secondary row text (`DIM` #7d8590) on the selection fill (#005c80) | **1.98:1** | 4.5:1 |
| Active workload label (`ACCENT` #5fb3e6) on the selection fill | **3.19:1** | 4.5:1 |
| Selection fill (#005c80) against a panel (#16191f) | **2.38:1** | 3:1 (1.4.11) |

That is the row a user just selected: PID, state, MEM and workload cells drop to
1.98:1, and the selection itself is signalled by a fill that is below the 3:1
component minimum. Fix: lighten the selection fill and/or switch selected-row
text to `TEXT`; the test pins the current numbers and flips when they improve.

### Major — at 150 %/200 % zoom the memory/disk panes start clipped, and the keyboard cannot recover them (A-5)

The top panes are laid out in a height-capped scroll area; at increased zoom the
content is taller than the cap and the first paint loses it:

```
cargo test -p ebbtop-gui --locked a11y_zoom_paints_no_overlapping_text_and_keeps_panes_reachable -- --nocapture
```

```
zoom census (visible, clipped): [(1.0, (174, 1)), (1.5, (88, 9)), (2.0, (66, 17))]
```

| Zoom | Visible text runs | Fully clipped | Overlapping |
|---|---|---|---|
| 100 % | 174 | 1 | 0 |
| 150 % | 88 | 9 (`Used`, `Swap`, `nvme0n1`, their values) | 0 |
| 200 % | 66 | 17 (`Load`, `VRAM`, mem/swap/disk totals) | 0 |

The census is pinned exactly in that test (`Frame::text_census`), so a layout
change that moves these numbers must update the table.

No text overlaps at any zoom, and the **mouse wheel** reveals the clipped values
(asserted in the same test). Keyboard-only recovery does not work, so this audit
makes no 1.4.4 claim for keyboard users: with `v mem`, `v disk` or `log scale`
focused, `ArrowDown`/`PageDown`/`End`/`Space`/`ArrowUp`/`PageUp` never reveal
`Used`, nor do the global `PageDown`/`End`/`ArrowDown` keys, and the splitter that
sizes the panes is drag-only (A-8). Pinned by the retained test below:

```
$ cargo test -p ebbtop-gui --locked a11y_keyboard_cannot_reveal_the_values_clipped_at_200_percent -- --nocapture
keyboard-scroll probe: Tab stop 7 = v mem; 6 keys never reveal Used
keyboard-scroll probe: Tab stop 8 = v disk; 6 keys never reveal Used
keyboard-scroll probe: Tab stop 11 = log scale; 6 keys never reveal Used
```

So at 150–200 % zoom a keyboard-only user cannot read the memory/disk/network
values at all, and the first paint of the most important numbers is empty at
150 %. Fix (structural, follow-up): per-pane keyboard scrolling or reflow at
scale, plus arrow-key handling on the splitter, so the numbers are reachable
without discovering the inner scroll area or owning a pointer.

### Minor — secondary text sits just under AA on two surfaces (A-6)

| Pair | Ratio | Required |
|---|---|---|
| `DIM` on the status bar (#1e2128) — the *age* readout, kill notes | **4.32:1** | 4.5:1 |
| `DIM` on a striped row (#1b1f26) — PID/state/MEM/workload cells | **4.43:1** | 4.5:1 |

Both are within a few percent of the bar; lightening `DIM` by one step clears
both without touching the look of the app. Pinned in
`wcag_aa_contrast_gaps_are_pinned`.

### Minor — focus indication is missing on rows and the splitter, colour-only on titles (A-7)

Measured by diffing painted primitives inside the widget's rect, focused vs
unfocused (`a11y_focus_visual_on_buttons_but_not_on_titles_or_rows`):

- Buttons and text fields: focus paints `widgets.active` — #282828 fill, **1 px
  white frame**, white text. Clearly visible; one frame after the key press.
- Pane titles and sort headers: the only change is the text turning pure white.
  Visible, but a colour-only cue on a header that already reads as a link
  (WCAG 1.4.1 is satisfied only because the cue is not the *sole* affordance —
  the same control is named and activatable — but it is weak).
- Table rows: **nothing changes**. Tab lands on a row and the pixels are
  identical.
- The splitter (A-7b): focusable, and it paints the same hairline focused or not.

Fix: a focus visual for rows and the splitter (2 px `ACCENT` inner frame on the
focused row is the cheapest; the splitter needs arrow-key handling as well).

### Minor — the splitter cannot be operated from the keyboard (A-8)

The 8 px drag handle between the panes and the tables senses drag only. It is
focusable (`Role::Unknown`, unnamed) but has no arrow-key handling, so the
top/bottom ratio can only be changed with a pointer. `Top height` in Settings →
Display is the keyboard-reachable equivalent and is now named; that is the
workaround to document until arrow handling lands.

### Minor — state changes are drawn, not announced (A-9)

The connection badge, the kill result and the update prompt are plain static
text: the tree carries no `Alert`/live node and focus never moves to them, so a
screen-reader user is not told that a connection went STALE or that a kill was
refused (WCAG 4.1.3). Pinned by `a11y_status_changes_are_not_announced`; the
fix is a live/alert node (or a focus move) when the state changes.

### Minor — one global single-key shortcut, and no way to remap it (A-10)

`/` focuses the filter from anywhere in the window (WCAG 2.1.4 wants such a
shortcut to be remappable or limited to the focused component). Measured safe,
which is why this is minor rather than a failure: typing `/` into a focused
field does **not** steal the character or the focus — the wizard walkthrough
types `t0/ken` into the token field and asserts both. Fix: honour the shortcut
only when no text field has focus, or add a Settings toggle.

### Passing inventory

| What | Evidence |
|---|---|
| Every screen builds a complete tree; every named control has an accessible name | `a11y_every_screen_exposes_named_controls` |
| Text fields are named — the wizard's Name/URL/token, Settings' source and connection fields, the filter, the pairing code, `Top height` | same test (this slice added the labels; before it the ten text fields were named only by hint text that disappears on typing, and the `Top height` slider had no name) |
| Buttons, check boxes, combo boxes, sliders, windows expose role + name | same test; window titles (`Welcome to EbbTop`, `Settings`, `Kill process`, `Update available`) are in the tree |
| LIVE / STALE is a word, not just a colour, and the stale frame states the age | `a11y_status_state_is_not_colour_only` |
| Sort direction is a glyph (`▲`/`▼`) in the header text, so it survives colour blindness | pinned in `a11y_every_screen_exposes_named_controls` (headers are named) |
| Keyboard selects a process row and reaches the Kill action | `a11y_keyboard_selects_a_row_and_reaches_kill` |
| The pairing wizard runs start to finish from the keyboard, and the Save gate still holds | `a11y_keyboard_operates_the_pairing_wizard` |
| No keyboard trap: Tab wraps and returns to the status bar | `a11y_tab_order_starts_in_the_status_bar_and_wraps` |
| Text contrast on page/panel/bar, badges, warnings/errors, buttons (normal, hover, focused) | `wcag_aa_text_contrast_holds_for_every_passing_pair` (19 pairs, 4.64:1 … 14.74:1) |
| Load bars and CPU cells: 3:1 for the graphic, 4.5:1 for the text over the wash | `wcag_non_text_contrast_for_load_bars` |
| Zoom works from the keyboard (Ctrl/Cmd + `=`/`+`/`-`/`0`) and persists | `a11y_keyboard_zoom_works_and_persists` |
| No visible text overlaps at 100/150/200 % zoom | `a11y_zoom_paints_no_overlapping_text_and_keeps_panes_reachable` |
| Disabled text is exempt from 1.4.3 and measured at 2.28:1 (recorded, not relied on) | `wcag_disabled_text_is_exempt_but_measured` |
| The global `/` shortcut never swallows a character or focus from a field being typed into | `a11y_keyboard_operates_the_pairing_wizard` (types `t0/ken`) |

## Quick wins landed in this slice

Only label strings and one shortcut name; no layout, colour or behaviour change:

- `accessible_name` on the filter field, the wizard's three fields, the source
  name/URL and pairing address in Settings, the connection URL/token, the
  pairing code, and the `Top height` slider. Before: the ten text fields were
  named only by hint text that disappears as soon as the user types (WCAG
  3.3.2), and the slider had no name at all (4.1.2).

## Out of scope, and why (honest mapping)

These WCAG 2.2 criteria are web-only or not applicable to a single-window native
app, and no claim is made about them:

| Criterion | Why out of scope |
|---|---|
| 1.1.1, 1.2.x (audio/video), 1.4.2, 1.4.5, 1.4.6, 1.4.7, 1.4.8, 1.4.9, 1.4.12 | no non-text content requiring alternatives, no media, no audio, no images. Graphs are decorative for this audit (their *values* are the accessible content — see A-1) |
| 1.4.13 (content on hover or focus) | not web-only: the app has nine `on_hover_text` tooltips, and egui tooltips are not hoverable, dismissible or persistent. No compliance claim is made; recorded as a gap for a later slice |
| 1.3.4, 1.4.10 (reflow/orientation), 1.4.4 browser text resize | desktop window, no document flow, no orientation lock. App zoom is measured instead (A-5); at 150–200 % the clipped values are reachable only with the mouse, so no 1.4.4 claim is made there |
| 2.5.x (pointer gestures), 3.3.x autocomplete | no touch targets and no multi-point gestures; the one pointer drag is the splitter, with `Top height` in Settings → Display as the keyboard-reachable alternative (A-8). No form autocomplete. (2.1.4 is **in scope**: see A-10) |
| 2.2.x (timing), 2.3.x (seizures), 3.2.x on load, 3.2.5, 3.3.4 | no time limits, no flashing, no re-authentication |
| 2.4.1, 2.4.5, 3.1.x, 3.2.3, 3.2.4 | single-window app: no blocks to bypass, no site navigation, one language (English, no `lang` equivalent in the tree), consistent identification is trivial. (4.1.3 is **in scope**: see A-9) |
| 1.4.3 (contrast) for disabled controls | exempt by the criterion; measured for the record |
| 2.4.7 (focus visible), 2.4.3 (focus order), 1.4.11 | **in scope** — these are the ones A-3/A-4/A-7 report on |

## Verification run (raw)

```
$ cargo test -p ebbtop-gui --locked a11y:: -- --test-threads=1
running 12 tests
a11y_every_screen_exposes_named_controls ... ok
a11y_focus_visual_on_buttons_but_not_on_titles_or_rows ... ok
a11y_keyboard_cannot_reveal_the_values_clipped_at_200_percent ... ok
a11y_keyboard_operates_the_pairing_wizard ... ok
a11y_keyboard_selects_a_row_and_reaches_kill ... ok
a11y_keyboard_zoom_works_and_persists ... ok
a11y_painted_metrics_are_not_in_the_tree ... ok
a11y_status_changes_are_not_announced ... ok
a11y_status_state_is_not_colour_only ... ok
a11y_tab_order_starts_in_the_status_bar_and_wraps ... ok
a11y_table_rows_and_splitter_have_no_role_or_name ... ok
a11y_zoom_paints_no_overlapping_text_and_keeps_panes_reachable ... ok
test result: ok. 12 passed; 0 failed

$ cargo test -p ebbtop-gui --locked wcag
running 4 tests
wcag_aa_contrast_gaps_are_pinned ... ok
wcag_aa_text_contrast_holds_for_every_passing_pair ... ok
wcag_disabled_text_is_exempt_but_measured ... ok
wcag_non_text_contrast_for_load_bars ... ok
test result: ok. 4 passed; 0 failed

$ cargo test --locked --workspace
     Running unittests src/lib.rs (ebbtop_agent)
test result: ok. 51 passed; 0 failed; 0 ignored
     Running unittests src/main.rs (ebbtop_agent)
test result: ok. 0 passed
     Running tests/cache_tests.rs (ebbtop_agent)
test result: ok. 1 passed
     Running tests/discovery.rs (ebbtop_agent)
test result: ok. 7 passed
     Running tests/pair_cli.rs (ebbtop_agent)
test result: ok. 3 passed
     Running tests/pairing_http.rs (ebbtop_agent)
test result: ok. 11 passed
     Running tests/port_parse.rs (ebbtop_agent)
test result: ok. 1 passed
     Running tests/process_details.rs (ebbtop_agent)
test result: ok. 5 passed
     Running unittests src/lib.rs (ebbtop_core)
test result: ok. 89 passed; 0 failed; 0 ignored
     Running unittests src/main.rs (the `ebbtop` binary)
test result: ok. 168 passed; 0 failed; 4 ignored
     Doc-tests ebbtop_agent / Doc-tests ebbtop_core
test result: ok. 0 passed
```

The four ignored tests are pre-existing render-evidence tests on the base tree,
not accessibility tests: `screenshot`, `frame_cost`, `e4_guidance_evidence`,
`p3b_render_evidence`. None was added or skipped by this slice.

Negative control for the naming gate: deleting `.accessible_name("Top
height")` makes `a11y_every_screen_exposes_named_controls` fail with
`settings display: input without an accessible name: ["Slider rect=..."]`; the
label was restored before the commit.

Harness note (wall-clock TTL): `narrow_bar_keeps_refusal_but_hides_success` and
`kill_result_shows_in_status_bar` assert on a successful kill reply, which the
status bar retires after `KILL_OK_TTL_MS` (15 s) of wall clock. With the
render-heavy accessibility tests in the same binary a frame can land after the
TTL, so a reply applied once at test start can already be gone when the frame
draws (measured: a reply aged 16 s is not painted at all). Both tests now re-arm
the same reply immediately before each render through one shared `kill_reply`
helper — a harness fix only; every assertion is unchanged. Stability on the
corrected bytes: three consecutive full `ebbtop-gui` runs at
`--test-threads=24` were clean (`168 passed; 0 failed; 4 ignored` each; 138 s /
143 s / 130 s). Before the fix the same command failed
`kill_result_shows_in_status_bar` in one of three runs.

## Store-listing accessibility statement (draft)

Paste into the listing's "Accessibility" field / product features. It claims
only what the audit above proves; it deliberately does **not** claim full
screen-reader support or complete keyboard parity.

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

Screen readers: buttons, text fields, check boxes, drop-downs, sliders and
dialog titles are exposed with their names and roles through the Windows UI
Automation tree (AccessKit). Connection state is shown as a word ("LIVE",
"STALE", "CONNECTING", "DOWN"), never by colour alone.

Display: the interface scales with Ctrl/Cmd + and -, and the chosen scale is
remembered between launches. Body text, headings, buttons, warnings and the
connection badges meet WCAG 2.2 AA contrast on the dark theme panels and
dialogs; two pairs of secondary grey text sit just under the bar (4.3-4.4:1) and
the selected-row highlight is below it (secondary cells at 1.98:1 on the
selection fill).

Known limitations, stated rather than glossed over: the process table's rows are
not yet announced individually by screen readers (the row data is read cell by
cell), the painted resource gauges (per-core CPU, memory/swap/disk totals, GPU
and network values) are not yet exposed to screen readers, and at 150-200% zoom
the memory and disk panes keep some values below the visible area, where only a
mouse-wheel scroll reaches them — there is no keyboard-only way to scroll that
area. A change of state (a connection going STALE, a kill being refused) is not
announced automatically either. Each limitation above is a recorded gap with a
retained test in the audit behind this statement.
```

Do not use "fully accessible", "WCAG 2.2 AA compliant", or "screen-reader
friendly" until A-1 and A-2 are fixed: with the gauges and the table rows
invisible to a reader, a blanket compliance claim would be false.

## Follow-up slices this audit asks for

1. **Rows** (A-2): role, name, selection state, bounds and a focus model for
   process and workload rows; delete
   `a11y_table_rows_and_splitter_have_no_role_or_name`.
2. **Painted gauges** (A-1): expose meter values as nodes; flip
   `a11y_painted_metrics_are_not_in_the_tree`.
3. **Dialog focus** (A-3): focus the confirm's first control on open; the pinned
   78 becomes 1.
4. **Palette** (A-4, A-6): selection fill and `DIM`; the pinned ratios move up.
5. **Zoom layout** (A-5): per-pane keyboard scrolling or reflow at scale, and
   arrow-key operation of the splitter (A-8), so the values clipped at 150–200 %
   are reachable without a pointer.
6. **Focus visuals** (A-7) and **splitter keys** (A-8).
7. **Live region** (A-9) on connection-state and kill-result changes; and scope
   the `/` shortcut to "no text field focused" or make it configurable (A-10).
