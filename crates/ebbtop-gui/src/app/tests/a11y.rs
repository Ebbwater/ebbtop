//! Headless accessibility audit: AccessKit tree, keyboard operability, focus
//! indication and text scaling.
//!
//! Every test renders the real `App` through `ctx.run_ui` with egui's AccessKit
//! generation switched on, so the assertions run against the tree a platform
//! adapter (Windows UIA on the Store build) would actually receive. No test
//! drives a pointer: this is what a keyboard and a screen reader get.
//!
//! Findings, severities and the commands behind each claim:
//! `docs/accessibility-audit.md`. Some tests pin an open gap on purpose (they
//! name it in their doc comment); the fixing slice flips them, it does not
//! relax them.

use super::*;

use egui::accesskit::{Action, NodeId, TreeUpdate};

/// One node of the AccessKit tree, flattened for assertions.
///
/// `pub(super)` so the localization tests in the sibling `l10n` module can make
/// the same assertions against a Chinese frame.
#[derive(Clone, Debug)]
pub(super) struct NodeInfo {
    pub(super) role: egui::Role,
    pub(super) label: Option<String>,
    pub(super) value: Option<String>,
    pub(super) description: Option<String>,
    pub(super) placeholder: Option<String>,
    pub(super) disabled: bool,
    pub(super) focused: bool,
    pub(super) click: bool,
    pub(super) focus: bool,
    pub(super) rect: Option<[f32; 4]>,
}

impl NodeInfo {
    /// The name a screen reader announces: `label`, else the text in `value`
    /// (egui reports `Role::Label` and `Role::TextRun` text there), else the
    /// description. A placeholder is not a name — see
    /// `named_by_placeholder_only`.
    pub(super) fn name(&self) -> Option<&str> {
        [
            self.label.as_deref(),
            self.value.as_deref(),
            self.description.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find(|s| !s.trim().is_empty())
    }

    pub(super) fn named_by_placeholder_only(&self) -> bool {
        self.name().is_none()
            && self
                .placeholder
                .as_deref()
                .is_some_and(|p| !p.trim().is_empty())
    }

    pub(super) fn role_name(&self) -> String {
        format!("{:?}", self.role)
    }

    /// A `egui_extras` table row: the row `Ui` senses clicks, so it is
    /// focusable, but `TableRow` never reports a role or a name (finding A-2).
    pub(super) fn is_table_row(&self) -> bool {
        self.role == egui::Role::GenericContainer && self.name().is_none()
    }

    /// The drag handle between the top panes and the tables: `Sense::drag()`,
    /// no role, no name, no keyboard operation (finding A-8).
    pub(super) fn is_splitter(&self) -> bool {
        self.role == egui::Role::Unknown
    }
}

/// What one frame produced.
pub(super) struct Frame {
    pub(super) nodes: Vec<NodeInfo>,
    pub(super) texts: Vec<String>,
    pub(super) shapes: Vec<egui::epaint::ClippedShape>,
}

impl Frame {
    /// Every string the tree exposes.
    pub(super) fn tree_text(&self) -> Vec<String> {
        self.nodes
            .iter()
            .flat_map(|n| [n.label.clone(), n.value.clone(), n.description.clone()])
            .flatten()
            .collect()
    }

    pub(super) fn focus(&self) -> Option<&NodeInfo> {
        self.nodes.iter().find(|n| n.focused)
    }

    pub(super) fn by_name(&self, name: &str) -> Option<&NodeInfo> {
        self.nodes.iter().find(|n| n.name() == Some(name))
    }

    /// Inputs a screen reader must be able to name, that have no name at all
    /// (a hint text is tracked separately by `named_by_placeholder_only`).
    pub(super) fn nameless_inputs(&self) -> Vec<&NodeInfo> {
        self.nodes
            .iter()
            .filter(|n| egui::accessibility::is_input(n.role))
            .filter(|n| n.name().is_none() && !n.named_by_placeholder_only())
            .collect()
    }

    /// Focusable controls a reader reaches with Tab, minus the pinned gaps.
    pub(super) fn unnamed_focusables(&self) -> Vec<&NodeInfo> {
        self.nodes
            .iter()
            .filter(|n| n.focus || n.click)
            .filter(|n| n.name().is_none())
            .filter(|n| !n.is_table_row() && !n.is_splitter())
            // egui's scrollbars carry the ScrollBar role and no label; UIA
            // announces the role, and WCAG asks for a name on controls whose
            // purpose the role alone does not convey.
            .filter(|n| n.role != egui::Role::ScrollBar)
            .collect()
    }

    /// Painted text that survives its clip rect (what the user can see).
    pub(super) fn visible_texts(&self) -> Vec<(egui::Rect, String)> {
        let mut out = vec![];
        for c in &self.shapes {
            if let egui::Shape::Text(t) = &c.shape {
                let text = t.galley.text().trim().to_string();
                if text.is_empty() {
                    continue;
                }
                let vis = t.visual_bounding_rect().intersect(c.clip_rect);
                if vis.width() > 0.5 && vis.height() > 0.5 {
                    out.push((vis, text));
                }
            }
        }
        out
    }

    /// Per-shape census of painted text: how many runs survive their clip rect
    /// and how many are painted entirely outside it. This is the evidence for
    /// the report's zoom table (A-5); the numbers are pinned in
    /// `a11y_zoom_paints_no_overlapping_text_and_keeps_panes_reachable`.
    pub(super) fn text_census(&self) -> (usize, usize) {
        let mut visible = 0;
        let mut clipped = 0;
        for c in &self.shapes {
            if let egui::Shape::Text(t) = &c.shape {
                if t.galley.text().trim().is_empty() {
                    continue;
                }
                let vis = t.visual_bounding_rect().intersect(c.clip_rect);
                if vis.width() > 0.5 && vis.height() > 0.5 {
                    visible += 1;
                } else {
                    clipped += 1;
                }
            }
        }
        (visible, clipped)
    }

    /// Visible text runs that overlap another visible run.
    pub(super) fn visible_text_overlaps(&self) -> Vec<String> {
        let texts = self.visible_texts();
        let mut out = vec![];
        for i in 0..texts.len() {
            for j in i + 1..texts.len() {
                let hit = texts[i].0.intersect(texts[j].0);
                if hit.width() > 1.0 && hit.height() > 1.0 {
                    out.push(format!("{:?} x {:?}", texts[i].1, texts[j].1));
                }
            }
        }
        out
    }
}

/// A persistent headless context with AccessKit on, so focus, scroll offsets
/// and the tree survive between frames like they do in a running app.
pub(super) struct Walk {
    pub(super) ctx: egui::Context,
}

impl Walk {
    pub(super) fn new() -> Self {
        Self::at_zoom(1.0)
    }

    pub(super) fn at_zoom(zoom: f32) -> Self {
        let ctx = egui::Context::default();
        style(&ctx);
        ctx.enable_accesskit();
        ctx.set_zoom_factor(zoom);
        Self { ctx }
    }

    /// One frame. `screen` is the physical window size; the points handed to
    /// egui shrink as the zoom factor grows, exactly like a real window.
    pub(super) fn frame(
        &mut self,
        app: &mut App,
        screen: (f32, f32),
        events: Vec<egui::Event>,
    ) -> Frame {
        self.frame_age(app, screen, events, Some(0))
    }

    pub(super) fn frame_age(
        &mut self,
        app: &mut App,
        screen: (f32, f32),
        events: Vec<egui::Event>,
        age_ms: Option<u64>,
    ) -> Frame {
        let zoom = self.ctx.zoom_factor();
        if let Some(age) = age_ms {
            let shared = app.shared();
            let mut s = shared.lock().unwrap();
            if s.last_new_sample_recv_ms != 0 {
                s.last_new_sample_recv_ms = client::now_ms().saturating_sub(age);
            }
        }
        let points = egui::vec2(screen.0 / zoom, screen.1 / zoom);
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, points)),
            focused: true,
            ..Default::default()
        };
        input.events = events;
        let mut out = self.ctx.run_ui(input, |ui| app.draw(ui));
        out.textures_delta.clear(); // no renderer in tests
        let update = out
            .platform_output
            .accesskit_update
            .clone()
            .unwrap_or(TreeUpdate {
                nodes: vec![],
                tree: None,
                tree_id: egui::accesskit::TreeId::ROOT,
                focus: NodeId(0),
            });
        Frame {
            nodes: nodes_of(&update),
            texts: texts(&out.shapes),
            shapes: out.shapes,
        }
    }

    /// Three settled frames, like the existing render helpers.
    pub(super) fn settle(&mut self, app: &mut App, screen: (f32, f32)) -> Frame {
        let mut last = self.frame(app, screen, vec![]);
        for _ in 0..2 {
            last = self.frame(app, screen, vec![]);
        }
        last
    }

    pub(super) fn tab(&mut self, app: &mut App, screen: (f32, f32)) -> Frame {
        self.frame(app, screen, vec![key(egui::Key::Tab)])
    }

    /// Tab until the focused node matches `want`. Returns the stop index and
    /// the frame of the *following* frame, because egui paints focus one frame
    /// after the key press (the tree already reports it in the press frame).
    pub(super) fn focus_matching(
        &mut self,
        app: &mut App,
        screen: (f32, f32),
        max: usize,
        want: impl Fn(&NodeInfo) -> bool,
    ) -> Option<(usize, Frame)> {
        for i in 0..max {
            let f = self.tab(app, screen);
            if f.focus().is_some_and(&want) {
                let settled = self.frame(app, screen, vec![]);
                return Some((i, settled));
            }
        }
        None
    }
}

fn nodes_of(update: &TreeUpdate) -> Vec<NodeInfo> {
    update
        .nodes
        .iter()
        .map(|(id, node)| NodeInfo {
            role: node.role(),
            label: node.label().map(str::to_string),
            value: node.value().map(str::to_string),
            description: node.description().map(str::to_string),
            placeholder: node.placeholder().map(str::to_string),
            disabled: node.is_disabled(),
            focused: *id == update.focus,
            click: node.supports_action(Action::Click),
            focus: node.supports_action(Action::Focus),
            rect: node
                .bounds()
                .map(|b| [b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32]),
        })
        .collect()
}

fn key(k: egui::Key) -> egui::Event {
    egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }
}

fn text(s: &str) -> egui::Event {
    egui::Event::Text(s.to_string())
}

fn to_rect(r: [f32; 4]) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(r[0], r[1]), egui::pos2(r[2], r[3]))
}

/// Painted primitives inside `rect`, as text, so two frames can be diffed.
fn paint_in(shapes: &[egui::epaint::ClippedShape], rect: egui::Rect) -> Vec<String> {
    fn walk(s: &egui::Shape, rect: egui::Rect, out: &mut Vec<String>) {
        match s {
            egui::Shape::Rect(r) if rect.intersects(r.rect) => {
                out.push(format!("rect fill={:?} stroke={:?}", r.fill, r.stroke))
            }
            egui::Shape::LineSegment { points, stroke }
                if points.iter().any(|p| rect.contains(*p)) =>
            {
                out.push(format!("line stroke={stroke:?}"))
            }
            egui::Shape::Text(t) if rect.intersects(t.visual_bounding_rect()) => out.push(format!(
                "text {:?} color={:?}",
                t.galley.text(),
                t.override_text_color.or(Some(t.fallback_color))
            )),
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, rect, out)),
            _ => {}
        }
    }
    let mut out = vec![];
    shapes.iter().for_each(|c| walk(&c.shape, rect, &mut out));
    out
}

/// Dashboard with the fixture sample: 401 processes, a hottest pid with detail
/// history, one disk. `onboard` cleared so the wizard does not cover it.
pub(super) fn dashboard() -> App {
    let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
    app.onboard = None;
    fake_state(&app, client::now_ms());
    app
}

/// Every screen the card names, in the state a user meets it.
fn screens() -> Vec<(&'static str, App)> {
    let mut out: Vec<(&'static str, App)> = vec![];

    out.push(("dashboard", dashboard()));

    let mut sel = dashboard();
    sel.selected = Some(4242);
    out.push(("dashboard + detail", sel));

    let mut wiz = App::with(Config::default(), None, TokenSource::Missing, None);
    wiz.onboard = Some(Onboard::default());
    out.push(("onboarding", wiz));

    let mut paired = App::with(Config::default(), None, TokenSource::Missing, None);
    paired.onboard = Some(Onboard {
        name: "lab".into(),
        url: "127.0.0.1:1".into(),
        pair: Some(Pairing::for_address("lab", "127.0.0.1:1")),
        ..Onboard::default()
    });
    out.push(("onboarding + pairing code", paired));

    let tabs: Vec<(&str, SettingsTab)> = {
        let mut v = vec![
            ("settings sources", SettingsTab::Sources),
            ("settings display", SettingsTab::Display),
            ("settings connection", SettingsTab::Connection),
        ];
        // The Updates tab exists only in updater builds: the Store package
        // compiles the updater out, and SettingsTab::Updates is cfg-gated in
        // app.rs, so this scene is absent from store-feature test runs.
        #[cfg(not(feature = "store"))]
        v.push(("settings updates", SettingsTab::Updates));
        v
    };
    for (name, tab) in tabs {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        app.onboard = None;
        app.settings_open = true;
        app.settings_tab = tab;
        out.push((name, app));
    }

    let mut kill = dashboard();
    kill.selected = Some(4242);
    kill.kill_confirm = Some(KillConfirm {
        pid: 4242,
        name: "hottest-proc".into(),
        start_time_ms: 1,
    });
    out.push(("kill confirm", kill));

    // The update dialog is an updater-build scene: the App::update field it
    // drives is cfg-gated out of store builds.
    #[cfg(not(feature = "store"))]
    {
        let mut upd = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        upd.onboard = None;
        upd.update.start(|| Ok(r#"{"tag_name": "v99.7.0"}"#.into()));
        settle_update(&mut upd);
        out.push(("update dialog", upd));
    }

    out
}

// ---------------------------------------------------------------- tests ---

/// Invariant: every screen builds a tree, and every control a reader reaches
/// has an accessible name. Only the pinned gaps (table rows, the splitter) are
/// exempt, and they are asserted separately by
/// `a11y_table_rows_and_splitter_have_no_role_or_name`.
#[test]
fn a11y_every_screen_exposes_named_controls() {
    let mut w = Walk::new();
    for (name, mut app) in screens() {
        let f = w.settle(&mut app, (960.0, 1000.0));
        assert!(!f.nodes.is_empty(), "{name}: empty accessibility tree");
        assert!(
            f.nodes.iter().any(|n| n.name().is_some()),
            "{name}: no named node at all"
        );
        let nameless: Vec<String> = f
            .nameless_inputs()
            .iter()
            .map(|n| {
                format!(
                    "{} rect={:?} placeholder={:?}",
                    n.role_name(),
                    n.rect,
                    n.placeholder
                )
            })
            .collect();
        assert!(
            nameless.is_empty(),
            "{name}: input without an accessible name: {nameless:?}"
        );
        let unnamed: Vec<String> = f
            .unnamed_focusables()
            .iter()
            .map(|n| n.role_name())
            .collect();
        assert!(
            unnamed.is_empty(),
            "{name}: focusable control without a name: {unnamed:?}"
        );
    }
}

/// Finding A-2 / A-8 (pinned gap): `egui_extras` table rows and the drag
/// handle are focusable but carry no role or name, so Tab lands on anonymous
/// nodes and a screen reader announces nothing. The fixing slice names them
/// (role `Cell`/`Row` plus the row text) and deletes this test.
#[test]
fn a11y_table_rows_and_splitter_have_no_role_or_name() {
    let mut w = Walk::new();
    let mut app = dashboard();
    let f = w.settle(&mut app, (960.0, 1000.0));
    let rows = f.nodes.iter().filter(|n| n.is_table_row()).count();
    let splitters = f.nodes.iter().filter(|n| n.is_splitter()).count();
    assert!(rows > 0, "expected focusable unnamed table rows");
    assert_eq!(splitters, 1, "expected exactly one unnamed drag handle");
    // They also carry no bounds: a screen reader cannot say where the focused
    // row is, and UIA hit-testing has nothing to point at.
    let unlocated = f
        .nodes
        .iter()
        .filter(|n| n.is_table_row())
        .filter(|n| {
            n.rect
                .is_none_or(|r| !r[0].is_finite() || r[2] <= r[0] || r[3] <= r[1])
        })
        .count();
    assert_eq!(
        unlocated, rows,
        "table rows must stay unlocated until A-2 is fixed"
    );

    // The rows are what Tab walks through: most stops in a cycle are anonymous.
    let mut w = Walk::new();
    let mut app = dashboard();
    w.settle(&mut app, (960.0, 1000.0));
    let mut unnamed_stops = 0;
    for _ in 0..60 {
        let f = w.tab(&mut app, (960.0, 1000.0));
        if f.focus().is_some_and(|n| n.name().is_none()) {
            unnamed_stops += 1;
        }
    }
    assert!(
        unnamed_stops > 30,
        "expected the table to dominate the tab order, got {unnamed_stops} unnamed stops"
    );
}

/// Finding A-1 (pinned gap): `w::meter` and `w::cpu_bar_cell` paint their label
/// and value with `painter.galley`, which never creates an AccessKit node. A
/// screen reader therefore cannot read the dashboard's numbers: per-core CPU,
/// load, VRAM, memory/swap/disk totals and every row's CPU%. The fixing slice
/// exposes those strings and inverts this assertion.
#[test]
fn a11y_painted_metrics_are_not_in_the_tree() {
    let mut w = Walk::new();
    let mut app = dashboard();
    let f = w.settle(&mut app, (960.0, 1000.0));
    let tree = f.tree_text();
    let placeholders: Vec<&str> = f
        .nodes
        .iter()
        .filter_map(|n| n.placeholder.as_deref())
        .collect();
    let mut missing: Vec<String> = f
        .texts
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .filter(|t| !tree.iter().any(|s| s.contains(t)))
        .filter(|t| !placeholders.iter().any(|p| p.contains(t.as_str())))
        .collect();
    missing.sort();
    missing.dedup();

    /// What the painted widgets draw and the tree cannot see.
    const PAINTED_ONLY: &[&str] = &[
        "0.1",
        "1.0 GiB / 4.0 GiB",
        "1.0 GiB / 8.0 GiB",
        "11%",
        "12.0 GiB / 32.0 GiB",
        "127.5",
        "18%",
        "25%",
        "32%",
        "37%",
        "39%",
        "4%",
        "46%",
        "512 GiB / 1.0 TiB",
        "69%",
        "76%",
        "83%",
        "87.5",
        "90%",
        "97%",
        "C0",
        "C1",
        "C10",
        "C11",
        "C2",
        "C3",
        "C4",
        "C5",
        "C6",
        "C7",
        "C8",
        "C9",
        "Load",
        "Swap",
        "Used",
        "VRAM",
    ];
    let want: Vec<String> = PAINTED_ONLY.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        missing, want,
        "the painted-only inventory moved; update the finding, do not widen it"
    );

    // Everything else that is painted is exposed: the words a sighted user
    // reads are the words the tree carries.
    for expected in [
        "LIVE",
        "PID",
        "Name",
        "CPU%",
        "MEM%",
        "401 of 401",
        "v proc",
        "v workloads",
    ] {
        assert!(
            tree.iter().any(|s| s.contains(expected)),
            "{expected:?} is painted but not exposed: {tree:?}"
        );
    }
}

/// Invariant (passes): the keyboard alone selects a process, reaches the Kill
/// action and confirms it. This is the walkthrough behind the Store
/// statement's keyboard claim: ArrowDown selects, Tab reaches Kill in 13
/// presses, Space opens the dialog, Space on "Send TERM" runs the kill path.
#[test]
fn a11y_keyboard_selects_a_row_and_reaches_kill() {
    const SCREEN: (f32, f32) = (960.0, 1000.0);
    let mut w = Walk::new();
    let mut app = dashboard();
    w.settle(&mut app, SCREEN);

    // Arrow keys select in the visible (filtered, sorted) order.
    let f = w.frame(&mut app, SCREEN, vec![key(egui::Key::ArrowDown)]);
    assert_eq!(
        app.selected,
        Some(4242),
        "ArrowDown must select the first row"
    );
    assert!(
        !f.texts.iter().any(|t| t == "v proc-detail"),
        "the detail pane is drawn before the table, so it appears on the next frame"
    );
    w.frame(&mut app, SCREEN, vec![]);

    // Tab reaches the Kill button without a pointer.
    let (stop, _) = w
        .focus_matching(&mut app, SCREEN, 20, |n| n.name() == Some("Kill"))
        .expect("Kill must be reachable with Tab");
    assert_eq!(
        stop, 13,
        "documented tab order changed: Kill used to be stop 14 (index 13)"
    );

    // Space activates the focused button and opens the confirm dialog.
    let _ = w.frame(&mut app, SCREEN, vec![key(egui::Key::Space)]);
    assert_eq!(
        app.kill_confirm.as_ref().map(|k| k.pid),
        Some(4242),
        "Space on Kill must open the confirm"
    );
    let f = w.frame(&mut app, SCREEN, vec![]);
    assert!(
        f.texts.iter().any(|t| t.contains("Kill process")),
        "confirm dialog did not render"
    );
    assert!(
        f.texts
            .iter()
            .any(|t| t.contains("Send SIGTERM to \"hottest-proc\" (pid 4242)")),
        "confirm dialog must name the process and pid"
    );

    // Tab into the dialog, choose TERM, and confirm. Finding A-3: egui does
    // not move focus into the window, and the dialog's widgets come last in
    // the frame, so the confirm sits behind every visible table row — 78
    // presses from Kill. The fixing slice focuses the dialog on open and this
    // number drops to 1.
    let (stop, _) = w
        .focus_matching(&mut app, SCREEN, 200, |n| n.name() == Some("Send TERM"))
        .expect("Send TERM must be reachable with Tab");
    assert_eq!(
        stop, 77,
        "the confirm dialog moved in the tab order: it used to be press 78"
    );
    w.frame(&mut app, SCREEN, vec![key(egui::Key::Space)]);
    assert!(
        app.kill_confirm.is_none(),
        "confirming must close the dialog"
    );
    let note = app.kill_note.clone().unwrap_or_default();
    assert!(
        note.starts_with("kill sent: pid 4242") || note.starts_with("kill unavailable"),
        "Space on Send TERM must run the kill path, got {note:?}"
    );
}

/// Invariant (passes): the tab order starts at the status bar, visits every
/// pane title once, and wraps — no keyboard trap.
///
/// This test also owns the tab census behind finding A-2: of 400 presses, 337
/// land on anonymous nodes and 332 of those are `egui_extras` table rows. The
/// counts are pinned exactly; a change in the row/tab model must update the
/// finding, not widen it.
#[test]
fn a11y_tab_order_starts_in_the_status_bar_and_wraps() {
    const SCREEN: (f32, f32) = (960.0, 1000.0);
    let mut w = Walk::new();
    let mut app = dashboard();
    w.settle(&mut app, SCREEN);
    let mut stops: Vec<String> = vec![];
    let mut wrap_at = None;
    let mut unnamed_stops = 0usize;
    let mut row_stops = 0usize;
    for i in 0..400 {
        let f = w.tab(&mut app, SCREEN);
        let what = f
            .focus()
            .map(|n| format!("{} {:?}", n.role_name(), n.name()))
            .unwrap_or_else(|| "NONE".into());
        if f.focus().is_some_and(|n| n.name().is_none()) {
            unnamed_stops += 1;
        }
        if f.focus().is_some_and(NodeInfo::is_table_row) {
            row_stops += 1;
        }
        if i > 0 && wrap_at.is_none() && what == stops[0] {
            wrap_at = Some(i);
        }
        stops.push(what);
    }
    let wrap_at = wrap_at.unwrap_or_else(|| {
        panic!(
            "Tab never wrapped inside 400 presses ({} stops): focus order may be trapped",
            stops.len()
        )
    });
    assert_eq!(stops[0], "Button Some(\"Settings\")", "first stop changed");
    assert_eq!(
        stops[wrap_at], stops[0],
        "the wrap must return to the first stop"
    );
    eprintln!(
        "tab census: {unnamed_stops}/400 unnamed stops, {row_stops} of them table rows (wrap at press {wrap_at})"
    );
    assert_eq!(
        unnamed_stops, 337,
        "the unnamed-stop census moved; update A-2's numbers instead of widening the finding"
    );
    assert_eq!(
        row_stops, 332,
        "the table-row stop census moved; update A-2's numbers instead of widening the finding"
    );
}

/// Finding A-7 (pinned gap): focus is visibly indicated on buttons —
/// egui maps `has_focus` to `widgets.active` (white 1px frame, white text) —
/// but nothing changes on a focused panel title or a focused table row, so a
/// keyboard user cannot see where they are while Tab walks the table. The
/// fixing slice adds a focus visual there and inverts these assertions.
#[test]
fn a11y_focus_visual_on_buttons_but_not_on_titles_or_rows() {
    const SCREEN: (f32, f32) = (960.0, 1000.0);
    let mut w = Walk::new();
    let mut app = dashboard();
    let base = w.settle(&mut app, SCREEN);

    let button = to_rect(
        base.by_name("Settings")
            .and_then(|n| n.rect)
            .expect("Settings button"),
    );
    let title = to_rect(
        base.nodes
            .iter()
            .find(|n| n.name().is_some_and(|s| s.starts_with("v cpu")))
            .and_then(|n| n.rect)
            .expect("cpu title"),
    );
    let before_button = paint_in(&base.shapes, button);
    let before_title = paint_in(&base.shapes, title);

    // Button: focus paints a white frame (widgets.active) — visible.
    let (_, focused) = w
        .focus_matching(&mut app, SCREEN, 4, |n| n.name() == Some("Settings"))
        .expect("Settings must be focusable");
    let after_button = paint_in(&focused.shapes, button);
    assert_ne!(
        before_button, after_button,
        "a focused button must look different: {before_button:?} vs {after_button:?}"
    );
    assert!(
        after_button.iter().any(|p| p.contains("#FF_FF_FF")),
        "the focused button's frame must be white: {after_button:?}"
    );

    // Panel title: focus repaints the text pure white and nothing else — a
    // colour-only cue on a header that already reads as a link (finding A-7).
    let mut w = Walk::new();
    let mut app = dashboard();
    w.settle(&mut app, SCREEN);
    let (_, focused) = w
        .focus_matching(&mut app, SCREEN, 6, |n| {
            n.name().is_some_and(|s| s.starts_with("v cpu"))
        })
        .expect("panel title must be focusable");
    let after_title = paint_in(&focused.shapes, title);
    assert_ne!(
        before_title, after_title,
        "a focused panel title must show something"
    );
    let whites: Vec<&String> = after_title
        .iter()
        .filter(|p| p.contains("#FF_FF_FF"))
        .collect();
    assert_eq!(
        whites.len(),
        1,
        "pinned gap A-7: the title's only focus cue is its text turning white: {after_title:?}"
    );
    assert!(
        before_title.iter().all(|p| !p.contains("#FF_FF_FF")),
        "an unfocused title must not paint white text"
    );

    // Drag handle: it has real bounds, is focusable, and paints the same line
    // focused or not — no focus indicator (finding A-7).
    let mut w = Walk::new();
    let mut app = dashboard();
    let base = w.settle(&mut app, SCREEN);
    let handle = to_rect(
        base.nodes
            .iter()
            .find(|n| n.role == egui::Role::Unknown)
            .and_then(|n| n.rect)
            .expect("the splitter must have bounds"),
    );
    let before = paint_in(&base.shapes, handle);
    let (_, focused) = w
        .focus_matching(&mut app, SCREEN, 20, |n| n.role == egui::Role::Unknown)
        .expect("the splitter must be focusable");
    let after = paint_in(&focused.shapes, handle);
    assert_eq!(
        before, after,
        "pinned gap A-7: a focused splitter paints no focus indicator"
    );
}

/// Invariant (passes): LIVE and STALE are words as well as colours, and the
/// stale frame also states the sample age — the state never depends on hue
/// alone (WCAG 1.4.1).
#[test]
fn a11y_status_state_is_not_colour_only() {
    let mut w = Walk::new();
    let mut app = dashboard();
    let f = w.settle(&mut app, (960.0, 1000.0));
    assert!(
        f.tree_text().iter().any(|t| t.contains("LIVE")),
        "LIVE must be exposed as text"
    );

    let mut app = dashboard();
    fake_state(&app, client::now_ms() - 5_000);
    let mut w = Walk::new();
    let f = w.frame_age(&mut app, (960.0, 1000.0), vec![], Some(5_000));
    let tree = f.tree_text();
    assert!(
        tree.iter().any(|t| t.contains("STALE")),
        "STALE must be exposed as text: {tree:?}"
    );
    assert!(
        tree.iter().any(|t| t.starts_with("age ")),
        "the stale frame must also state the sample age in words: {tree:?}"
    );
    assert!(
        !tree.iter().any(|t| t.contains("LIVE")),
        "STALE and LIVE must not both show"
    );
}

/// Invariant (passes) with one pinned fact: at 150% and 200% zoom no visible
/// text overlaps and the panes stay reachable. Measured: at 200% the
/// memory/disk meters start clipped (painted outside the top pane's clip
/// rect). The wheel scroll that reveals them is pointer-only — the keyboard
/// path is pinned separately by
/// `a11y_keyboard_cannot_reveal_the_values_clipped_at_200_percent`, so the
/// audit does not claim WCAG 1.4.4 for keyboard users. The per-shape census
/// is pinned here: it is the evidence for A-5's table.
#[test]
fn a11y_zoom_paints_no_overlapping_text_and_keeps_panes_reachable() {
    let mut census = vec![];
    for zoom in [1.0f32, 1.5, 2.0] {
        let mut w = Walk::at_zoom(zoom);
        let mut app = dashboard();
        let f = w.settle(&mut app, (960.0, 1000.0));
        census.push((zoom, f.text_census()));
        let overlaps = f.visible_text_overlaps();
        assert!(
            overlaps.is_empty(),
            "zoom {zoom}: overlapping text {overlaps:?}"
        );
        let visible: Vec<String> = f.visible_texts().into_iter().map(|(_, t)| t).collect();
        for expected in ["EbbTop", "Settings", "v cpu"] {
            assert!(
                visible.iter().any(|t| t.contains(expected)),
                "zoom {zoom}: {expected:?} is not visible: {visible:?}"
            );
        }
        let proc_visible = visible.iter().any(|t| t.starts_with("v proc"));
        assert!(
            proc_visible || app.group_scroll_max > 0.0,
            "zoom {zoom}: proc table unreachable"
        );
    }

    // At 200% the mem/disk meters are clipped until the top pane is scrolled.
    let visible =
        |f: &Frame| -> Vec<String> { f.visible_texts().into_iter().map(|(_, t)| t).collect() };
    let mut w = Walk::at_zoom(2.0);
    let mut app = dashboard();
    let f = w.settle(&mut app, (960.0, 1000.0));
    assert!(
        !visible(&f).iter().any(|t| t == "Used"),
        "the 200% clipping finding changed: mem values are visible without scrolling"
    );
    let mut events = vec![egui::Event::PointerMoved(egui::pos2(200.0, 150.0))];
    for _ in 0..10 {
        events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -60.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: Default::default(),
        });
    }
    w.frame(&mut app, (960.0, 1000.0), events);
    let mut f = w.frame(&mut app, (960.0, 1000.0), vec![]);
    for _ in 0..3 {
        f = w.frame(&mut app, (960.0, 1000.0), vec![]);
    }
    assert!(
        visible(&f).iter().any(|t| t == "Used"),
        "scrolling the top pane must reveal the clipped mem values: {:?}",
        visible(&f)
    );

    // The census behind the report's zoom table (visible, fully clipped).
    eprintln!("zoom census (visible, clipped): {census:?}");
    assert_eq!(
        census,
        vec![(1.0, (174, 1)), (1.5, (88, 9)), (2.0, (66, 17))],
        "the zoom text census moved; update A-5's table instead of widening the finding"
    );
}

/// Finding A-5 / A-8 (pinned gap): the values clipped at 150-200% zoom are
/// recoverable with the mouse wheel — and with nothing else. Focus the pane
/// titles and the log-scale box and press every scrolling key, then press the
/// global keys: none of them reveals `Used`. The splitter is drag-only
/// (`Sense::drag()`, app.rs), so a keyboard-only user cannot scroll the top
/// pane either. The fixing slice adds per-pane keyboard scrolling (or reflow)
/// and the arrow-key splitter, then inverts these assertions.
#[test]
fn a11y_keyboard_cannot_reveal_the_values_clipped_at_200_percent() {
    const SCREEN: (f32, f32) = (960.0, 1000.0);
    const KEYS: [egui::Key; 6] = [
        egui::Key::ArrowDown,
        egui::Key::PageDown,
        egui::Key::End,
        egui::Key::Space,
        egui::Key::ArrowUp,
        egui::Key::PageUp,
    ];
    let visible =
        |f: &Frame| -> Vec<String> { f.visible_texts().into_iter().map(|(_, t)| t).collect() };

    for target in ["v mem", "v disk", "log scale"] {
        let mut w = Walk::at_zoom(2.0);
        let mut app = dashboard();
        let f = w.settle(&mut app, SCREEN);
        assert!(
            !visible(&f).iter().any(|t| t == "Used"),
            "setup: Used must start clipped at 200%"
        );
        let (stop, _) = w
            .focus_matching(&mut app, SCREEN, 16, |n| match n.name() {
                Some(name) if target.starts_with("v ") => name.starts_with(target),
                Some(name) => name == target,
                None => false,
            })
            .unwrap_or_else(|| panic!("{target} must be focusable with Tab"));
        for k in KEYS {
            w.frame(&mut app, SCREEN, vec![key(k)]);
            let f = w.frame(&mut app, SCREEN, vec![]);
            assert!(
                !visible(&f).iter().any(|t| t == "Used"),
                "pinned gap A-5: {target} + {k:?} revealed the clipped values — add keyboard recovery and flip this"
            );
        }
        eprintln!(
            "keyboard-scroll probe: Tab stop {} = {target}; {} keys never reveal Used",
            stop + 1,
            KEYS.len()
        );
    }

    // Global keys with nothing but the default focus.
    let mut w = Walk::at_zoom(2.0);
    let mut app = dashboard();
    w.settle(&mut app, SCREEN);
    for k in [egui::Key::PageDown, egui::Key::End, egui::Key::ArrowDown] {
        w.frame(&mut app, SCREEN, vec![key(k)]);
        let f = w.frame(&mut app, SCREEN, vec![]);
        assert!(
            !visible(&f).iter().any(|t| t == "Used"),
            "pinned gap A-5: the global {k:?} shortcut revealed the clipped values — flip this"
        );
    }

    // The splitter between the panes is focusable but drag-only (A-8): arrow
    // keys do not move the ratio and do not scroll anything.
    let mut w = Walk::at_zoom(2.0);
    let mut app = dashboard();
    w.settle(&mut app, SCREEN);
    let before = app.cfg.top_frac;
    w.focus_matching(&mut app, SCREEN, 20, |n| n.role == egui::Role::Unknown)
        .expect("the splitter must be focusable");
    for k in [
        egui::Key::ArrowDown,
        egui::Key::ArrowUp,
        egui::Key::PageDown,
    ] {
        w.frame(&mut app, SCREEN, vec![key(k)]);
    }
    assert_eq!(
        app.cfg.top_frac, before,
        "pinned gap A-8: the splitter gained keyboard operation — flip this"
    );
    let f = w.frame(&mut app, SCREEN, vec![]);
    assert!(
        !visible(&f).iter().any(|t| t == "Used"),
        "pinned gap A-8: a focused splitter cannot scroll the panes"
    );
}

/// Invariant (passes): zoom is a real feature of this build — the egui
/// shortcuts work (Ctrl/Cmd + = / - / 0), the factor lives in `Options`, eframe
/// persists egui memory, and the factor survives a serialization round trip
/// (what the app.ron store writes).
#[test]
fn a11y_keyboard_zoom_works_and_persists() {
    let cmd = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
    let press = |k| egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: cmd,
    };
    let mut w = Walk::new();
    let mut app = dashboard();
    w.settle(&mut app, (960.0, 1000.0));
    assert_eq!(w.ctx.zoom_factor(), 1.0);

    for (k, want) in [
        (egui::Key::Equals, 1.1),
        (egui::Key::Plus, 1.2),
        (egui::Key::Minus, 1.1),
        (egui::Key::Num0, 1.0),
    ] {
        w.frame(&mut app, (960.0, 1000.0), vec![press(k)]);
        w.frame(&mut app, (960.0, 1000.0), vec![]);
        assert_eq!(
            w.ctx.zoom_factor(),
            want,
            "shortcut {k:?} must set zoom {want}"
        );
    }

    w.ctx.set_zoom_factor(1.7);
    w.frame(&mut app, (960.0, 1000.0), vec![]);
    let opts = w.ctx.memory(|m| m.options.clone());
    assert_eq!(
        opts.zoom_factor, 1.7,
        "the zoom factor must live in egui Options"
    );
    let json = serde_json::to_string(&opts).expect("Options must serialize");
    let back: egui::Options = serde_json::from_str(&json).expect("Options must deserialize");
    assert_eq!(
        back.zoom_factor, 1.7,
        "zoom must survive a persistence round trip"
    );
    assert!(
        eframe::App::persist_egui_memory(&app),
        "eframe must persist egui memory"
    );
}

/// Invariant (passes): the pairing wizard is operable from the keyboard alone —
/// type the three fields, open the code prompt, type a code, start the
/// exchange. `Save and connect` stays disabled because no green Test has run,
/// so keyboard use does not bypass the Save gate.
#[test]
fn a11y_keyboard_operates_the_pairing_wizard() {
    const SCREEN: (f32, f32) = (960.0, 1000.0);
    let mut w = Walk::new();
    let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
    app.onboard = Some(Onboard::default());
    w.settle(&mut app, SCREEN);

    w.focus_matching(&mut app, SCREEN, 30, |n| n.name() == Some("Name"))
        .expect("the Name field must be reachable with Tab");
    w.frame(&mut app, SCREEN, vec![text("lab")]);
    assert_eq!(app.onboard.as_ref().map(|o| o.name.as_str()), Some("lab"));

    w.focus_matching(&mut app, SCREEN, 4, |n| n.name() == Some("URL"))
        .expect("the URL field must follow the Name field");
    w.frame(&mut app, SCREEN, vec![text("127.0.0.1:1")]);
    assert_eq!(
        app.onboard.as_ref().map(|o| o.url.as_str()),
        Some("127.0.0.1:1")
    );

    w.focus_matching(&mut app, SCREEN, 4, |n| n.name() == Some("Agent token"))
        .expect("the token field must follow the URL field");
    // The global `/` shortcut focuses the filter; it must not steal the
    // character or the focus while a text field is being typed into (A-10).
    for ev in [text("t0"), key(egui::Key::Slash), text("/"), text("ken")] {
        w.frame(&mut app, SCREEN, vec![ev]);
    }
    assert_eq!(
        app.onboard.as_ref().map(|o| o.token.as_str()),
        Some("t0/ken")
    );
    let f = w.frame(&mut app, SCREEN, vec![]);
    assert_eq!(
        f.focus().and_then(|n| n.name()),
        Some("Agent token"),
        "typing `/` must not move focus out of the field"
    );

    w.focus_matching(&mut app, SCREEN, 8, |n| {
        n.name() == Some("Use a pairing code")
    })
    .expect("Use a pairing code must be reachable with Tab");
    w.frame(&mut app, SCREEN, vec![key(egui::Key::Space)]);
    assert!(
        app.onboard.as_ref().and_then(|o| o.pair.as_ref()).is_some(),
        "the code step must open"
    );
    w.frame(&mut app, SCREEN, vec![]);
    w.focus_matching(&mut app, SCREEN, 30, |n| n.name() == Some("Pairing code"))
        .expect("the pairing-code field must be reachable with Tab");
    w.frame(&mut app, SCREEN, vec![text("123456")]);
    assert_eq!(
        app.onboard
            .as_ref()
            .and_then(|o| o.pair.as_ref())
            .map(|p| p.code.clone()),
        Some("123456".to_string())
    );

    // Start the exchange. The address has no listener, so the worker fails on
    // its own thread; the keyboard path is what this test proves.
    w.focus_matching(&mut app, SCREEN, 6, |n| n.name() == Some("Pair"))
        .expect("Pair must be reachable with Tab");
    w.frame(&mut app, SCREEN, vec![key(egui::Key::Space)]);
    assert!(
        app.onboard
            .as_ref()
            .and_then(|o| o.pair.as_ref())
            .is_some_and(|p| p.busy),
        "Space on Pair must start the exchange"
    );

    let f = w.frame(&mut app, SCREEN, vec![]);
    let save = f
        .by_name("Save and connect")
        .expect("Save and connect must be in the tree");
    assert!(save.disabled, "Save must stay disabled until a green Test");
}

/// Finding A-9 (pinned gap): state changes are drawn, not announced. The
/// connection badge, the kill result and the update prompt all appear as plain
/// static text; the tree carries no `Alert`/live node and focus never moves to
/// them, so a screen-reader user is not told that anything changed (WCAG 4.1.3).
/// The fixing slice adds a live/alert node (or focus) on change and inverts this.
#[test]
fn a11y_status_changes_are_not_announced() {
    let mut w = Walk::new();
    let mut app = dashboard();
    let f = w.settle(&mut app, (960.0, 1000.0));
    assert!(
        !f.nodes.iter().any(|n| n.role == egui::Role::Alert),
        "pinned gap A-9: a live region appeared — flip this assertion"
    );
    assert!(
        f.nodes.iter().any(|n| n.name() == Some(" LIVE ")
            && matches!(n.role, egui::Role::Label | egui::Role::TextRun)),
        "the badge is static text today"
    );
}
