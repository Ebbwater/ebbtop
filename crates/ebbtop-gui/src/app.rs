//! The window. Reads shared `State`, which the client task updates.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use ebbtop_core::client::{self, ClientConfig};
use ebbtop_core::history::{Conn, NetRate, Ring, State};
use ebbtop_core::update as core_update;
use ebbtop_core::view::{self, SortKey};
use ebbtop_core::wire::{KillJob, KillSignal};
use tokio::sync::mpsc;

use crate::store::{self, Config, TokenSource};
use crate::update::{self, UpdateCheck, UpdateStatus};
use crate::widgets as w;

const STALE_MS: i64 = 2000;

/// The GPU box is omitted entirely on hosts without a usable GPU.
fn has_gpu(st: &State) -> bool {
    st.metrics
        .as_ref()
        .and_then(|m| m.gpus.as_ref())
        .is_some_and(|g| !g.is_empty())
}

/// Disk shown in the disk I/O graphs: the pinned name while it carries
/// data, else the busiest disk by last read+write rate (name tiebreak).
/// None while no disk has data.
fn pick_disk<'a>(io: &'a HashMap<String, Ring<NetRate>>, pinned: Option<&str>) -> Option<&'a str> {
    if let Some(p) = pinned.filter(|p| io.contains_key(*p)) {
        return io.get_key_value(p).map(|(k, _)| k.as_str());
    }
    io.iter()
        .max_by(|a, b| {
            let sa = a.1.last().map(|r| r.rx_bps + r.tx_bps).unwrap_or(0.0);
            let sb = b.1.last().map(|r| r.rx_bps + r.tx_bps).unwrap_or(0.0);
            // max_by keeps the last maximum on ties; reversing the name
            // comparison makes the smallest name win, like the old sort.
            sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal).then(b.0.cmp(a.0))
        })
        .map(|(k, _)| k.as_str())
}

/// Pure selection move for Up/Down (keyboard). `idx` indexes into `rows`.
fn move_selection(
    rows: &[ebbtop_core::wire::Process],
    idx: &[usize],
    selected: Option<u32>,
    up: bool,
) -> Option<u32> {
    if idx.is_empty() {
        return None;
    }
    let pos = selected.and_then(|pid| idx.iter().position(|&ri| rows[ri].pid == pid));
    let next = match (pos, up) {
        (Some(i), true) => i.saturating_sub(1),
        (Some(i), false) => (i + 1).min(idx.len() - 1),
        (None, true) => idx.len() - 1,
        (None, false) => 0,
    };
    Some(rows[idx[next]].pid)
}

/// Phase 2 kill: snapshot of the confirmed target (name for the dialog,
/// pid + start time for the agent request).
#[derive(Debug, Clone)]
struct KillConfirm {
    pid: u32,
    name: String,
    start_time_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Sources,
    Display,
    Connection,
    Updates,
}

/// First-run wizard state. Present while the user has no sources and has
/// not skipped; the Sources tab is the re-run path afterwards.
#[derive(Debug, Default)]
struct Onboard {
    name: String,
    url: String,
    token: String,
    msg: Option<String>,
    rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    testing: bool,
    /// Triple (name, normalized URL, cleaned token) of the last green Test.
    /// `Save and connect` is gated on this matching the current fields.
    tested: Option<(String, String, String)>,
    /// Triple the in-flight probe is testing (set at spawn, checked at poll).
    probe_for: Option<(String, String, String)>,
}

/// Hover + sizing outcome of one linked table. The two bodies scroll
/// independently; only the group frame and the shared hover mirror link them.
/// `viewport` sizes the proc body cap; `hover` drives the mirror.
struct LinkedTable {
    viewport: f32,
    hover: Option<String>,
}

impl LinkedTable {
    const NONE: Self = Self { viewport: 0.0, hover: None };
}

pub struct App {
    cfg: Config,
    shared: Arc<Mutex<State>>,
    rt: Option<tokio::runtime::Runtime>,
    task: Option<tokio::task::JoinHandle<()>>,
    token: Option<String>,
    token_src: TokenSource,
    filter: String,
    /// Workload selected in the roll-up pane, applied on top of `filter`.
    /// None shows every workload. Set by clicking a workload row, cleared by
    /// clicking it again or the chip in the proc header.
    workload_filter: Option<String>,
    /// Workload whose row is hovered in either table. Mirrors the hover
    /// highlight across the workloads/proc pair. Transient, not persisted.
    hover_workload: Option<String>,
    /// Proc panel chrome (title + filter + table header + margins), measured
    /// each frame so the proc body cap leaves room for it and the group does
    /// not overflow in a normal window.
    proc_chrome: f32,
    /// Scroll range of the workloads+proc group frame (0 when it all fits).
    group_scroll_max: f32,
    sort: SortKey,
    desc: bool,
    selected: Option<u32>,
    /// Shared with the client task (`ClientConfig::detail_pid`): the GUI
    /// still owns `selected`; this mirrors it so the 1 s session loop can
    /// poll `get_process_metrics:<pid>` without a lock or a second loop.
    detail_pid: Arc<AtomicU32>,
    settings_open: bool,
    settings_tab: SettingsTab,
    url_edit: String,
    token_edit: String,
    settings_msg: Option<String>,
    source_name_edit: String,
    source_url_edit: String,
    source_msg: Option<String>,
    /// In-flight connection test (Settings Sources tab).
    test_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    testing: bool,
    onboard: Option<Onboard>,
    kill_confirm: Option<KillConfirm>,
    kill_tx: Option<mpsc::UnboundedSender<KillJob>>,
    /// Local note shown in the status bar when no client reply exists
    /// (e.g. no runtime): "kill sent ..." or why the kill was not queued.
    kill_note: Option<String>,
    /// Source index chosen in the status-bar picker, applied after the
    /// frame's shared-state lock is released (see draw). The picker runs
    /// inside draw() while the lock is held; switching inline would
    /// re-lock the non-reentrant mutex on the same thread and wedge the
    /// UI forever. Settings/onboarding call use_active_source after the
    /// lock is dropped, so they are unaffected.
    pending_source: Option<usize>,
    /// Background release check (spec section 1) + update dialog state.
    update: UpdateCheck,
    update_open: bool,
    /// Launch + interval re-checks. True in the running app; false for
    /// harness-built apps so tests (which drive `start()` explicitly)
    /// never spawn the real GitHub fetch from `draw`.
    update_auto: bool,
}

fn local_time(ms: u64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms as i64) {
        chrono::LocalResult::Single(t) => t.format("%H:%M:%S").to_string(),
        _ => "--:--:--".into(),
    }
}

/// Unix seconds as local "2025-11-06 12:00". Zero means unknown.
fn local_datetime(s: u64) -> String {
    if s == 0 {
        return "--".into();
    }
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(s as i64, 0) {
        chrono::LocalResult::Single(t) => t.format("%Y-%m-%d %H:%M").to_string(),
        _ => "--".into(),
    }
}

impl App {
    pub fn new(ctx: egui::Context) -> Self {
        let cfg = store::load_config();
        // Per-source token from the OS credential store (or env override).
        let (token, token_src) = match cfg.active_source() {
            Some(s) => store::load_token_for(&s.name),
            None => (None, TokenSource::Missing),
        };
        // Lazy runtime: fresh installs (no sources) skip the thread spawn
        // at launch so the wizard opens instantly; the runtime is built
        // on demand by `ensure_rt` on the first Test/connect.
        let rt = if cfg.active_source().is_some() {
            Self::build_rt()
        } else {
            None
        };
        let mut app = Self::with(cfg, token, token_src, rt);
        style_with(&ctx, app.cfg.compact);
        app.settings_open = app.token.is_none() && app.onboard.is_none();
        app.start_client(&ctx);
        // Launch release check on its own thread; never blocks startup.
        // Skipped when a persisted check is still fresh (24 h cadence).
        app.update_auto = true;
        if app.update.due() {
            app.update.start(update::fetch_latest_release);
        }
        app.update_title(&ctx);
        app
    }

    /// Construct without a runtime or client (tests).
    pub fn with(cfg: Config, token: Option<String>, token_src: TokenSource, rt: Option<tokio::runtime::Runtime>) -> Self {
        let first_run = cfg.sources.is_empty();
        // Seed before the move below: restores the 24 h cadence and any
        // skipped release across restarts.
        let update =
            UpdateCheck::with_persisted(cfg.update_last_check_ms, cfg.update_skipped.clone(), client::now_ms());
        Self {
            url_edit: cfg.active_source().map(|s| s.url.clone()).unwrap_or_default(),
            cfg,
            shared: Arc::new(Mutex::new(State::default())),
            rt,
            task: None,
            token,
            token_src,
            filter: String::new(),
            workload_filter: None,
            hover_workload: None,
            proc_chrome: 0.0,
            group_scroll_max: 0.0,
            sort: SortKey::Cpu,
            desc: true,
            selected: None,
            detail_pid: Arc::new(AtomicU32::new(client::NO_DETAIL_PID)),
            settings_open: false,
            settings_tab: SettingsTab::Connection,
            token_edit: String::new(),
            settings_msg: None,
            source_name_edit: String::new(),
            source_url_edit: String::new(),
            source_msg: None,
            test_rx: None,
            testing: false,
            onboard: if first_run { Some(Onboard::default()) } else { None },
            kill_confirm: None,
            kill_tx: None,
            kill_note: None,
            pending_source: None,
            update,
            update_open: false,
            update_auto: false,
        }
    }

    #[cfg(test)]
    pub fn shared(&self) -> Arc<Mutex<State>> {
        self.shared.clone()
    }

    fn build_rt() -> Option<tokio::runtime::Runtime> {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("ebbtop-client")
            .build()
            .ok()
    }

    /// Build the client runtime on first use (fresh installs skip it at
    /// launch; the first Test/connect pays it instead).
    fn ensure_rt(&mut self) {
        if self.rt.is_none() {
            self.rt = Self::build_rt();
        }
    }

    fn start_client(&mut self, ctx: &egui::Context) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
        *self.shared.lock().unwrap_or_else(|e| e.into_inner()) = State::default();
        self.kill_tx = None;
        if self.cfg.active_source().is_some() {
            self.ensure_rt();
        }
        let Some(rt) = &self.rt else { return };
        // No sources yet (fresh install): leave the status at CONNECTING
        // until onboarding adds the first source.
        let Some(url) = self.cfg.active_source().map(|s| s.url.clone()) else { return };
        let mut cc = ClientConfig::new(url, self.token.clone());
        // Same atomic the GUI mirrors `selected` into; a reconnect reuses it.
        cc.detail_pid = self.detail_pid.clone();
        let shared = self.shared.clone();
        let ctx = ctx.clone();
        let log_shared = shared.clone();
        // Phase 2 kill: queued on the client's connection, between ticks.
        let (kill_tx, kill_rx) = mpsc::unbounded_channel::<KillJob>();
        self.kill_tx = Some(kill_tx);
        self.task = Some(rt.spawn(async move {
            // Evidence for the overnight check: one stats line a minute.
            let logger = tokio::spawn(async move {
                let mut t = tokio::time::interval(Duration::from_secs(60));
                t.tick().await;
                loop {
                    t.tick().await;
                    let line = log_shared.lock().map(|s| s.summary_line(client::now_ms())).unwrap_or_default();
                    store::append_log(&line);
                }
            });
            struct AbortOnDrop(tokio::task::JoinHandle<()>);
            impl Drop for AbortOnDrop {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _guard = AbortOnDrop(logger);
            store::append_log(&format!("start url={}", cc.url));
            client::run_with_kill(cc, Some(kill_rx), move |ev| {
                if let client::Event::Disconnected { reason, retry_in } = &ev {
                    store::append_log(&format!("disconnected reason={reason} retry_ms={}", retry_in.as_millis()));
                }
                if let client::Event::KillResult { r, .. } = &ev {
                    store::append_log(&format!("kill pid={} signal={} ok={}", r.pid, r.signal, r.ok));
                }
                if let Ok(mut s) = shared.lock() {
                    s.apply(ev, client::now_ms());
                }
                ctx.request_repaint();
            })
            .await
        }));
    }

    pub fn draw(&mut self, ui: &mut Ui) {
        // Release check bookkeeping: drain a finished check (logging the
        // outcome), re-check at the interval, and pop the dialog for a
        // version the user has not seen yet. The fetch runs on its own
        // thread; none of this blocks the frame.
        if self.update.poll() {
            match self.update.status() {
                UpdateStatus::Available { version, .. } => store::append_log(&format!(
                    "update available {version} (current {})", update::current_version())),
                UpdateStatus::UpToDate { version } => {
                    store::append_log(&format!("update check up to date {version}"));
                }
                UpdateStatus::Failed(e) => store::append_log(&format!("update check failed: {e}")),
                _ => {}
            }
            // Persist the check time so the 24 h cadence survives restarts.
            self.cfg.update_last_check_ms = client::now_ms();
            let _ = store::save_config(&self.cfg);
        }
        if self.update_auto && self.update.due() {
            self.update.start(update::fetch_latest_release);
        }
        if self.update.should_prompt() {
            self.update_open = true;
            self.update.mark_notified();
        }
        let now = client::now_ms();
        let shared = self.shared.clone();
        let st = shared.lock().unwrap_or_else(|e| e.into_inner());
        ui.ctx().request_repaint_after(Duration::from_millis(250));

        egui::Panel::top("status")
            .frame(egui::Frame::new().fill(w::BOX).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(ui, |ui| self.status_bar(ui, &st, now));

        egui::CentralPanel::default_margins()
            .frame(egui::Frame::new().fill(w::BG).inner_margin(8.0))
            .show(ui, |ui| {
                let slots = self.cfg.window_s as usize;
                let wide = ui.available_width() >= 1100.0;
                let total_h = ui.available_height();
                let any_top =
                    self.cfg.show_cpu || self.cfg.show_gpu || self.cfg.show_mem || self.cfg.show_net || self.cfg.show_disk;
                if any_top {
                    let frac = self.cfg.top_frac.clamp(0.25, 0.75);
                    // Dynamic handoff: if the process table is collapsed,
                    // give the freed height to the top boxes instead of
                    // leaving it blank at the bottom. One bool branch.
                    let top_max = if self.cfg.collapsed_proc {
                        total_h.max(120.0)
                    } else if total_h > 0.0 {
                        (total_h * frac).max(120.0)
                    } else {
                        120.0
                    };
                    egui::ScrollArea::vertical()
                        .id_salt("top")
                        .max_height(top_max)
                        .auto_shrink([false, true])
                        .show(ui, |ui| self.top_boxes(ui, &st, slots, wide));
                    // Drag handle (phase 2): adjust the saved ratio.
                    // One 8px allocate + paint per frame — no per-frame alloc.
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.0), egui::Sense::drag());
                    let hover_like = resp.hovered() || resp.dragged();
                    let line_col = if hover_like { w::ACCENT } else { w::LINE };
                    let cy = rect.center().y;
                    ui.painter_at(rect).line_segment(
                        [
                            egui::pos2(rect.left() + 4.0, cy),
                            egui::pos2(rect.right() - 4.0, cy),
                        ],
                        egui::Stroke::new(if hover_like { 2.0 } else { 1.0 }, line_col),
                    );
                    let resp = resp.on_hover_cursor(egui::CursorIcon::ResizeVertical);
                    if resp.dragged() {
                        let d = resp.drag_delta().y;
                        if d.abs() > 0.0 && total_h > 0.0 {
                            let nf = (self.cfg.top_frac + d / total_h).clamp(0.25, 0.75);
                            self.cfg.top_frac = nf;
                            let _ = store::save_config(&self.cfg);
                        }
                    }
                }
                // Detail pane sits between the top boxes and the process
                // table; it is hidden unless a row is selected.
                self.proc_detail_box(ui, &st, slots);
                // Workloads + proc scroll as a group (like the top resource
                // boxes) while each keeps its own inner scroll. Capture the
                // viewport height before the group so the short-window rule
                // and the proc cap are sized from what is actually visible.
                let group_h = ui.available_height();
                let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
                let group_out = egui::ScrollArea::vertical()
                    .id_salt("tables")
                    .max_height(group_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let y0 = ui.cursor().min.y;
                        let wl = self.workloads_box(ui, &st, group_h);
                        let wl_h = ui.cursor().min.y - y0;
                        // Proc fills what the roll-up leaves (minus its own
                        // chrome), with a floor so a short window scrolls the
                        // group instead of squeezing.
                        let proc_cap = (group_h - wl_h - self.proc_chrome).max(row_h * 8.0);
                        let y1 = ui.cursor().min.y;
                        let proc = self.proc_box(ui, &st, proc_cap);
                        self.proc_chrome = (ui.cursor().min.y - y1) - proc.viewport;
                        (wl, proc)
                    });
                let (wl, proc) = group_out.inner;
                self.group_scroll_max =
                    (group_out.content_size.y - group_out.inner_rect.height()).max(0.0);
                // Hover mirror: whichever table saw a hover owns the key this
                // frame; both tables highlight it on the next one.
                self.hover_workload = wl.hover.or(proc.hover);
                // The two bodies scroll independently: egui keeps each one's
                // offset by id_salt, so no cross-table sync is applied here.
            });
        drop(st);
        // Apply a status-bar picker choice now that the lock is released.
        if let Some(i) = self.pending_source.take() {
            self.cfg.active = i;
            let _ = store::save_config(&self.cfg);
            self.use_active_source(ui.ctx());
            store::append_log(&format!("picker switched active={i}"));
        }
        // Mirror the final selection for the client task's 1 s poll. The
        // proc_box update above is what changed it (click / Up / Down).
        // Store only on change: an atomic is cheap, but this runs every
        // 250 ms repaint. A selection change also retires a stale kill
        // notice: the reply belonged to the previously selected pid.
        let want = self.selected.unwrap_or(client::NO_DETAIL_PID);
        if self.detail_pid.load(Ordering::Relaxed) != want {
            self.detail_pid.store(want, Ordering::Relaxed);
            if let Ok(mut st) = self.shared.lock() {
                st.clear_kill();
            }
        }

        if self.settings_open {
            self.settings(ui.ctx());
        }
        if self.update_open {
            self.update_dialog(ui.ctx());
        }
        if self.onboard.is_some() {
            self.onboarding(ui.ctx());
        }
        self.kill_dialog(ui.ctx());
    }

    /// Phase 2 kill confirm dialog. Names the process and pid; the signal
    /// is chosen here (TERM default path, INT alternative). Sends exactly
    /// one job on the client's kill channel.
    fn kill_dialog(&mut self, ctx: &egui::Context) {
        let Some(kc) = self.kill_confirm.clone() else { return };
        let mut open = true;
        let mut sig: Option<KillSignal> = None;
        let mut cancel = false;
        let host = self.cfg.active_source().map(|s| s.name.clone()).unwrap_or_else(|| "The server".to_string());
        egui::Window::new("Kill process")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add(egui::Label::new(RichText::new(format!(
                    r#"Send SIGTERM to "{}" (pid {})? {} will terminate this process. This cannot be undone."#,
                    kc.name, kc.pid, host
                )))
                .wrap());
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Send TERM").clicked() {
                        sig = Some(KillSignal::Term);
                    }
                    if ui.button("Send INT").clicked() {
                        sig = Some(KillSignal::Int);
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if let Some(s) = sig {
            self.queue_kill(&kc, s);
        } else if !open || cancel {
            self.kill_confirm = None;
        }
    }

    fn queue_kill(&mut self, kc: &KillConfirm, signal: KillSignal) {
        // A new attempt supersedes any previous reply (success or refusal).
        if let Ok(mut st) = self.shared.lock() {
            st.clear_kill();
        }
        if let Some(tx) = &self.kill_tx {
            let _ = tx.send(KillJob { pid: kc.pid, start_time_ms: kc.start_time_ms, signal });
            self.kill_note = Some(format!("kill sent: pid {} {}", kc.pid, signal.as_str()));
        } else {
            self.kill_note = Some("kill unavailable: no client running".to_string());
        }
        self.kill_confirm = None;
    }

    fn top_boxes(&mut self, ui: &mut Ui, st: &State, slots: usize, wide: bool) {
        if wide {
            ui.columns(2, |c| {
                let show_cpu = self.cfg.show_cpu;
                let show_gpu = self.cfg.show_gpu && has_gpu(st);
                if show_cpu {
                    self.cpu_box(&mut c[0], st, slots, 170.0);
                }
                if show_gpu {
                    if show_cpu {
                        c[0].add_space(6.0);
                    }
                    self.gpu_box(&mut c[0], st, slots);
                }
                let mut first_right = true;
                if self.cfg.show_mem {
                    self.mem_box(&mut c[1], st, slots);
                    first_right = false;
                }
                if self.cfg.show_net {
                    if !first_right {
                        c[1].add_space(6.0);
                    }
                    self.net_box(&mut c[1], st, slots);
                    first_right = false;
                }
                if self.cfg.show_disk {
                    if !first_right {
                        c[1].add_space(6.0);
                    }
                    self.disk_box(&mut c[1], st, slots);
                }
            });
        } else {
            let mut first = true;
            if self.cfg.show_cpu {
                // 50 not 70: frees room for the new swap/graph rows so
                // disk io stays in view at half-screen (see render test).
                self.cpu_box(ui, st, slots, 50.0);
                first = false;
            }
            if self.cfg.show_gpu && has_gpu(st) {
                if !first {
                    ui.add_space(6.0);
                }
                self.gpu_box(ui, st, slots);
                first = false;
            }
            // Narrow second row: mem/disk left, net right (existing fit).
            let left_any = self.cfg.show_mem || self.cfg.show_disk;
            if left_any || self.cfg.show_net {
                if !first {
                    ui.add_space(6.0);
                }
                ui.columns(2, |c| {
                    let mut first_left = true;
                    if self.cfg.show_mem {
                        self.mem_box(&mut c[0], st, slots);
                        first_left = false;
                    }
                    if self.cfg.show_disk {
                        if !first_left {
                            c[0].add_space(6.0);
                        }
                        self.disk_box(&mut c[0], st, slots);
                    }
                    if self.cfg.show_net {
                        self.net_box(&mut c[1], st, slots);
                    }
                });
            }
        }
    }

    fn status_bar(&mut self, ui: &mut Ui, st: &State, now: u64) {
        ui.horizontal(|ui| {
            // Half-screen (640px) has no room for every status chip.
            // Age + hover summary stay; sample/tick move to the tooltip.
            let narrow = ui.available_width() < 720.0;
            let age = st.since_new_sample_ms(now);
            let (label, color) = match (&st.conn, age) {
                (Conn::Connected, Some(a)) if a < STALE_MS => ("LIVE", w::OK),
                (Conn::Connected, Some(_)) => ("STALE", w::WARN),
                (Conn::Connected, None) | (Conn::Connecting { .. }, _) => ("CONNECTING", w::WARN),
                (Conn::Down { .. }, _) => ("DOWN", w::BAD),
            };
            // Dot + text (phase 1): no background fill, so no contrast hack.
            // Dot is painted as a circle — no glyph dependency.
            let dot = (egui::vec2(8.0, 8.0), egui::Sense::hover());
            let (rect, _) = ui.allocate_exact_size(dot.0, dot.1);
            ui.painter_at(rect).circle_filled(rect.center(), 3.5, color);
            ui.label(RichText::new(format!(" {label} ")).monospace().strong().color(color));
            // Metrics hostname when live, else the configured source name,
            // else the empty state. Never a hardcoded host.
            let host = st.metrics.as_ref().map(|m| m.hostname.clone()).or_else(|| {
                self.cfg.active_source().map(|s| s.name.clone())
            });
            let host = host.as_deref().unwrap_or("no source");
            ui.label(RichText::new(host).monospace().strong().color(w::TEXT));
            if let Some(a) = age {
                let c = if a < STALE_MS { w::DIM } else { w::WARN };
                let lag = st.sample_age_ms(now).map(|l| format!(" (lag {l} ms)")).unwrap_or_default();
                // Diagnostics live here, not in the bar: sample time, tick
                // p95 and reconnects join the summary line in the tooltip.
                let diag = match (&st.metrics, st.stats.interval_p(95.0)) {
                    (Some(m), Some(p95)) => format!(
                        "\nsample {} · tick p95 {}{}",
                        local_time(m.sampled_at_ms),
                        view::age(p95),
                        if st.reconnects > 0 { format!(" · reconnects {}", st.reconnects) } else { String::new() }
                    ),
                    (Some(m), None) => format!("\nsample {}", local_time(m.sampled_at_ms)),
                    _ => String::new(),
                };
                ui.label(RichText::new(format!("age {}", view::age(a))).monospace().color(c))
                    .on_hover_text(format!("time since a new sample arrived, local clock{lag}{diag}\n{}", st.summary_line(now)));
            }
            match &st.conn {
                Conn::Down { reason, retry_in, .. } => {
                    ui.label(RichText::new(format!("{reason}; retry {}", view::age(retry_in.as_millis() as i64))).monospace().color(w::BAD));
                }
                Conn::Connecting { attempt } if *attempt > 1 => {
                    ui.label(RichText::new(format!("attempt {attempt}")).monospace().color(w::WARN));
                }
                _ => {}
            }
            // Phase 2 kill outcome (agent reply wins over the local note).
            // Successes expire via kill_visible; refusals persist until the
            // next attempt or a selection change. An agent reply retires the
            // local "kill sent" note. Narrow bars hide transient success and
            // notes; a refusal is safety-critical and always stays visible.
            let show_kill =
                st.kill_visible(now) && (!narrow || st.kill.as_ref().is_some_and(|r| !r.ok));
            if show_kill {
                self.kill_note = None;
            }
            if let Some(r) = st.kill.as_ref().filter(|_| show_kill) {
                let (txt, col) = if r.ok {
                    (format!("kill {} {} sent", r.pid, r.signal), w::OK)
                } else {
                    (format!("kill {} refused: {}", r.pid, r.error.as_deref().unwrap_or("error")), w::BAD)
                };
                ui.label(RichText::new(txt).monospace().color(col));
            } else if !narrow {
                if let Some(note) = &self.kill_note {
                    ui.label(RichText::new(note).monospace().color(w::DIM));
                }
            }
            // Release check outcome: an available release gets a prompt
            // button; a failed check is a quiet note (spec: retry next
            // interval, app runs as before).
            match self.update.status() {
                UpdateStatus::Available { version, .. } => {
                    let version = version.clone();
                    ui.label(RichText::new(format!("update {version} ready")).monospace().color(w::WARN));
                    if ui.button("Update").clicked() {
                        self.update_open = true;
                        self.update.mark_notified();
                    }
                }
                UpdateStatus::Failed(_) if !narrow => {
                    ui.label(RichText::new("update check failed").monospace().color(w::DIM));
                }
                _ => {}
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    self.settings_open = true;
                    self.url_edit = self.cfg.active_source().map(|s| s.url.clone()).unwrap_or_default();
                    self.settings_msg = None;
                }
                // Source picker, only when there is a choice to make.
                // Sits with the other controls, left of Settings.
                if self.cfg.sources.len() > 1 {
                    let mut pick = self.cfg.active.min(self.cfg.sources.len() - 1);
                    let names: Vec<String> = self.cfg.sources.iter().map(|s| s.name.clone()).collect();
                    egui::ComboBox::new("source", "source").selected_text(&names[pick]).show_ui(ui, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            ui.selectable_value(&mut pick, i, n);
                        }
                    });
                    if pick != self.cfg.active {
                        // Deferred: draw() holds the shared-state lock
                        // across this whole frame, so switching inline
                        // would deadlock. Applied below after drop(st).
                        store::append_log(&format!("picker pick={pick} prev={}", self.cfg.active));
                        self.pending_source = Some(pick);
                    }
                }
                if ui.selectable_label(self.cfg.compact, "Dense").clicked() {
                    self.cfg.compact = !self.cfg.compact;
                    let _ = store::save_config(&self.cfg);
                    style_with(ui.ctx(), self.cfg.compact);
                }
                for (s, l) in [(300u32, "5m"), (60, "60s")] {
                    if ui.selectable_label(self.cfg.window_s == s, l).clicked() && self.cfg.window_s != s {
                        self.cfg.window_s = s;
                        let _ = store::save_config(&self.cfg);
                    }
                }
            });
        });
    }

    fn cpu_box(&mut self, ui: &mut Ui, st: &State, slots: usize, graph_h: f32) {
        let mut open = !self.cfg.collapsed_cpu;
        // The header carries the machine's model so it stays readable while
        // the box is collapsed. None (legacy agent) => label alone.
        let model = st.metrics.as_ref().and_then(|m| m.cpu_model.as_deref());
        w::panel_with_model(ui, "cpu", model, &mut open, |ui| {
            let Some(m) = &st.metrics else {
                ui.label(RichText::new("waiting for data").color(w::DIM));
                return;
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{:5.1}%", m.cpu_total)).monospace().strong().color(w::heat(m.cpu_total / 100.0)));
                ui.label(RichText::new(format!("{} cores", m.cpu_per_core.len())).monospace().color(w::DIM));
                if let Some(t) = m.cpu_temp_c {
                    ui.label(RichText::new(format!("{t:.0}°C")).monospace().color(w::DIM));
                }
                if let Some([a, b, c]) = m.load_avg {
                    ui.label(RichText::new(format!("load {a:.2} {b:.2} {c:.2}")).monospace().color(w::DIM));
                }
                if let Some(u) = m.uptime_s {
                    ui.label(RichText::new(format!("up {}", view::uptime(u))).monospace().color(w::DIM));
                }
            });
            let vals: Vec<f64> = st.cpu.iter().map(|p| p.1).collect();
            w::graph(ui, &vals, slots, 100.0, graph_h, Some(w::CPU));
            ui.add_space(4.0);
            let cols = ((ui.available_width() / 150.0).floor() as usize).clamp(1, 8);
            for (row_i, chunk) in m.cpu_per_core.chunks(cols).enumerate() {
                ui.columns(cols, |c| {
                    for (j, v) in chunk.iter().enumerate() {
                        let i = row_i * cols + j;
                        w::meter(&mut c[j], &format!("C{i}"), v / 100.0, &format!("{v:3.0}%"), Some(w::CPU));
                    }
                });
            }
        });
        let collapsed = !open;
        if collapsed != self.cfg.collapsed_cpu {
            self.cfg.collapsed_cpu = collapsed;
            let _ = store::save_config(&self.cfg);
        }
    }

    fn gpu_box(&mut self, ui: &mut Ui, st: &State, slots: usize) {
        let Some(gpus) = st
            .metrics
            .as_ref()
            .and_then(|m| m.gpus.as_ref())
            .filter(|g| !g.is_empty())
        else {
            return;
        };
        let mut open = !self.cfg.collapsed_gpu;
        // Header carries the first GPU's model (short form, same as the body
        // rows) so it stays readable while the box is collapsed.
        let model = gpus.first().map(|g| w::short_gpu_name(&g.name));
        w::panel_with_model(ui, "gpu", model, &mut open, |ui| {
            for (i, g) in gpus.iter().enumerate() {
                let short = w::short_gpu_name(&g.name);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("G{i} {short}")).monospace().strong().color(w::TEXT))
                        .on_hover_text(&g.name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(t) = g.temperature_c {
                            ui.label(RichText::new(format!("{t:.0}°C")).monospace()
                                .color(w::heat((t / 100.0).clamp(0.0, 1.0))));
                        }
                    });
                });
                w::meter(ui, "Load", g.utilization_gpu_pct as f32 / 100.0,
                    &format!("{}%", g.utilization_gpu_pct), Some(w::GPU));
                let vram = if g.mem_total_bytes == 0 {
                    0.0
                } else {
                    g.mem_used_bytes as f32 / g.mem_total_bytes as f32
                };
                w::meter(ui, "VRAM", vram,
                    &format!("{} / {}", view::bytes(g.mem_used_bytes), view::bytes(g.mem_total_bytes)), Some(w::GPU));
                if let Some(ring) = st.gpu_pct.get(&g.name) {
                    let vals: Vec<f64> = ring.iter().map(|p| p.1).collect();
                    w::graph(ui, &vals, slots, 100.0, 24.0, Some(w::GPU));
                }
                // VRAM history (ring added in history.rs, same clock).
                // No label line: 24+18 keeps the box compact for half-screen.
                if let Some(ring) = st.gpu_mem_pct.get(&g.name) {
                    let vals: Vec<f64> = ring.iter().map(|p| p.1).collect();
                    w::graph(ui, &vals, slots, 100.0, 18.0, Some(w::GPU));
                }
                if i + 1 < gpus.len() {
                    ui.add_space(4.0);
                }
            }
        });
        let collapsed = !open;
        if collapsed != self.cfg.collapsed_gpu {
            self.cfg.collapsed_gpu = collapsed;
            let _ = store::save_config(&self.cfg);
        }
    }

    fn mem_box(&mut self, ui: &mut Ui, st: &State, slots: usize) {
        let mut open = !self.cfg.collapsed_mem;
        w::panel(ui, "mem", &mut open, |ui| {
            let Some(m) = &st.metrics else { return };
            let f = |u: u64, t: u64| if t == 0 { 0.0 } else { u as f32 / t as f32 };
            w::meter(ui, "Used", f(m.mem_used, m.mem_total),
                &format!("{} / {}", view::bytes(m.mem_used), view::bytes(m.mem_total)), Some(w::MEM));
            w::meter(ui, "Swap", f(m.swap_used, m.swap_total),
                &format!("{} / {}", view::bytes(m.swap_used), view::bytes(m.swap_total)), Some(w::MEM));
            let vals: Vec<f64> = st.mem_pct.iter().map(|p| p.1).collect();
            w::graph(ui, &vals, slots, 100.0, 24.0, Some(w::MEM));
            // Swap history was already tracked in State, just not drawn.
            // Only drawn when swap exists, to save vertical room.
            if m.swap_total > 0 {
                let svals: Vec<f64> = st.swap_pct.iter().map(|p| p.1).collect();
                // Amber (DISK) reads as swap/disk-backed, distinct from RAM green.
                w::graph(ui, &svals, slots, 100.0, 18.0, Some(w::DISK));
            }
        });
        let collapsed = !open;
        if collapsed != self.cfg.collapsed_mem {
            self.cfg.collapsed_mem = collapsed;
            let _ = store::save_config(&self.cfg);
        }
    }

    fn net_box(&mut self, ui: &mut Ui, st: &State, slots: usize) {
        let mut open = !self.cfg.collapsed_net;
        w::panel(ui, "net", &mut open, |ui| {
            let ifaces = st.interfaces();
            if ifaces.is_empty() {
                return;
            }
            let cur = self.cfg.iface.clone().filter(|i| ifaces.contains(i)).unwrap_or_else(|| ifaces[0].clone());
            let mut pick = cur.clone();
            egui::ComboBox::new("iface", "interface").selected_text(&pick).show_ui(ui, |ui| {
                for i in &ifaces {
                    ui.selectable_value(&mut pick, i.clone(), i);
                }
            });
            if pick != cur {
                self.cfg.iface = Some(pick.clone());
                let _ = store::save_config(&self.cfg);
            }
            if ui.checkbox(&mut self.cfg.log_scale, "log scale").changed() {
                let _ = store::save_config(&self.cfg);
            }
            let Some(ring) = st.net.get(&pick) else {
                ui.label(RichText::new("measuring").color(w::DIM));
                return;
            };
            let take = slots.min(ring.len());
            let recent: Vec<_> = ring.iter().skip(ring.len() - take).copied().collect();
            for (arrow, color, sel) in [("rx", w::RX, true), ("tx", w::TX, false)] {
                let linear: Vec<f64> = recent.iter().map(|r| if sel { r.rx_bps } else { r.tx_bps }).collect();
                let peak_lin = linear.iter().copied().fold(0.0, f64::max).max(1024.0);
                let now = linear.last().copied().unwrap_or(0.0);
                // Log transform only when toggled; labels stay linear.
                let (vals, peak) = if self.cfg.log_scale {
                    let vals: Vec<f64> = linear.iter().map(|v| v.max(1.0).log10()).collect();
                    let peak = vals.iter().copied().fold(0.0, f64::max).max(3.0);
                    (vals, peak)
                } else {
                    (linear, peak_lin)
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{arrow} {:>11}", view::rate_bits(now))).monospace().color(color));
                    ui.label(RichText::new(format!("peak {}", view::rate_bits(peak_lin))).monospace().color(w::DIM));
                });
                w::graph(ui, &vals, slots, peak, 30.0, Some(color));
            }
        });
        let collapsed = !open;
        if collapsed != self.cfg.collapsed_net {
            self.cfg.collapsed_net = collapsed;
            let _ = store::save_config(&self.cfg);
        }
    }

    fn disk_box(&mut self, ui: &mut Ui, st: &State, slots: usize) {
        let mut open = !self.cfg.collapsed_disk;
        w::panel(ui, "disk", &mut open, |ui| {
            // Fullest first: the disks most likely to need attention stay
            // visible. Capped at four meters; the busiest-disk graphs below
            // carry the live rates, so per-disk text lines are gone.
            let mut disks: Vec<_> = st.disks.iter().filter(|d| d.total > 0).collect();
            disks.sort_by(|a, b| {
                let fa = a.total.saturating_sub(a.available) as f64 / a.total as f64;
                let fb = b.total.saturating_sub(b.available) as f64 / b.total as f64;
                fb.partial_cmp(&fa).unwrap_or(std::cmp::Ordering::Equal).then(a.name.cmp(&b.name))
            });
            if disks.is_empty() {
                ui.label(RichText::new("waiting for data").color(w::DIM));
            }
            for d in disks.iter().take(4) {
                let used = d.total.saturating_sub(d.available);
                let name: String = d.name.trim_start_matches("/dev/").chars().take(8).collect();
                w::meter(ui, &name, used as f32 / d.total as f32,
                    &format!("{} / {}", view::bytes(used), view::bytes(d.total)), Some(w::DISK));
            }
            if disks.len() > 4 {
                ui.label(RichText::new(format!("+{} more", disks.len() - 4)).monospace().color(w::DIM));
            }
            // r/w history graphs for the picked disk (pin or busiest),
            // same pattern + log toggle as net. Data rings already
            // existed in State.
            if let Some(picked) = pick_disk(&st.disk_io, self.cfg.disk.as_deref())
                && let Some(ring) = st.disk_io.get(picked)
            {
                let name = picked;
                let take = slots.min(ring.len());
                if take > 1 {
                    // Picker state, resolved once for both graph rows. The
                    // ComboBox itself lives in the read row below: a picker
                    // row of its own would push these graphs under the
                    // scroll fold at 1000 px (see renders test).
                    let mut names: Vec<String> = st.disk_io.keys().cloned().collect();
                    names.sort();
                    let current = self.cfg.disk.clone().filter(|d| st.disk_io.contains_key(d));
                    let mut pick = current.clone();
                    let selected = current.clone().unwrap_or_else(|| "auto".to_string());
                    // No choice, no picker: mirrors the host picker rule.
                    let show_pick = names.len() > 1;
                    let recent: Vec<_> = ring.iter().skip(ring.len() - take).copied().collect();
                    for (arrow, color, sel) in [("r", w::RX, true), ("w", w::TX, false)] {
                        let linear: Vec<f64> =
                            recent.iter().map(|r| if sel { r.rx_bps } else { r.tx_bps }).collect();
                        let peak_lin = linear.iter().copied().fold(0.0, f64::max).max(1024.0);
                        let now = linear.last().copied().unwrap_or(0.0);
                        let (vals, peak) = if self.cfg.log_scale {
                            let vals: Vec<f64> = linear.iter().map(|v| v.max(1.0).log10()).collect();
                            let peak = vals.iter().copied().fold(0.0, f64::max).max(3.0);
                            (vals, peak)
                        } else {
                            (linear, peak_lin)
                        };
                        ui.horizontal(|ui| {
                            if show_pick && arrow == "r" {
                                egui::ComboBox::new("disk", "disk").selected_text(&selected).show_ui(ui, |ui| {
                                    ui.selectable_value(&mut pick, None, "auto");
                                    for name in &names {
                                        ui.selectable_value(&mut pick, Some(name.clone()), name);
                                    }
                                });
                                if pick != current {
                                    self.cfg.disk = pick.clone();
                                    let _ = store::save_config(&self.cfg);
                                }
                            }
                            ui.label(RichText::new(format!("{name} {arrow} {:>10}", view::bytes(now as u64)))
                                .monospace()
                                .color(color));
                            ui.label(RichText::new(format!("peak {}", view::bytes(peak_lin as u64)))
                                .monospace()
                                .color(w::DIM));
                        });
                        w::graph(ui, &vals, slots, peak, 18.0, Some(color));
                    }
                }
            }
        });
        let collapsed = !open;
        if collapsed != self.cfg.collapsed_disk {
            self.cfg.collapsed_disk = collapsed;
            let _ = store::save_config(&self.cfg);
        }
    }

    /// Compact per-process detail. Hidden unless a row is selected; never
    /// blocks the 1 s loop (the client task fills `st.proc_detail` at 1 Hz
    /// while a pid is selected). While the first reply is still in flight
    /// it shows "waiting for data" like the other boxes; "no detail" means
    /// a reply arrived for a different pid (stale after a fast reselect).
    /// The Kill action lives in this header so the table below never
    /// reflows when the selection changes.
    fn proc_detail_box(&mut self, ui: &mut Ui, st: &State, slots: usize) {
        let Some(pid) = self.selected else { return };
        let mut open = !self.cfg.collapsed_proc_detail;
        w::panel(ui, "proc-detail", &mut open, |ui| {
            let row = st.procs.as_ref().and_then(|pp| pp.rows.iter().find(|r| r.pid == pid));
            let detail = st.proc_detail.as_ref().filter(|d| d.process.pid == pid);
            let mem_total = st.metrics.as_ref().map(|m| m.mem_total).unwrap_or(0).max(1);
            // Identity prefers the detail reply, falls back to the list row
            // (which knows the /etc/passwd name too) so the header and the
            // Kill action render while the first reply is still in flight.
            // The CPU/MEM readout needs the reply and waits for it.
            let (name, user, state_str) = match (&detail, &row) {
                (Some(d), _) => {
                    let p = &d.process;
                    let name = if !p.name.is_empty() {
                        p.name.clone()
                    } else {
                        row.as_ref().map(|r| r.name.clone()).unwrap_or_default()
                    };
                    let user = match row.as_ref() {
                        Some(r) if !r.user.is_empty() => r.user.clone(),
                        _ if p.user_id != 0 => p.user_id.to_string(),
                        _ => String::new(),
                    };
                    let state_str = if !p.status.is_empty() {
                        p.status.clone()
                    } else {
                        row.as_ref().map(|r| r.state.clone()).unwrap_or_default()
                    };
                    (name, user, state_str)
                }
                (None, Some(r)) => (r.name.clone(), r.user.clone(), r.state.clone()),
                (None, None) => (String::new(), String::new(), String::new()),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{pid} {name} {user} {state_str}")).monospace().strong().color(w::TEXT));
                if let Some(d) = detail.as_ref() {
                    let p = &d.process;
                    let memf = p.mem_bytes as f32 * 100.0 / mem_total as f32;
                    ui.label(RichText::new(format!("{:.1}% {:.1}% {}", p.cpu_usage, memf, view::bytes(p.mem_bytes)))
                        .monospace()
                        .color(w::heat((p.cpu_usage / 100.0).clamp(0.0, 1.0))));
                }
                // Kill, on demand only: enabled for a non-root row that
                // carries a start time. Clicking opens the confirm dialog;
                // the signal always goes via the dialog.
                if let Some(r) = row.as_ref() {
                    let root = view::is_root_owned(&r.user);
                    // 0 = agent did not report a start time (unpatched): no
                    // PID-reuse guard, so no kill.
                    let no_start = r.start_time_ms == 0;
                    let host = self.cfg.active_source().map(|s| s.name.clone()).unwrap_or_else(|| "this host".into());
                    let hint = if root {
                        "root-owned processes cannot be killed".to_string()
                    } else if no_start {
                        "agent does not report process start times".to_string()
                    } else {
                        format!("terminate this process on {host}")
                    };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!root && !no_start, egui::Button::new("Kill"))
                            .on_hover_text(format!("{hint} (pid {})", r.pid))
                            .clicked()
                        {
                            self.kill_confirm = Some(KillConfirm {
                                pid: r.pid,
                                name: r.name.clone(),
                                start_time_ms: r.start_time_ms,
                            });
                        }
                    });
                }
            });
            // Body: CPU history once the reply backs this pid, else the
            // waiting/stale copy shared with the other boxes.
            match detail.as_ref() {
                Some(d) => {
                    let p = &d.process;
                    // Command (truncated display) plus Copy for the full text.
                    if !p.command.is_empty() && p.command != name {
                        ui.horizontal(|ui| {
                            let cmd: String = p.command.chars().take(120).collect();
                            let cmd = if p.command.chars().count() > 120 { format!("{cmd}…") } else { cmd };
                            ui.label(RichText::new(cmd).monospace().color(w::DIM));
                            if ui.button("Copy").on_hover_text("copy full command").clicked() {
                                ui.ctx().copy_text(p.command.clone());
                            }
                        });
                    }
                    // Facts row 1: counts + CPU time. The threads/fds/ppid
                    // order is asserted by the exe test.
                    let mut facts = vec![format!("threads {}", p.thread_count)];
                    if let Some(fds) = p.fd_count {
                        facts.push(format!("fds {fds}"));
                    }
                    if let Some(ppid) = p.parent_pid {
                        facts.push(format!("ppid {ppid}"));
                    }
                    facts.push(format!("cpu {}", view::cpu_time(p.cpu_time_user.saturating_add(p.cpu_time_system))));
                    ui.label(RichText::new(facts.join(" · ")).monospace().color(w::DIM));
                    // Facts row 2: memory + I/O. r/w stay adjacent (exe test).
                    let mut mem = vec![format!("vmem {}", view::bytes(p.virtual_mem_bytes))];
                    if let Some(s) = p.shared_mem_bytes {
                        mem.push(format!("shared {}", view::bytes(s)));
                    }
                    match (p.read_bytes, p.write_bytes) {
                        (Some(r), Some(wr)) => mem.push(format!("r {} · w {}", view::bytes(r), view::bytes(wr))),
                        (Some(r), None) => mem.push(format!("read {}", view::bytes(r))),
                        (None, Some(wr)) => mem.push(format!("written {}", view::bytes(wr))),
                        (None, None) => {}
                    }
                    ui.label(RichText::new(mem.join(" · ")).monospace().color(w::DIM));
                    // Facts row 3: origin.
                    let mut origin = vec![format!("started {}", local_datetime(p.start_time))];
                    if let Some(cwd) = p.working_directory.as_ref().filter(|c| !c.is_empty()) {
                        origin.push(format!("cwd {cwd}"));
                    }
                    ui.label(RichText::new(origin.join(" · ")).monospace().color(w::DIM));
                    if let Some(exe) = p.executable_path.as_ref().filter(|e| !e.is_empty()) {
                        ui.label(RichText::new(exe).monospace().color(w::DIM));
                    }
                    // Children on one row; click jumps the selection there.
                    if !p.child_processes.is_empty() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("children:").monospace().color(w::DIM));
                            for c in p.child_processes.iter().take(4) {
                                if ui.selectable_label(false, format!("{} {}", c.pid, c.name)).clicked() {
                                    self.selected = Some(c.pid);
                                }
                            }
                            if p.child_processes.len() > 4 {
                                ui.label(RichText::new(format!("+{} more", p.child_processes.len() - 4)).monospace().color(w::DIM));
                            }
                        });
                    }
                    // Top threads by CPU time, capped.
                    if !p.threads.is_empty() {
                        let mut th: Vec<_> = p.threads.iter().collect();
                        th.sort_by(|a, b| {
                            let ta = a.cpu_time_user.saturating_add(a.cpu_time_system);
                            let tb = b.cpu_time_user.saturating_add(b.cpu_time_system);
                            tb.cmp(&ta).then(a.tid.cmp(&b.tid))
                        });
                        ui.label(RichText::new("TID Name S CPU").monospace().color(w::DIM));
                        for t in th.iter().take(5) {
                            let cpu = view::cpu_time(t.cpu_time_user.saturating_add(t.cpu_time_system));
                            let row = if t.name.is_empty() {
                                format!("{} {} {cpu}", t.tid, view::state_letter(&t.status))
                            } else {
                                format!("{} {} {} {cpu}", t.tid, t.name, view::state_letter(&t.status))
                            };
                            ui.label(RichText::new(row).monospace().color(w::DIM));
                        }
                        if th.len() > 5 {
                            ui.label(RichText::new(format!("+{} more", th.len() - 5)).monospace().color(w::DIM));
                        }
                    }
                    // CPU + RSS graphs side by side: no added height.
                    let cpu_vals: Vec<f64> = st.proc_detail_cpu.iter().map(|pt| pt.1).collect();
                    if cpu_vals.is_empty() {
                        ui.label(RichText::new("no detail").monospace().color(w::DIM));
                    } else {
                        let mem_vals: Vec<f64> = st.proc_detail_mem.iter().map(|pt| pt.1).collect();
                        let mem_peak = mem_vals.iter().copied().fold(0.0, f64::max);
                        ui.columns(2, |c| {
                            w::graph(&mut c[0], &cpu_vals, slots, 100.0, 40.0, Some(w::CPU));
                            w::graph(&mut c[1], &mem_vals, slots, mem_peak, 40.0, Some(w::MEM));
                        });
                    }
                }
                None if st.proc_detail.is_none() => {
                    ui.label(RichText::new("waiting for data").color(w::DIM));
                }
                None => {
                    ui.label(RichText::new("no detail").monospace().color(w::DIM));
                }
            }
        });
        let collapsed = !open;
        if collapsed != self.cfg.collapsed_proc_detail {
            self.cfg.collapsed_proc_detail = collapsed;
            let _ = store::save_config(&self.cfg);
        }
    }

    /// Workload roll-up: one row per cgroup boundary (container, unit, host),
    /// summed from the current process sample. Clicking a row filters the
    /// process table to that workload; clicking the active row clears it.
    fn workloads_box(&mut self, ui: &mut Ui, st: &State, viewport_h: f32) -> LinkedTable {
        if st.procs.is_none() || st.workloads.is_empty() {
            return LinkedTable::NONE;
        }
        // Short window: the process table matters more than the roll-up, and
        // the pane would push its first rows offscreen (seen at 480x420).
        // Measured on the viewport, not the (unbounded) group scroll height.
        if viewport_h < 240.0 {
            return LinkedTable::NONE;
        }
        let mut open = !self.cfg.collapsed_workloads;
        let mem_total = st.metrics.as_ref().map(|m| m.mem_total).unwrap_or(0).max(1);
        let mut scroll = LinkedTable::NONE;
        let mut hover: Option<String> = None;
        w::panel(ui, "workloads", &mut open, |ui| {
            let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
            let narrow = ui.available_width() < 640.0;
            let mut pick: Option<String> = None;
            let mut tb = TableBuilder::new(ui)
                .id_salt("workloads-table")
                .striped(true)
                .sense(egui::Sense::click())
                .auto_shrink([false, true])
                .max_scroll_height(row_h * 7.0)
                .column(Column::remainder().at_least(120.0).clip(true))
                .column(Column::exact(52.0))
                .column(Column::exact(64.0))
                .column(Column::exact(64.0));
            if !narrow {
                tb = tb.column(Column::exact(76.0));
            }
            let out = tb.header(row_h + 2.0, |mut h| {
                for name in ["Workload", "PIDs", "CPU%", "MEM%"] {
                    h.col(|ui| {
                        ui.label(RichText::new(name).monospace().strong().color(w::TEXT));
                    });
                }
                if !narrow {
                    h.col(|ui| {
                        ui.label(RichText::new("MEM").monospace().strong().color(w::TEXT));
                    });
                }
            })
            .body(|body| {
                body.rows(row_h, st.workloads.len(), |mut row| {
                    let g = &st.workloads[row.index()];
                    let active = self.workload_filter.as_deref() == Some(g.key.as_str());
                    row.set_selected(active);
                    // Hover mirror: highlight the row matching the shared key.
                    if self.hover_workload.as_deref() == Some(g.key.as_str()) {
                        row.set_hovered(true);
                    }
                    let mono = |s: String, c: Color32| RichText::new(s).monospace().color(c);
                    let memf = g.mem_bytes as f32 * 100.0 / mem_total as f32;
                    row.col(|ui| { ui.label(mono(view::workload_label(&g.key), if active { w::ACCENT } else { w::TEXT })); });
                    row.col(|ui| { ui.label(mono(g.pids.to_string(), w::DIM)); });
                    row.col(|ui| { w::cpu_bar_cell(ui, g.cpu_usage / 100.0, &format!("{:5.1}", g.cpu_usage)); });
                    row.col(|ui| { ui.label(mono(format!("{memf:5.1}"), w::heat(memf / 100.0))); });
                    if !narrow {
                        row.col(|ui| { ui.label(mono(view::bytes(g.mem_bytes), w::DIM)); });
                    }
                    if row.response().container_hovered() {
                        hover = Some(g.key.clone());
                    }
                    if row.response().clicked() {
                        pick = Some(g.key.clone());
                    }
                });
            });
            scroll.viewport = out.inner_rect.height();
            if let Some(key) = pick {
                self.workload_filter = if self.workload_filter.as_deref() == Some(key.as_str()) {
                    None
                } else {
                    Some(key)
                };
            }
        });
        if !open != self.cfg.collapsed_workloads {
            self.cfg.collapsed_workloads = !open;
            let _ = store::save_config(&self.cfg);
        }
        scroll.hover = hover;
        scroll
    }

    fn proc_box(&mut self, ui: &mut Ui, st: &State, max_h: f32) -> LinkedTable {
        let mut open = !self.cfg.collapsed_proc;
        let mut scroll = LinkedTable::NONE;
        let mut hover: Option<String> = None;
        w::panel(ui, "proc", &mut open, |ui| {
            let Some(p) = &st.procs else {
                ui.label(RichText::new("waiting for data").color(w::DIM));
                return;
            };
            let mem_total = st.metrics.as_ref().map(|m| m.mem_total).unwrap_or(0).max(1);
            let idx = {
                let mut v = view::select_rows(&p.rows, &self.filter, self.sort, self.desc);
                if let Some(w) = &self.workload_filter {
                    v.retain(|&i| st.row_workload.get(i).is_some_and(|k| k == w));
                }
                v
            };
            let want_focus = !self.settings_open && ui.input(|i| i.key_pressed(egui::Key::Slash));
            let mut clear_workload = false;
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("filter name / user / pid (/)").desired_width(220.0));
                if want_focus {
                    r.request_focus();
                }
                if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.filter.clear();
                }
                ui.label(RichText::new(format!("{} of {}", idx.len(), p.rows.len())).monospace().color(w::DIM));
                if let Some(wk) = &self.workload_filter {
                    let chip = format!("workload: {} ✕", view::workload_label(wk));
                    let resp = ui
                        .add(egui::Label::new(RichText::new(chip).monospace().color(w::ACCENT)).sense(egui::Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        clear_workload = true;
                    }
                }
                if p.sampled_at_ms > 0 {
                    ui.label(RichText::new(format!("at {}", local_time(p.sampled_at_ms))).monospace().color(w::DIM));
                }
            });
            if clear_workload {
                self.workload_filter = None;
            }

            let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
            let mut sort = (self.sort, self.desc);
            let mut selected = self.selected;
            // Esc backs out of the selection, filter focused or not (the
            // focused-filter Esc above also clears the filter text).
            if !self.settings_open && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                selected = None;
            }
            // Keyboard selection (zero-dep): Up/Down move in filtered order.
            if !self.settings_open && !idx.is_empty() {
                let up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
                let down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
                if up || down {
                    selected = move_selection(&p.rows, &idx, selected, up);
                }
            }
            let narrow = ui.available_width() < 640.0;
            let mut tb = TableBuilder::new(ui)
                .id_salt("proc-table")
                .striped(true)
                .sense(egui::Sense::click())
                .auto_shrink([false, false])
                .max_scroll_height(max_h)
                .column(Column::exact(64.0))
                .column(Column::remainder().at_least(110.0).clip(true));
            if !narrow {
                tb = tb.column(Column::exact(84.0).clip(true));
            }
            tb = tb.column(Column::exact(20.0)).column(Column::exact(64.0)).column(Column::exact(64.0));
            if !narrow {
                tb = tb.column(Column::exact(76.0));
                // Workload: display only (click a workloads row to filter).
                tb = tb.column(Column::exact(120.0).clip(true));
            }
            let mut cols: Vec<(&str, SortKey)> = vec![("PID", SortKey::Pid), ("Name", SortKey::Name)];
            if !narrow {
                cols.push(("User", SortKey::User));
            }
            cols.extend([("S", SortKey::State), ("CPU%", SortKey::Cpu), ("MEM%", SortKey::Mem)]);
            if !narrow {
                cols.push(("MEM", SortKey::Mem));
            }
            let out = tb.header(row_h + 2.0, |mut h| {
                for (name, key) in &cols {
                    let resp = h.col(|ui| {
                        let on = sort.0 == *key;
                        let txt = if on { format!("{name}{}", if sort.1 { "▼" } else { "▲" }) } else { name.to_string() };
                        // Text-only sort marker (same clickable-Label pattern
                        // as panel titles): the filled Accent background is
                        // reserved for row selection and view toggles, so the
                        // three blue fills stop competing.
                        ui.add(egui::Label::new(
                            RichText::new(txt).monospace().strong().color(if on { w::ACCENT } else { w::TEXT }),
                        ).sense(egui::Sense::click()));
                    }).1;
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        let on = sort.0 == *key;
                        sort = if on { (*key, !sort.1) } else {
                            // Numbers default to high-first, text to A-Z.
                            (*key, matches!(key, SortKey::Cpu | SortKey::Mem))
                        };
                    }
                }
                if !narrow {
                    h.col(|ui| {
                        ui.label(RichText::new("Workload").monospace().strong().color(w::TEXT));
                    });
                }
            })
            .body(|body| {
                body.rows(row_h, idx.len(), |mut row| {
                    let r = &p.rows[idx[row.index()]];
                    let row_wk = st.row_workload.get(idx[row.index()]).map(String::as_str);
                    row.set_selected(selected == Some(r.pid));
                    // Hover mirror: highlight rows owned by the shared key.
                    if self.hover_workload.as_deref().is_some() && row_wk == self.hover_workload.as_deref() {
                        row.set_hovered(true);
                    }
                    let mono = |s: String, c: Color32| RichText::new(s).monospace().color(c);
                    let memf = r.mem_bytes as f32 * 100.0 / mem_total as f32;
                    row.col(|ui| { ui.label(mono(r.pid.to_string(), w::DIM)); });
                    row.col(|ui| { ui.label(mono(r.name.clone(), w::TEXT)); });
                    if !narrow {
                        row.col(|ui| { ui.label(mono(r.user.clone(), if r.user == "root" { w::WARN } else { w::DIM })); });
                    }
                    row.col(|ui| { ui.label(mono(view::state_letter(&r.state).into(), w::DIM)); });
                    row.col(|ui| { w::cpu_bar_cell(ui, r.cpu_usage / 100.0, &format!("{:5.1}", r.cpu_usage)); });
                    row.col(|ui| { ui.label(mono(format!("{memf:5.1}"), w::heat(memf / 100.0))); });
                    if !narrow {
                        row.col(|ui| { ui.label(mono(view::bytes(r.mem_bytes), w::DIM)); });
                        row.col(|ui| { ui.label(mono(view::workload_label(row_wk.unwrap_or("")), w::DIM)); });
                    }
                    if row.response().container_hovered() {
                        hover = row_wk.map(|s| s.to_string());
                    }
                    if row.response().clicked() {
                        selected = if selected == Some(r.pid) { None } else { Some(r.pid) };
                    }
                });
            });
            scroll.viewport = out.inner_rect.height();
            (self.sort, self.desc) = sort;
            self.selected = selected;
        });
        let collapsed = !open;
        if collapsed != self.cfg.collapsed_proc {
            self.cfg.collapsed_proc = collapsed;
            let _ = store::save_config(&self.cfg);
        }
        scroll.hover = hover;
        scroll
    }

    /// Name/URL checks shared by Settings add and the onboarding test.
    /// Returns the normalized URL (`ws(s)://host:port/path` with the agent
    /// port and `/ws` assumed) so callers store and probe one value.
    fn validate_source(&self, name: &str, url: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("name is empty".to_string());
        }
        if self.cfg.sources.iter().any(|s| s.name == name) {
            return Err(format!("source \"{name}\" already exists"));
        }
        client::normalize_base_url(url).map_err(|e| format!("bad URL: {e}"))
    }

    /// Validate and append a source. Pure cfg mutation; the caller saves.
    /// The stored URL is the normalized form (bare hosts gain `ws://`,
    /// port 45976 and `/ws`).
    fn add_source(&mut self, name: &str, url: &str) -> Result<(), String> {
        let normalized = self.validate_source(name, url)?;
        self.cfg.sources.push(store::Source { name: name.trim().to_string(), url: normalized });
        if self.cfg.sources.len() == 1 {
            self.cfg.active = 0;
        }
        Ok(())
    }

    /// Current wizard triple: (trimmed name, normalized URL or "", cleaned
    /// token). Used to gate Save on a green Test of these exact values.
    fn onboard_current(&self) -> (String, String, String) {
        match self.onboard.as_ref() {
            Some(ob) => (
                ob.name.trim().to_string(),
                client::normalize_base_url(&ob.url).unwrap_or_default(),
                client::clean_token(&ob.token),
            ),
            None => (String::new(), String::new(), String::new()),
        }
    }

    /// Remove a source and keep `active` valid. Pure cfg mutation; the
    /// caller saves and restarts the client.
    fn remove_source(&mut self, idx: usize) {
        if idx >= self.cfg.sources.len() {
            return;
        }
        self.cfg.sources.remove(idx);
        if self.cfg.sources.is_empty() {
            self.cfg.active = 0;
        } else if self.cfg.active >= self.cfg.sources.len() {
            self.cfg.active = self.cfg.sources.len() - 1;
        } else if idx < self.cfg.active {
            self.cfg.active -= 1;
        }
    }

    /// Token, URL field and client follow `cfg.active`. Call after any
    /// source switch, removal, or onboarding completion.
    fn use_active_source(&mut self, ctx: &egui::Context) {
        let (token, src) = match self.cfg.active_source() {
            Some(s) => store::load_token_for(&s.name),
            None => (None, TokenSource::Missing),
        };
        self.token = token;
        self.token_src = src;
        self.url_edit = self.cfg.active_source().map(|s| s.url.clone()).unwrap_or_default();
        self.token_edit.clear();
        self.start_client(ctx);
        self.update_title(ctx);
    }

    /// Window title follows the active source.
    fn update_title(&self, ctx: &egui::Context) {
        let title = match self.cfg.active_source() {
            Some(s) => format!("EBBTOP {}", s.name),
            None => "EBBTOP".to_string(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    /// One-shot connection test for a source row. The result arrives on
    /// `test_rx` and is polled in `settings()`.
    fn start_probe(&mut self, idx: usize) {
        self.ensure_rt();
        let Some(s) = self.cfg.sources.get(idx) else { return };
        let Some(rt) = &self.rt else {
            self.source_msg = Some("test unavailable (no runtime)".to_string());
            return;
        };
        let token = if idx == self.cfg.active {
            self.token.clone()
        } else {
            store::load_token_for(&s.name).0
        };
        let url = s.url.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.test_rx = Some(rx);
        self.testing = true;
        self.source_msg = None;
        rt.spawn(async move {
            let r = client::probe_once(&url, token.as_deref(), Duration::from_secs(5)).await;
            let _ = tx.send(r.map(|ok| ok.hostname));
        });
    }

    /// Wizard connection test: validates the fields, then probes once.
    /// The result arrives on the wizard's channel, polled in `onboarding()`.
    fn onboard_test(&mut self) {
        let (name, url, tok) = match self.onboard.as_ref() {
            Some(ob) => (ob.name.clone(), ob.url.clone(), ob.token.clone()),
            None => return,
        };
        let normalized = match self.validate_source(&name, &url) {
            Ok(u) => u,
            Err(e) => {
                if let Some(ob) = self.onboard.as_mut() {
                    ob.msg = Some(e);
                }
                return;
            }
        };
        let tok = client::clean_token(&tok);
        if tok.is_empty() {
            if let Some(ob) = self.onboard.as_mut() {
                ob.msg = Some(
                    "paste the agent token: on the agent host run `ssh user@host cat services/ebbtop-agent/ebbtop.token`".to_string(),
                );
            }
            return;
        }
        self.ensure_rt();
        let Some(rt) = &self.rt else {
            if let Some(ob) = self.onboard.as_mut() {
                ob.msg = Some("test unavailable (no runtime)".to_string());
            }
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        if let Some(ob) = self.onboard.as_mut() {
            ob.rx = Some(rx);
            ob.testing = true;
            ob.msg = None;
            ob.probe_for = Some((name.trim().to_string(), normalized.clone(), tok.clone()));
        }
        rt.spawn(async move {
            let r = client::probe_once(&normalized, Some(tok.as_str()), Duration::from_secs(5)).await;
            let _ = tx.send(r.map(|ok| ok.hostname));
        });
    }

    /// Wizard save: requires a green Test of the exact current values, then
    /// stores the source + token and connects. A keyring failure keeps the
    /// token for this session (as in Settings) rather than blocking first run.
    fn onboard_save(&mut self, ctx: &egui::Context) {
        let current = self.onboard_current();
        let tested_ok = self.onboard.as_ref().and_then(|ob| ob.tested.clone()) == Some(current.clone());
        if !tested_ok {
            if let Some(ob) = self.onboard.as_mut() {
                ob.msg = Some("Test the connection first — Save unlocks after a green Test for these exact values".to_string());
            }
            return;
        }
        let (name, _, tok) = current;
        if tok.is_empty() {
            if let Some(ob) = self.onboard.as_mut() {
                ob.msg = Some(
                    "paste the agent token: on the agent host run `ssh user@host cat services/ebbtop-agent/ebbtop.token`".to_string(),
                );
            }
            return;
        }
        if let Err(e) = self.add_source(&name, &self.onboard.as_ref().map(|ob| ob.url.clone()).unwrap_or_default()) {
            if let Some(ob) = self.onboard.as_mut() {
                ob.msg = Some(e);
            }
            return;
        }
        self.cfg.active = self.cfg.sources.len() - 1;
        let user = self.cfg.active_source().map(|s| s.name.clone()).unwrap_or_default();
        match store::save_token_for(&user, &tok) {
            Ok(()) => self.token_src = TokenSource::Keyring,
            Err(e) => {
                self.token_src = TokenSource::Error(e.to_string());
                store::append_log("onboarding: token kept for this session only");
            }
        }
        self.token = Some(tok);
        self.url_edit = self.cfg.active_source().map(|s| s.url.clone()).unwrap_or_default();
        let _ = store::save_config(&self.cfg);
        self.onboard = None;
        self.start_client(ctx);
        self.update_title(ctx);
    }

    fn onboarding(&mut self, ctx: &egui::Context) {
        // Poll the in-flight probe, if any.
        if let Some(ob) = self.onboard.as_mut()
            && ob.testing
        {
            match ob.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                Some(r) => {
                    ob.testing = false;
                    ob.rx = None;
                    match r {
                        Ok(host) => {
                            ob.tested = ob.probe_for.clone();
                            ob.msg = Some(format!("connected: {host}"));
                        }
                        Err(e) => {
                            ob.tested = None;
                            ob.msg = Some(client::probe_hint(&e));
                        }
                    }
                    ob.probe_for = None;
                }
                None if ob.rx.is_none() => ob.testing = false,
                None => {}
            }
        }
        let mut open = self.onboard.is_some();
        let mut skip = false;
        egui::Window::new("Welcome to EbbTop")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Add your first host. The token is stored in the OS credential store.");
                ui.label(RichText::new("Agent port 45976; token file ~/services/ebbtop-agent/ebbtop.token on the agent host.").small().color(w::DIM));
                ui.label(RichText::new("Fetch it with: ssh user@host cat services/ebbtop-agent/ebbtop.token (| pbcopy / | Set-Clipboard)").small().color(w::DIM));
                ui.label(RichText::new("Tip: a bare hostname works — ws://, :45976 and /ws are added.").small().color(w::DIM));
                ui.add_space(6.0);
                let mut test = false;
                let mut save = false;
                // Snapshot for the gated Save button; computed before the
                // mutable borrow of the wizard fields below.
                let current = self.onboard_current();
                let tested = self.onboard.as_ref().and_then(|ob| ob.tested.clone());
                let can_save = tested.is_some() && tested == Some(current.clone())
                    && !current.0.is_empty()
                    && !current.1.is_empty()
                    && !current.2.is_empty();
                let assumed = if current.1.is_empty() { None } else { Some(current.1.clone()) };
                let tok_warn = client::token_warning(&current.2);
                {
                    let ob = self.onboard.as_mut().expect("wizard open");
                    egui::Grid::new("onboard").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut ob.name).desired_width(220.0).hint_text("myserver"));
                        ui.end_row();
                        ui.label("URL");
                        ui.add(egui::TextEdit::singleline(&mut ob.url).desired_width(220.0).hint_text("myserver or ws://host:45976/ws"));
                        ui.end_row();
                        ui.label("Token");
                        ui.add(egui::TextEdit::singleline(&mut ob.token).password(true).desired_width(220.0).hint_text("paste the agent token"));
                        ui.end_row();
                    });
                    if let Some(u) = assumed {
                        ui.label(RichText::new(format!("will connect to {u}")).small().color(w::DIM));
                    }
                    if let Some(w) = tok_warn {
                        ui.label(RichText::new(w).small().color(w::WARN));
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Test").clicked() {
                            test = true;
                        }
                        let save_btn = ui.add_enabled(can_save, egui::Button::new("Save and connect"));
                        if save_btn.clicked() {
                            save = true;
                        }
                        if ui.button("Skip").clicked() {
                            skip = true;
                        }
                    });
                    if !can_save {
                        ui.label(RichText::new("Save unlocks after a green Test for these exact values.").small().color(w::DIM));
                    }
                    if ob.testing {
                        ui.label(RichText::new("testing…").color(w::DIM));
                    }
                    if let Some(m) = &ob.msg {
                        ui.label(RichText::new(m).color(w::WARN));
                    }
                }
                if test {
                    self.onboard_test();
                }
                if save {
                    self.onboard_save(ctx);
                }
                if skip {
                    self.onboard = None;
                }
            });
        if !open {
            self.onboard = None;
        }
    }

    fn settings(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        let mut save = false;
        // Poll the in-flight connection test, if any.
        if self.testing {
            match self.test_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                Some(r) => {
                    self.testing = false;
                    self.test_rx = None;
                    self.source_msg = Some(match r {
                        Ok(host) => format!("connected: {host}"),
                        Err(e) => client::probe_hint(&e),
                    });
                }
                None if self.test_rx.is_none() => self.testing = false,
                None => {}
            }
        }
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (SettingsTab::Sources, "Sources"),
                        (SettingsTab::Display, "Display"),
                        (SettingsTab::Connection, "Connection"),
                        (SettingsTab::Updates, "Updates"),
                    ] {
                        if ui.selectable_label(self.settings_tab == tab, label).clicked() {
                            self.settings_tab = tab;
                        }
                    }
                });
                ui.separator();
                match self.settings_tab {
                    SettingsTab::Sources => self.settings_sources(ctx, ui),
                    SettingsTab::Display => self.settings_display(ui),
                    SettingsTab::Connection => {
                        if self.settings_connection(ui) {
                            save = true;
                        }
                    }
                    SettingsTab::Updates => self.settings_updates(ui),
                }
            });
        if save {
            self.save_settings(ctx);
            if self.settings_msg.is_none() {
                open = false;
            }
        }
        self.settings_open = open;
    }

    fn settings_sources(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        if self.cfg.sources.is_empty() {
            ui.label(RichText::new("no sources yet; add one below").color(w::DIM));
        }
        let n = self.cfg.sources.len();
        for i in 0..n {
            let (name, url, is_active) = {
                let s = &self.cfg.sources[i];
                (s.name.clone(), s.url.clone(), i == self.cfg.active)
            };
            // Removal shrinks `sources` mid-loop; stop after one so the
            // fixed range never indexes past the end.
            let mut removed = false;
            ui.horizontal(|ui| {
                if ui.selectable_label(is_active, &name).on_hover_text("set active source").clicked()
                    && !is_active
                {
                    self.cfg.active = i;
                    let _ = store::save_config(&self.cfg);
                    self.use_active_source(ctx);
                }
                ui.label(RichText::new(&url).monospace().small().color(w::DIM));
                if ui.button("Test").on_hover_text(format!("connect once to {name}")).clicked() {
                    self.start_probe(i);
                }
                if ui.button("Remove").clicked() {
                    self.remove_source(i);
                    store::delete_token_for(&name);
                    let _ = store::save_config(&self.cfg);
                    self.use_active_source(ctx);
                    removed = true;
                }
            });
            if removed {
                break;
            }
        }
        if self.testing {
            ui.label(RichText::new("testing…").color(w::DIM));
        }
        if let Some(m) = &self.source_msg {
            ui.label(RichText::new(m).color(w::WARN));
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut self.source_name_edit).desired_width(110.0).hint_text("myserver"));
            ui.label("URL");
            ui.add(egui::TextEdit::singleline(&mut self.source_url_edit).desired_width(220.0).hint_text("ws://host:port/ws"));
            if ui.button("Add").clicked() {
                let name = self.source_name_edit.clone();
                let url = self.source_url_edit.clone();
                match self.add_source(&name, &url) {
                    Ok(()) => {
                        self.source_name_edit.clear();
                        self.source_url_edit.clear();
                        self.source_msg = Some(format!("added {name}"));
                        let _ = store::save_config(&self.cfg);
                    }
                    Err(e) => self.source_msg = Some(e),
                }
            }
        });
    }

    fn settings_display(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Panels").strong().color(w::TEXT));
        egui::Grid::new("panels").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
            let changed_cpu = ui.checkbox(&mut self.cfg.show_cpu, "CPU").changed();
            let changed_gpu = ui.checkbox(&mut self.cfg.show_gpu, "GPU").changed();
            let changed_mem = ui.checkbox(&mut self.cfg.show_mem, "Mem").changed();
            ui.end_row();
            let changed_net = ui.checkbox(&mut self.cfg.show_net, "Net").changed();
            let changed_disk = ui.checkbox(&mut self.cfg.show_disk, "Disk").changed();
            if changed_cpu || changed_gpu || changed_mem || changed_net || changed_disk {
                let _ = store::save_config(&self.cfg);
            }
        });
        if ui.add(egui::Slider::new(&mut self.cfg.top_frac, 0.25..=0.75).text("Top height")).changed() {
            let _ = store::save_config(&self.cfg);
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.selectable_label(self.cfg.compact, "Dense").on_hover_text("tighter spacing").clicked() {
                self.cfg.compact = !self.cfg.compact;
                let _ = store::save_config(&self.cfg);
                style_with(ui.ctx(), self.cfg.compact);
            }
            for (s, l) in [(300u32, "5m"), (60, "60s")] {
                if ui.selectable_label(self.cfg.window_s == s, l).clicked() && self.cfg.window_s != s {
                    self.cfg.window_s = s;
                    let _ = store::save_config(&self.cfg);
                }
            }
            if ui.checkbox(&mut self.cfg.log_scale, "log scale").changed() {
                let _ = store::save_config(&self.cfg);
            }
        });
    }

    /// Connection tab for the active source. True when Save was pressed.
    fn settings_connection(&mut self, ui: &mut Ui) -> bool {
        let mut save = false;
        if let Some(s) = self.cfg.active_source() {
            ui.label(RichText::new(format!("source: {}", s.name)).strong().color(w::TEXT));
        } else {
            ui.label(RichText::new("no sources yet; add one under Sources").color(w::WARN));
        }
        egui::Grid::new("conn").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Agent URL");
            ui.add(egui::TextEdit::singleline(&mut self.url_edit).desired_width(300.0).hint_text("ws://host:port/ws"));
            ui.end_row();
            ui.label("Token");
            ui.add(egui::TextEdit::singleline(&mut self.token_edit).password(true).desired_width(300.0)
                .hint_text("leave empty to keep the saved token"));
            ui.end_row();
            ui.label("Stored");
            ui.label(match &self.token_src {
                TokenSource::Env => "from environment variable".to_string(),
                TokenSource::Keyring => "in OS credential store".to_string(),
                TokenSource::Missing => "no token saved".to_string(),
                TokenSource::Error(e) => format!("credential store error: {e}"),
            });
            ui.end_row();
        });
        ui.add_space(6.0);
        if let Some(p) = store::config_path() {
            ui.label(RichText::new(format!("config: {}", p.display())).small().color(w::DIM));
        }
        if let Some(m) = &self.settings_msg {
            ui.label(RichText::new(m).color(w::WARN));
        }
        if ui.button("Save and reconnect").clicked() {
            save = true;
        }
        save
    }

    fn save_settings(&mut self, ctx: &egui::Context) {
        self.settings_msg = None;
        let url = self.url_edit.trim().to_string();
        if let Err(e) = client::connect_url(&url, None) {
            self.settings_msg = Some(format!("bad URL: {e}"));
            return;
        }
        match self.cfg.sources.get_mut(self.cfg.active) {
            Some(s) => s.url = url,
            None => {
                let name = store::name_from_url(&url);
                self.cfg.sources.push(store::Source { name, url });
                self.cfg.active = self.cfg.sources.len() - 1;
            }
        }
        if let Err(e) = store::save_config(&self.cfg) {
            self.settings_msg = Some(format!("config not saved: {e}"));
        }
        let t = std::mem::take(&mut self.token_edit);
        let t = t.trim();
        if !t.is_empty() {
            self.token = Some(t.to_string());
            let user = self.cfg.active_source().map(|s| s.name.clone()).unwrap_or_default();
            match store::save_token_for(&user, t) {
                Ok(()) => self.token_src = TokenSource::Keyring,
                Err(e) => {
                    // Keep it for this session; say so instead of failing silently.
                    self.token_src = TokenSource::Error(e.to_string());
                    self.settings_msg = Some(format!("token used for this session only, store failed: {e}"));
                }
            }
        }
        self.start_client(ctx);
    }

    /// Updates tab: current version, last check outcome, manual check.
    fn settings_updates(&mut self, ui: &mut Ui) {
        ui.label(RichText::new(format!("EbbTop {}", update::current_version())).strong().color(w::TEXT));
        let status = match self.update.status() {
            UpdateStatus::Idle => "never checked".to_string(),
            UpdateStatus::Checking => "checking…".to_string(),
            UpdateStatus::Available { version, .. } => format!("{version} available"),
            UpdateStatus::UpToDate { version } => format!("up to date ({version})"),
            UpdateStatus::Failed(e) => format!("update failed: {e}"),
            UpdateStatus::Accepted { version, .. } => format!("{version} approved"),
            UpdateStatus::Downloading { version, done } => {
                format!("downloading {version} ({} MB)", done / 1_000_000)
            }
            UpdateStatus::Ready { version, .. } => format!("{version} ready to restart"),
        };
        ui.label(RichText::new(status).monospace().color(w::DIM));
        ui.add_space(6.0);
        if ui.button("Check for updates").clicked() {
            self.update.start(update::fetch_latest_release);
        }
        // Re-offer a declined-but-still-available release without
        // re-popping the dialog.
        if let UpdateStatus::Available { version, .. } = self.update.status() {
            let version = version.clone();
            ui.add_space(6.0);
            if ui.button(format!("Download update {version}")).clicked() {
                self.update_open = true;
                self.update.mark_notified();
            }
        }
    }

    /// Update dialog across the whole flow: prompt (Available), progress
    /// (Downloading), restart (Ready), and retry (Failed). Accepting moves
    /// to `Accepted`; the verified download starts on the next frame so
    /// the button click never blocks. Later declines and stays quiet
    /// until a newer release appears.
    fn update_dialog(&mut self, ctx: &egui::Context) {
        // Accepted is transient: kick off the download, then render the
        // resulting Downloading (or Failed) state below.
        if matches!(self.update.status(), UpdateStatus::Accepted { .. }) {
            self.begin_update_download();
        }
        enum Act {
            None,
            Accept,
            Decline,
            Restart,
            Recheck,
        }
        let mut act = Act::None;
        let mut open = self.update_open;
        let title: &str;
        match self.update.status() {
            UpdateStatus::Available { .. } | UpdateStatus::Accepted { .. } => title = "Update available",
            UpdateStatus::Downloading { .. } => title = "Downloading update",
            UpdateStatus::Ready { .. } => title = "Restart to update",
            UpdateStatus::Failed(_) => title = "Update failed",
            _ => {
                self.update_open = false;
                return;
            }
        }
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                match self.update.status() {
                    UpdateStatus::Available { version, .. }
                    | UpdateStatus::Accepted { version, .. } => {
                        ui.label(format!(
                            "EbbTop {version} is available (you have {}).",
                            update::current_version()
                        ));
                        ui.horizontal(|ui| {
                            if ui.button("Download update").clicked() {
                                act = Act::Accept;
                            }
                            if ui.button("Later").clicked() {
                                act = Act::Decline;
                            }
                        });
                    }
                    UpdateStatus::Downloading { version, done } => {
                        ui.label(format!(
                            "Downloading EbbTop {version}… {} MB so far.",
                            done / 1_000_000
                        ));
                    }
                    UpdateStatus::Ready { version, .. } => {
                        ui.label(format!(
                            "EbbTop {version} is downloaded and verified."
                        ));
                        ui.horizontal(|ui| {
                            if ui.button("Restart to update").clicked() {
                                act = Act::Restart;
                            }
                            if ui.button("Later").clicked() {
                                act = Act::Decline;
                            }
                        });
                    }
                    UpdateStatus::Failed(e) => {
                        let e = e.clone();
                        ui.label(RichText::new(format!("Update failed: {e}")).color(w::WARN));
                        if ui.button("Check again").clicked() {
                            act = Act::Recheck;
                        }
                    }
                    _ => {}
                }
            });
        match act {
            Act::None => {}
            Act::Accept => self.accept_update(),
            Act::Decline => {
                self.decline_update();
                open = false;
            }
            Act::Restart => self.restart_for_update(ctx),
            Act::Recheck => self.update.start(update::fetch_latest_release),
        }
        // Accepted is transient (download starts next frame); Failed stays
        // for retry; everything else terminal closes the dialog.
        let done = !matches!(
            self.update.status(),
            UpdateStatus::Available { .. }
                | UpdateStatus::Accepted { .. }
                | UpdateStatus::Downloading { .. }
                | UpdateStatus::Ready { .. }
                | UpdateStatus::Failed(_)
        );
        self.update_open = open && !done;
    }

    /// Start the verified download for the accepted release (spec design,
    /// section 2). On Windows the release's `install.ps1` rides along as a
    /// verified extra so the restart step has an installer. Resolves this
    /// platform's asset plus `SHA256SUMS` from
    /// the carried asset list; a platform with no served asset, or a
    /// release without the needed URLs, lands in `Failed` with the reason.
    fn begin_update_download(&mut self) {
        let (version, assets) = match self.update.status() {
            UpdateStatus::Accepted { version, assets } => (version.clone(), assets.clone()),
            _ => return,
        };
        let Some(asset) = core_update::asset_name_for_current_platform() else {
            self.update.set_failed("updates are not served for this platform".into());
            return;
        };
        let urls = match (
            core_update::release_asset_url(&assets, asset),
            core_update::release_asset_url(&assets, core_update::SUMS_ASSET),
        ) {
            (Some(a), Some(s)) => (a, s),
            _ => {
                self.update.set_failed(format!("release {version} has no download for this platform"));
                return;
            }
        };
        let dest = update::download_dest_path(&version, asset);
        // Windows needs the installer script next to the exe for the
        // restart step; it shares the release's SHA256SUMS, so it rides
        // along as a verified extra on the same job. Other platforms apply
        // with system tools and download nothing extra.
        #[cfg(windows)]
        let extra = update::windows_installer_extra(&version, &assets, &urls.1);
        #[cfg(windows)]
        if extra.is_none() {
            self.update.set_failed(format!("release {version} has no installer for this platform"));
            return;
        }
        #[cfg(not(windows))]
        let extra: Option<update::ExtraDownload> = None;
        store::append_log(&format!("update download start version={version} asset={asset}"));
        self.update
            .start_download(version, urls.0, urls.1, asset.to_string(), dest, extra);
    }

    /// "Download update" pressed: approve and clear any earlier skip.
    fn accept_update(&mut self) {
        let version = match self.update.status() {
            UpdateStatus::Available { version, .. } => version.clone(),
            _ => return,
        };
        self.update.accept();
        self.cfg.update_skipped = String::new();
        let _ = store::save_config(&self.cfg);
        store::append_log(&format!("update accepted version={version}"));
    }

    /// "Later" pressed: decline and persist the skip for this tag; a
    /// newer release still prompts.
    fn decline_update(&mut self) {
        let version = match self.update.status() {
            UpdateStatus::Available { version, .. } | UpdateStatus::Ready { version, .. } => {
                version.clone()
            }
            _ => return,
        };
        // A Ready download stays on disk; declining only quiets the prompt.
        if matches!(self.update.status(), UpdateStatus::Available { .. }) {
            self.update.decline();
        } else {
            self.update.mark_notified();
        }
        self.cfg.update_skipped = version.clone();
        let _ = store::save_config(&self.cfg);
        store::append_log(&format!("update declined version={version}"));
        self.update_open = false;
    }

    /// "Restart to update" pressed: run the verified installer (Windows) or
    /// swap in the verified bundle (macOS), relaunch, and quit. Anything
    /// missing lands in `Failed` — a bad path never kills the running app.
    fn restart_for_update(&mut self, ctx: &egui::Context) {
        let (version, path) = match self.update.status() {
            UpdateStatus::Ready { version, path } => (version.clone(), path.clone()),
            _ => return,
        };
        #[cfg(target_os = "macos")]
        {
            let exe = std::env::current_exe().unwrap_or_default();
            match update::app_bundle_dir(&exe) {
                None => self.update.set_failed(
                    "not running from EbbTop.app; replace the bundle by hand".into(),
                ),
                Some(bundle) => match update::apply_update_macos(&path, &bundle) {
                    Ok(()) => {
                        store::append_log(&format!(
                            "update applied version={version}; relaunching"
                        ));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(e) => self.update.set_failed(e),
                },
            }
        }
        #[cfg(windows)]
        {
            let ps1 = update::windows_installer_dest(&version);
            match update::apply_update_windows(&path, &ps1) {
                Ok(()) => {
                    store::append_log(&format!(
                        "update applied version={version}; restarting"
                    ));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Err(e) => self.update.set_failed(e),
            }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = (ctx, &version, &path);
            self.update.set_failed("updates are not served for this platform".into());
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

#[allow(dead_code)]
pub fn style(ctx: &egui::Context) {
    style_with(ctx, false);
}

pub fn style_with(ctx: &egui::Context, compact: bool) {
    ctx.set_visuals(egui::Visuals::dark());
    ctx.global_style_mut(|s| {
        s.visuals.panel_fill = w::BG;
        s.visuals.window_fill = w::BOX;
        s.visuals.extreme_bg_color = w::BG;
        s.visuals.faint_bg_color = Color32::from_rgb(0x1b, 0x1f, 0x26);
        // Density is a spacing constant only — zero per-frame cost.
        s.spacing.item_spacing = if compact { egui::vec2(4.0, 2.0) } else { egui::vec2(6.0, 3.0) };
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ebbtop_core::client::Event;
    use ebbtop_core::wire::{DiskInfo, DiskIo, GpuMetrics, Metrics, NetworkInfo, Process, Processes};

    fn fake_state(app: &App, now: u64) {
        let mut s = app.shared().lock().unwrap().clone();
        s.apply(Event::Connected, now);
        for k in 0..70u64 {
            let t = now - (70 - k) * 1000;
            s.apply(Event::Metrics {
                m: Metrics {
                    sampled_at_ms: t,
                    cpu_total: (k % 100) as f32,
                    cpu_per_core: (0..12).map(|c| ((c * 7 + k) % 100) as f32).collect(),
                    mem_total: 32 << 30,
                    mem_used: 12 << 30,
                    swap_total: 8 << 30,
                    swap_used: 1 << 30,
                    hostname: "k-server".into(),
                    cpu_temp_c: Some(51.0),
                    disks: vec![],
                    networks: vec![
                        NetworkInfo { name: "lo".into(), received: k * 10, transmitted: k * 10 },
                        NetworkInfo { name: "wlp59s0".into(), received: k * 1_000_000, transmitted: k * 50_000 },
                    ],
                    gpus: Some(vec![GpuMetrics {
                        name: "NVIDIA GeForce GTX 1650".into(),
                        utilization_gpu_pct: 37,
                        mem_used_bytes: 1 << 30,
                        mem_total_bytes: 4 << 30,
                        temperature_c: Some(56.0),
                    }]),
                    load_avg: Some([1.25, 0.9, 0.7]),
                    uptime_s: Some(453_201),
                    disk_io: vec![ebbtop_core::wire::DiskIo { name: "nvme0n1".into(), read_bytes: k * 4_000_000, written_bytes: k * 1_000_000 }],
                    cpu_model: Some("Intel(R) Core(TM) i7-9750H CPU @ 2.60GHz".into()),
                },
                recv_ms: t + 3,
            }, t);
        }
        let mut rows: Vec<Process> = (1..=400)
            .map(|i| Process { pid: i, name: format!("proc{i}"), cpu_usage: 0.1, mem_bytes: 1 << 20,
                user: if i % 3 == 0 { "root".into() } else { "kyler".into() }, state: "Sleeping".into(),
                start_time_ms: 1_700_000_000_000 + i as u64, cgroup: String::new() })
            .collect();
        rows.push(Process { pid: 4242, name: "hottest-proc".into(), cpu_usage: 87.5, mem_bytes: 3 << 30,
            user: "kyler".into(), state: "Runnable".into(), start_time_ms: 1_700_000_002_500, cgroup: String::new() });
        s.apply(Event::Processes { p: Processes { process_count: 401, rows, sampled_at_ms: now - 500 }, recv_ms: now - 497 }, now);
        // Detail for the hottest pid: 30 points of CPU history so the
        // proc-detail graph has something to draw. Only shown when 4242 is
        // selected, so tests without a selection are unaffected.
        {
            use ebbtop_core::wire::{DetailedProcessInfo, ProcessMetricsResponse};
            for k in 0..30u64 {
                let cached = (now - (30 - k) * 1000) / 1000;
                s.apply(Event::ProcessDetail {
                    d: ProcessMetricsResponse {
                        process: DetailedProcessInfo {
                            pid: 4242,
                            name: "hottest-proc".into(),
                            command: "hottest-proc".into(),
                            cpu_usage: 60.0 + (k as f32 % 30.0),
                            mem_bytes: 3 << 30,
                            virtual_mem_bytes: 4 << 30,
                            shared_mem_bytes: None,
                            thread_count: 3,
                            fd_count: None,
                            status: "Running".into(),
                            parent_pid: Some(1),
                            user_id: 1000,
                            group_id: 1000,
                            start_time: 1_762_500_000,
                            cpu_time_user: 0,
                            cpu_time_system: 0,
                            read_bytes: None,
                            write_bytes: None,
                            working_directory: None,
                            executable_path: None,
                            child_processes: vec![],
                            threads: vec![],
                        },
                        cached_at: cached,
                    },
                    recv_ms: now - (30 - k) * 1000 + 10,
                }, now);
            }
        }
        s.apply(Event::Disks { d: vec![DiskInfo { name: "/dev/nvme0n1".into(), total: 1 << 40, available: 1 << 39,
            temperature: None, is_partition: false }], recv_ms: now }, now);
        *app.shared().lock().unwrap() = s;
    }

    /// Replace the fake process list with rows carrying real cgroup paths, so
    /// a screenshot shows several workload groups instead of one host group.
    fn workload_rows(app: &App, now: u64) {
        const PG: &str = "65326a2caf20c491810c381f5b09e81c74c0fef5819e05ddd4205ec0335d25d7";
        const HS: &str = "f2e4d8568a2bbe0a6d80e69459da094966a34b4695c4dc5488f09d440eb06db9";
        const CI: &str = "38e7002d091ca2f3a0c1d3c39f32a1f3d8f9b1a0";
        let base = "/user.slice/user-1000.slice/user@1000.service/app.slice";
        let mk = |pid: u32, name: &str, user: &str, mem: u64, cpu: f32, cg: String| Process {
            pid,
            name: name.into(),
            cpu_usage: cpu,
            mem_bytes: mem,
            user: user.into(),
            state: "Sleeping".into(),
            start_time_ms: 1_700_000_000_000 + pid as u64,
            cgroup: cg,
        };
        let rows = vec![
            mk(1, "systemd", "root", 16 << 20, 0.2, "/init.scope".into()),
            mk(2730, "postgres", "100998", 640 << 20, 3.4, format!("{base}/davy-pg.service/libpod-payload-{PG}")),
            mk(2862, "postgres", "100998", 240 << 20, 1.1, format!("{base}/davy-pg.service/libpod-payload-{PG}")),
            mk(2727, "conmon", "kyler", 3 << 20, 0.4, format!("{base}/davy-pg.service/runtime")),
            mk(2920, "hindsight-api", "kyler", 900 << 20, 6.2, format!("{base}/davy-hindsight.service/libpod-payload-{HS}")),
            mk(2855, "postgres", "100998", 180 << 20, 0.9, format!("{base}/davy-hindsight.service/libpod-payload-{HS}")),
            mk(2853, "conmon", "kyler", 2 << 20, 0.3, format!("{base}/davy-hindsight.service/runtime")),
            mk(2066, "Runner.Worker", "kyler", 210 << 20, 2.7, format!("{base}/wn-ci-runner-2.service/libpod-payload-{CI}")),
            mk(2061, "node", "kyler", 300 << 20, 4.0, format!("{base}/wn-ci-runner-2.service/libpod-payload-{CI}")),
            mk(2106, "hermes", "kyler", 660 << 20, 0.8, format!("{base}/hermes-gateway.service")),
        ];
        let shared = app.shared();
        let mut s = shared.lock().unwrap();
        s.apply(Event::Processes {
            p: Processes { process_count: rows.len() as u64, rows, sampled_at_ms: now },
            recv_ms: now,
        }, now);
    }

    fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        fn walk(s: &egui::Shape, out: &mut Vec<String>) {
            match s {
                egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
                _ => {}
            }
        }
        let mut out = vec![];
        shapes.iter().for_each(|c| walk(&c.shape, &mut out));
        out
    }

    fn render(app: &mut App, w: f32, h: f32) -> Vec<String> {
        render_with(app, w, h, vec![])
    }

    /// Like `render`, but the second frame carries raw input events (key
    /// presses the harness needs to observe). Focus is forced on: the
    /// headless context starts unfocused, so keys would otherwise not land.
    fn render_with(app: &mut App, w: f32, h: f32, events: Vec<egui::Event>) -> Vec<String> {
        render_age(app, w, h, events, Some(0))
    }

    /// Render with the newest metrics sample pinned to a fixed age. The
    /// status badge flips LIVE to STALE after 2 s of wall clock, so a render
    /// that lands late under a parallel test run would otherwise render STALE
    /// for a fixture that was live when it was applied. Tests that assert the
    /// badge state it here (Some(5_000) for STALE) instead of relying on
    /// scheduling.
    fn render_age(app: &mut App, w: f32, h: f32, events: Vec<egui::Event>, age_ms: Option<u64>) -> Vec<String> {
        let ctx = egui::Context::default();
        style(&ctx);
        let mut last = vec![];
        let mut pending = Some(events);
        for _ in 0..3 {
            if let Some(age) = age_ms {
                let shared = app.shared();
                let mut s = shared.lock().unwrap();
                if s.last_new_sample_recv_ms != 0 {
                    s.last_new_sample_recv_ms = client::now_ms().saturating_sub(age);
                }
            }
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
                focused: true,
                ..Default::default()
            };
            if let Some(evs) = pending.take() {
                input.events = evs;
            }
            let mut out = ctx.run_ui(input, |ui| app.draw(ui));
            out.textures_delta.clear(); // no renderer in tests
            last = texts(&out.shapes);
        }
        last
    }

    fn esc_key() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    #[test]
    fn renders_at_half_full_and_minimum_width() {
        for (w, h) in [(960.0, 1000.0), (1920.0, 1080.0), (480.0, 420.0)] {
            let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
            fake_state(&app, client::now_ms());
            let t = render(&mut app, w, h);
            let has = |s: &str| t.iter().any(|x| x.contains(s));
            assert!(has(" LIVE "), "{w}: status not LIVE: {t:?}");
            assert!(has("k-server"), "{w}");
            assert!(has("hottest-proc"), "{w}: top cpu process not first visible row");
            assert!(has("87.5"), "{w}");
            assert!(has("C11"), "{w}: per-core meters missing");
            assert!(has("wlp59s0"), "{w}: busiest non-lo iface not default");
            assert!(has("nvme0n1"), "{w}");
            assert!(has("401 of 401"), "{w}");
            assert!(has("load 1.25 0.90 0.70"), "{w}");
            assert!(has("up 5d 5h"), "{w}");
            // At the minimum size the disk and GPU boxes sit in the scrolled
            // part of the upper area, so check them only when wide enough.
            if w >= 960.0 {
                assert!(t.iter().any(|x| x.contains("nvme0n1 r")), "{w}: busiest-disk graph missing: {t:?}");
                assert!(t.iter().any(|x| x.contains("GTX 1650")), "{w}: GPU name missing");
                assert!(t.iter().any(|x| x.contains("37%")), "{w}: GPU load missing");
                assert!(t.iter().any(|x| x.contains("56°C")), "{w}: GPU temp missing");
                assert!(t.iter().any(|x| x.contains("1.0 GiB / 4.0 GiB")), "{w}: VRAM missing");
            }
        }
    }

    #[test]
    fn collapsed_panels_hide_content_but_keep_title() {
        let mut cfg = Config::default();
        cfg.collapsed_cpu = true;
        cfg.collapsed_gpu = true;
        let mut app = App::with(cfg, Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        let t = render(&mut app, 1920.0, 1080.0);
        assert!(t.iter().any(|x| x.contains("> cpu")), "collapsed cpu title missing: {t:?}");
        assert!(!t.iter().any(|x| x.contains("69.0%")), "cpu content should be hidden");
        assert!(t.iter().any(|x| x.contains("> gpu")), "collapsed gpu title missing");
        // The header line itself carries the model (stays visible collapsed),
        // so the body is what has to disappear: per-GPU row label and VRAM.
        assert!(!t.iter().any(|x| x.contains("VRAM")), "gpu content should be hidden: {t:?}");
        assert!(!t.iter().any(|x| x.contains("G0 ")), "gpu body row should be hidden: {t:?}");
        // Expand again: titles flip back to v.
        app.cfg.collapsed_cpu = false;
        app.cfg.collapsed_gpu = false;
        let t = render(&mut app, 1920.0, 1080.0);
        assert!(t.iter().any(|x| x.contains("v cpu")));
        assert!(t.iter().any(|x| x.contains("GTX 1650")));
    }

    /// One panel header is a single galley (arrow + type label + model), so
    /// the header text is exactly the shape that starts with `> kind`/`v kind`.
    /// Matching on the boundary keeps `cpu` from also matching `cpu-something`.
    fn header_text(t: &[String], kind: &str) -> Option<String> {
        t.iter()
            .find(|s| ["> ", "v "].iter().any(|a| s.strip_prefix(a).is_some_and(|r| {
                r == kind || r.strip_prefix(kind).is_some_and(|rest| rest.starts_with(" · "))
            })))
            .cloned()
    }

    /// GPU model rides on the header line and survives collapsing. Applied
    /// unchanged to the base commit this fails: the base header is bare
    /// `> gpu` with no model anywhere while collapsed.
    #[test]
    fn gpu_header_shows_model_when_collapsed() {
        let mut cfg = Config::default();
        cfg.collapsed_gpu = true;
        let mut app = App::with(cfg, Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        let t = render(&mut app, 1920.0, 1080.0);
        let h = header_text(&t, "gpu").unwrap_or_else(|| panic!("gpu header missing: {t:?}"));
        assert!(h.contains("GTX 1650"), "collapsed gpu header must name the model: {h:?}");
        // Same short form the body rows use: the "NVIDIA " prefix is stripped.
        assert!(!h.contains("NVIDIA"), "header must use the body's short form: {h:?}");
        assert!(h.starts_with("> gpu"), "collapsed header keeps the > glyph: {h:?}");
        // The body really is hidden (the model in the header is all that is left).
        assert!(!t.iter().any(|x| x.contains("VRAM")), "collapsed gpu body leaked: {t:?}");
        // Expanded: the header keeps the same model.
        app.cfg.collapsed_gpu = false;
        let t = render(&mut app, 1920.0, 1080.0);
        let h = header_text(&t, "gpu").expect("gpu header while expanded");
        assert!(h.starts_with("v gpu") && h.contains("GTX 1650"), "expanded header: {h:?}");
        // Invariant 6: the per-GPU body row keeps its own `G0` label and the
        // full name (tooltip source) is untouched by the header going short.
        assert!(t.iter().any(|x| x.contains("G0 GeForce GTX 1650")),
            "per-GPU body row must keep its G0 label: {t:?}");
    }

    /// CPU model rides on the header line and survives collapsing, in both
    /// states. The revert probe (header call site switched back to the plain
    /// `panel`) leaves this compiling and fails on the model assertion.
    #[test]
    fn cpu_header_shows_model_when_collapsed() {
        let mut cfg = Config::default();
        cfg.collapsed_cpu = true;
        let mut app = App::with(cfg, Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        let t = render(&mut app, 1920.0, 1080.0);
        let h = header_text(&t, "cpu").unwrap_or_else(|| panic!("cpu header missing: {t:?}"));
        assert!(h.starts_with("> cpu"), "collapsed header keeps the > glyph: {h:?}");
        assert!(h.contains("i7-9750H"), "collapsed cpu header must name the model: {h:?}");
        app.cfg.collapsed_cpu = false;
        let t = render(&mut app, 1920.0, 1080.0);
        let h = header_text(&t, "cpu").expect("cpu header while expanded");
        assert!(h.starts_with("v cpu") && h.contains("i7-9750H"), "expanded header: {h:?}");
    }

    /// The three sizes `screenshot` writes. The model must be on both headers
    /// at every one, so the committed snapshots are not the only evidence.
    #[test]
    fn headers_show_models_at_every_snapshot_size() {
        for (w, h) in [(960.0f32, 1040.0f32), (1920.0, 1080.0), (640.0, 520.0)] {
            let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
            app.onboard = None;
            fake_state(&app, client::now_ms());
            let t = render(&mut app, w, h);
            let cpu = header_text(&t, "cpu").unwrap_or_else(|| panic!("{w}x{h}: no cpu header"));
            let gpu = header_text(&t, "gpu").unwrap_or_else(|| panic!("{w}x{h}: no gpu header"));
            assert!(cpu.contains("i7-9750H"), "{w}x{h}: cpu model missing: {cpu:?}");
            assert!(gpu.contains("GTX 1650"), "{w}x{h}: gpu model missing: {gpu:?}");
        }
    }

    /// Legacy agent (`cpu_model == None`) and a host with no GPUs: the header
    /// is the type label alone — no separator, no `None`/`unknown` filler,
    /// and the window still renders end to end.
    #[test]
    fn headers_show_label_alone_without_model() {
        let mut cfg = Config::default();
        cfg.collapsed_cpu = true;
        let mut app = App::with(cfg, Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        {
            let shared = app.shared();
            let mut s = shared.lock().unwrap();
            let m = s.metrics.as_mut().unwrap();
            m.cpu_model = None; // legacy agent
            m.gpus = None; // no GPU on this host
        }
        let t = render(&mut app, 1920.0, 1080.0);
        assert_eq!(header_text(&t, "cpu").as_deref(), Some("> cpu"),
            "absent model must leave the bare label, no dangling separator: {t:?}");
        assert!(header_text(&t, "gpu").is_none(), "no gpu panel without GPUs: {t:?}");
        assert!(!t.iter().any(|x| x.contains("None")), "placeholder leaked: {t:?}");
        assert!(!t.iter().any(|x| x.contains("unknown")), "placeholder leaked: {t:?}");
        // End to end still renders: the window, and the cpu body once expanded.
        assert!(t.iter().any(|x| x.contains("k-server")), "window must still render: {t:?}");
        app.cfg.collapsed_cpu = false;
        let t = render(&mut app, 1920.0, 1080.0);
        assert_eq!(header_text(&t, "cpu").as_deref(), Some("v cpu"),
            "expanded header without a model is still the bare label: {t:?}");
        assert!(t.iter().any(|x| x.contains("C11")), "cpu body must still render: {t:?}");
    }

    /// A header click still toggles the panel after the label gained the model:
    /// the click target is the same response, and the whole header line (label
    /// + model) toggles, in both directions.
    #[test]
    fn header_click_still_toggles_the_panel() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        app.onboard = None;
        fake_state(&app, client::now_ms());
        let ctx = egui::Context::default();
        style(&ctx);
        let (w, h) = (1920.0, 1080.0);
        let s0 = frame(&ctx, &mut app, w, h, vec![]);
        // Aim at the *model* part of the cpu header: dots after the label, so
        // the toggle does not depend on hitting the two glyph words.
        let (text, rect) = text_rects(&s0)
            .into_iter()
            .find(|(s, _)| s.starts_with("v cpu"))
            .expect("cpu header");
        assert!(text.contains("i7-9750H"), "header carries the model: {text:?}");
        let target = egui::pos2(rect.right() - 6.0, rect.center().y);

        let click = |ctx: &egui::Context, app: &mut App| {
            let btn = |pressed: bool| egui::Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(ctx, app, w, h, vec![egui::Event::PointerMoved(target)]);
            frame(ctx, app, w, h, vec![btn(true)]);
            frame(ctx, app, w, h, vec![btn(false)]);
            // The toggle lands during the click frame; the next frame paints
            // the new arrow and body state.
            frame(ctx, app, w, h, vec![])
        };
        let s = click(&ctx, &mut app);
        assert!(app.cfg.collapsed_cpu, "clicking the header must collapse the cpu panel");
        let t = texts(&s);
        let h_text = header_text(&t, "cpu").expect("collapsed cpu header");
        assert!(h_text.starts_with("> cpu") && h_text.contains("i7-9750H"),
            "collapsed after the click, model still visible: {h_text:?}");

        let s = click(&ctx, &mut app);
        assert!(!app.cfg.collapsed_cpu, "a second click must expand it again");
        let t = texts(&s);
        let h_text = header_text(&t, "cpu").expect("expanded cpu header");
        assert!(h_text.starts_with("v cpu") && h_text.contains("i7-9750H"),
            "expanded after the click, model still visible: {h_text:?}");
    }

    /// An over-long model elides with `…` on one row at the minimum width and
    /// does not grow the header row.
    #[test]
    fn long_model_elides_without_growing_the_header() {
        // Longer than the 640 px header can hold (a plain `~90 char` model
        // still fits there), so elision is genuinely exercised.
        const LONG: &str = "Intel(R) Xeon(R) Platinum 8480C CPU @ 2.00GHz \
(preproduction qualification sample, 2 sockets, microcode 0x2b000590, all-core turbo 3.80GHz)";
        let mut cfg = Config::default();
        cfg.collapsed_cpu = true;
        cfg.collapsed_gpu = true;
        let mut app = App::with(cfg, Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        app.shared().lock().unwrap().metrics.as_mut().unwrap().cpu_model = Some(LONG.into());

        let ctx = egui::Context::default();
        style(&ctx);
        let (w, h) = (640.0, 520.0);
        // The rendered rows, not the layout job's source text: elision bends
        // the last row, and `Galley::text()` still returns the full job text.
        let header = |shapes: &[egui::epaint::ClippedShape]| -> (Vec<String>, f32) {
            fn walk(s: &egui::Shape, out: &mut Vec<(String, Vec<(String, f32)>)>) {
                match s {
                    egui::Shape::Text(t) => out.push((
                        t.galley.text().to_string(),
                        t.galley.rows.iter().map(|r| (r.text(), r.size.y)).collect(),
                    )),
                    egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
                    _ => {}
                }
            }
            let mut all = vec![];
            shapes.iter().for_each(|c| walk(&c.shape, &mut all));
            let (_, rows) = all
                .into_iter()
                .find(|(job, _)| job.starts_with("> cpu"))
                .unwrap_or_else(|| panic!("cpu header missing"));
            let height = rows.iter().map(|(_, h)| *h).sum::<f32>();
            (rows.into_iter().map(|(t, _)| t).collect(), height)
        };

        let (rows, height) = header(&frame(&ctx, &mut app, w, h, vec![]));
        assert_eq!(rows.len(), 1, "header must stay one row: {rows:?}");
        assert!(rows[0].ends_with('…'), "over-long model must elide: {:?}", rows[0]);

        // Row height is the plain label's row height: the model does not grow
        // it, so siblings below the header never shift at the minimum size.
        app.shared().lock().unwrap().metrics.as_mut().unwrap().cpu_model = None;
        let (bare, bare_height) = header(&frame(&ctx, &mut app, w, h, vec![]));
        assert_eq!(bare, vec!["> cpu".to_string()]);
        assert!((height - bare_height).abs() < 0.5,
            "header row grew: {bare_height} -> {height}");
    }

    #[test]
    fn gpu_panel_hidden_without_gpu() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        app.shared().lock().unwrap().metrics.as_mut().unwrap().gpus = None;
        let t = render(&mut app, 960.0, 1000.0);
        assert!(!t.iter().any(|x| x.contains("VRAM")), "{t:?}");
        assert!(!t.iter().any(|x| x.contains("GTX 1650")), "{t:?}");
    }

    #[test]
    fn filter_and_stale_and_down() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        let now = client::now_ms();
        fake_state(&app, now - 5_000); // newest sample 5 s old
        app.filter = "ROOT".into();
        // Pin the age so the badge is STALE regardless of when this frame runs.
        let t = render_age(&mut app, 960.0, 1000.0, vec![], Some(5_000));
        assert!(t.iter().any(|x| x.contains(" STALE ")), "{t:?}");
        assert!(t.iter().any(|x| x == "133 of 401"));
        app.shared().lock().unwrap().apply(
            Event::Disconnected { reason: "connect timed out".into(), retry_in: Duration::from_secs(2) }, now);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains(" DOWN ")));
        assert!(t.iter().any(|x| x.contains("connect timed out; retry 2.0s")));
    }

    #[test]
    fn keyboard_selection_moves_in_filtered_order() {
        fn proc(pid: u32) -> Process {
            Process { pid, name: format!("p{pid}"), cpu_usage: 0.0, mem_bytes: 0, user: "kyler".into(), state: "Sleeping".into(), start_time_ms: 1, cgroup: String::new() }
        }
        let rows = vec![proc(10), proc(20), proc(30)];
        let idx = vec![0, 1, 2];
        assert_eq!(move_selection(&rows, &idx, None, false), Some(10));
        assert_eq!(move_selection(&rows, &idx, None, true), Some(30));
        assert_eq!(move_selection(&rows, &idx, Some(10), false), Some(20));
        assert_eq!(move_selection(&rows, &idx, Some(10), true), Some(10));
        assert_eq!(move_selection(&rows, &idx, Some(30), false), Some(30));
        assert_eq!(move_selection(&rows, &idx, Some(30), true), Some(20));
        assert_eq!(move_selection(&rows, &[], Some(10), false), None);
        // Filtered subset (e.g. only 20 and 30 survive the filter).
        let idx2 = vec![1, 2];
        assert_eq!(move_selection(&rows, &idx2, Some(20), false), Some(30));
        assert_eq!(move_selection(&rows, &idx2, Some(10), false), Some(20));
    }

    #[test]
    fn missing_token_shows_connection_dialog_without_secret() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        app.settings_open = true;
        app.token_edit = "supersecret".into();
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("no token saved")));
        assert!(!t.iter().any(|x| x.contains("supersecret")), "password field leaked");
    }

    fn two_source_cfg() -> Config {
        Config {
            sources: vec![
                store::Source { name: "k-server".into(), url: "ws://10.0.0.1:1/ws".into() },
                store::Source { name: "lab".into(), url: "ws://10.0.0.2:1/ws".into() },
            ],
            active: 1,
            ..Config::default()
        }
    }

    #[test]
    fn picker_switch_applies_after_draw_lock_without_deadlock() {
        // Desktop repro: the status-bar picker runs inside draw() while the
        // shared-state lock is held. Switching inline re-locked the
        // non-reentrant mutex on the same thread and wedged the UI with no
        // panic and no log (desktop log showed "picker pick=" with no
        // "picker switched"). The picker now only records pending_source;
        // draw() applies it after drop(st).
        let dir = std::env::temp_dir().join(format!("ebbtop-picker-test-{}", std::process::id()));
        // SAFETY: test-only; no other test in this binary touches these
        // dirs through save/append paths concurrently (store tests use
        // explicit temp paths and only assert path shapes).
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &dir);
            std::env::set_var("XDG_DATA_HOME", &dir);
        }
        let mut cfg = two_source_cfg();
        cfg.active = 0;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let mut app = App::with(cfg, Some("t".into()), TokenSource::Env, Some(rt));
        fake_state(&app, client::now_ms());
        app.selected = Some(101);
        let t = render(&mut app, 1920.0, 1080.0);
        assert!(t.iter().any(|x| x.contains("401 of 401")), "{t:?}");
        // What draw() holds across the status bar, and what the picker does.
        // These lines must complete without the shared-state lock: the old
        // code called use_active_source here and blocked forever.
        let shared = app.shared();
        let _held = shared.lock().unwrap();
        assert!(shared.try_lock().is_err(), "precondition: draw holds the lock");
        app.pending_source = Some(1);
        drop(_held);
        // draw()'s post-drop apply, verbatim.
        let ctx = egui::Context::default();
        if let Some(i) = app.pending_source.take() {
            app.cfg.active = i;
            let _ = store::save_config(&app.cfg);
            app.use_active_source(&ctx);
        }
        assert_eq!(app.cfg.active, 1);
        let t = render(&mut app, 1920.0, 1080.0);
        assert!(t.iter().any(|x| x.contains("waiting for data")), "{t:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_defaults_to_connection_tab() {
        let app = App::with(Config::default(), None, TokenSource::Missing, None);
        assert_eq!(app.settings_tab, SettingsTab::Connection);
    }

    #[test]
    fn settings_shows_sources_and_display_tabs() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        app.settings_open = true;
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("Sources")), "sources tab missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("Display")), "display tab missing: {t:?}");
    }

    /// Spin a locally injected check to completion without touching the
    /// network, so render tests never spawn the real GitHub fetch.
    fn settle_update(app: &mut App) {
        for _ in 0..500 {
            if app.update.poll() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("update check thread did not report");
    }

    #[test]
    fn updates_tab_offers_manual_check() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        app.settings_open = true;
        app.settings_tab = SettingsTab::Updates;
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("Updates")), "updates tab missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("Check for updates")), "manual check missing: {t:?}");
        assert!(t.iter().any(|x| x.contains(update::current_version())), "current version missing: {t:?}");
    }

    #[test]
    fn available_update_prompts_with_version() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        app.update.start(|| Ok(r#"{"tag_name": "v99.7.0"}"#.into()));
        settle_update(&mut app);
        // The frame pops the dialog on its own; no manual open needed.
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("v99.7.0")), "new version missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("Download update")), "accept missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("Later")), "decline missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("update v99.7.0 ready")), "status note missing: {t:?}");
    }

    #[test]
    fn downloading_update_shows_progress() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        app.update.test_set_status(UpdateStatus::Downloading { version: "v9.9.9".into(), done: 3_000_000 });
        app.update_open = true;
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("Downloading")), "progress missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("v9.9.9")), "version missing: {t:?}");
        assert!(!t.iter().any(|x| x.contains("Restart to update")), "no restart while downloading: {t:?}");
    }

    #[test]
    fn ready_update_offers_restart() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        app.update.test_set_status(UpdateStatus::Ready {
            version: "v9.9.9".into(),
            path: std::path::PathBuf::from("/tmp/ebbtop-update-v9.9.9-pkg"),
        });
        app.update_open = true;
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("Restart to update")), "restart missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("v9.9.9")), "version missing: {t:?}");
    }

    #[test]
    fn failed_check_shows_quiet_note_and_runs_as_before() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        app.update.start(|| Err("offline".into()));
        settle_update(&mut app);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("update check failed")), "quiet note missing: {t:?}");
        assert!(t.iter().any(|x| x.contains(" LIVE ")), "app should run as before: {t:?}");
        assert!(!t.iter().any(|x| x.contains("Download update")), "no dialog on failure: {t:?}");
    }

    #[test]
    fn sources_tab_lists_sources() {
        let mut app = App::with(two_source_cfg(), Some("t".into()), TokenSource::Env, None);
        app.settings_open = true;
        app.settings_tab = SettingsTab::Sources;
        let t = render(&mut app, 960.0, 1000.0);
        for s in ["k-server", "lab", "ws://10.0.0.1:1/ws", "ws://10.0.0.2:1/ws", "Test"] {
            assert!(t.iter().any(|x| x.contains(s)), "{s} missing: {t:?}");
        }
        assert!(!t.iter().any(|x| x.contains("Top height")), "display controls leak: {t:?}");
    }

    #[test]
    fn display_tab_has_display_controls() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        app.settings_open = true;
        app.settings_tab = SettingsTab::Display;
        let t = render(&mut app, 960.0, 1000.0);
        for s in ["Panels", "Top height", "log scale"] {
            assert!(t.iter().any(|x| x.contains(s)), "{s} missing: {t:?}");
        }
        assert!(!t.iter().any(|x| x.contains("Agent URL")), "connection fields leak: {t:?}");
    }

    #[test]
    fn connection_tab_keeps_agent_url() {
        let mut app = App::with(two_source_cfg(), Some("t".into()), TokenSource::Env, None);
        app.settings_open = true;
        app.settings_tab = SettingsTab::Connection;
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("Agent URL")), "agent url missing: {t:?}");
        assert!(!t.iter().any(|x| x.contains("Top height")), "display controls leak: {t:?}");
    }

    #[test]
    fn onboarding_opens_with_no_sources() {
        let app = App::with(Config::default(), None, TokenSource::Missing, None);
        assert!(app.onboard.is_some());
        let app = App::with(two_source_cfg(), Some("t".into()), TokenSource::Env, None);
        assert!(app.onboard.is_none());
    }

    #[test]
    fn onboarding_renders_welcome_fields() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        let t = render(&mut app, 960.0, 1000.0);
        for s in ["Welcome", "Name", "URL", "Token", "Test", "Save and connect", "Skip"] {
            assert!(t.iter().any(|x| x.contains(s)), "{s} missing: {t:?}");
        }
    }

    #[test]
    fn onboarding_hides_token_value() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        app.onboard.as_mut().unwrap().token = "supersecret".into();
        let t = render(&mut app, 960.0, 1000.0);
        assert!(!t.iter().any(|x| x.contains("supersecret")), "password field leaked");
    }

    #[test]
    fn onboarding_test_validates_fields() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        app.onboard_test();
        let m = app.onboard.as_ref().unwrap().msg.clone().unwrap_or_default();
        assert!(m.contains("name"), "unhelpful: {m}");
    }

    #[test]
    fn onboarding_test_builds_runtime_on_demand() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        {
            let ob = app.onboard.as_mut().unwrap();
            ob.name = "h".into();
            ob.url = "ws://h/ws".into();
            ob.token = "t".into();
        }
        app.onboard_test();
        // Lazy runtime: the first Test builds it instead of reporting
        // "no runtime"; the probe is in flight (or failed fast), never
        // the missing-runtime note.
        let ob = app.onboard.as_ref().unwrap();
        assert!(ob.rx.is_some() || ob.testing, "probe never started");
        assert!(
            !ob.msg.clone().unwrap_or_default().contains("no runtime"),
            "unexpected: {:?}",
            ob.msg
        );
    }

    #[test]
    fn validate_source_rejects_empty_name() {
        let app = App::with(Config::default(), None, TokenSource::Missing, None);
        let e = app.validate_source("", "ws://h/ws").expect_err("empty name must fail");
        assert!(e.contains("name"), "unhelpful: {e}");
    }

    #[test]
    fn source_add_rejects_bad_url() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        let e = app.add_source("x", "http://not-ws/ws").expect_err("bad url must fail");
        assert!(e.contains("ws://"), "unhelpful: {e}");
        assert!(app.cfg.sources.is_empty());
    }

    #[test]
    fn source_add_rejects_duplicate_name() {
        let mut app = App::with(two_source_cfg(), None, TokenSource::Missing, None);
        let e = app.add_source("lab", "ws://10.0.0.9:1/ws").expect_err("dup name must fail");
        assert!(e.contains("lab"), "unhelpful: {e}");
        assert_eq!(app.cfg.sources.len(), 2);
    }

    #[test]
    fn source_add_appends_valid() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        app.add_source("b", "ws://b/ws").expect("valid add");
        assert_eq!(app.cfg.sources.len(), 1);
        assert_eq!(app.cfg.sources[0].name, "b");
    }

    #[test]
    fn validate_source_normalizes_bare_host() {
        let app = App::with(Config::default(), None, TokenSource::Missing, None);
        let normalized = app.validate_source("h", "myserver").expect("bare host must pass");
        assert_eq!(normalized, "ws://myserver:45976/ws");
    }

    #[test]
    fn onboard_save_requires_green_test() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        {
            let ob = app.onboard.as_mut().unwrap();
            ob.name = "h".into();
            ob.url = "ws://127.0.0.1:1/ws".into();
            ob.token = "s3cr3t".into();
        }
        let ctx = egui::Context::default();
        app.onboard_save(&ctx);
        assert!(app.onboard.is_some(), "save must not complete before a green Test");
        assert!(app.cfg.sources.is_empty(), "no source without a green Test");
        let m = app.onboard.as_ref().unwrap().msg.clone().unwrap_or_default();
        assert!(m.contains("Test"), "unhelpful: {m}");
    }

    fn ring_with_rate(rx: f64, tx: f64) -> Ring<NetRate> {
        let mut r = Ring::new(8);
        r.push(NetRate { t_ms: 1, rx_bps: rx, tx_bps: tx });
        r
    }

    #[test]
    fn status_bar_shows_source_picker_with_two_sources() {
        let mut app = App::with(two_source_cfg(), Some("t".into()), TokenSource::Env, None);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("source")), "picker label missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("lab")), "active name missing: {t:?}");
    }

    #[test]
    fn status_bar_names_source_without_metrics() {
        let mut cfg = Config::default();
        cfg.sources = vec![store::Source { name: "solo".into(), url: "ws://s/ws".into() }];
        let mut app = App::with(cfg, Some("t".into()), TokenSource::Env, None);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("solo")), "source name missing: {t:?}");
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("no source")), "empty state missing: {t:?}");
        assert!(!t.iter().any(|x| x.contains("k-server")), "legacy name leaks: {t:?}");
    }

    #[test]
    fn status_bar_hides_picker_with_one_source() {
        let mut cfg = Config::default();
        cfg.sources = vec![store::Source { name: "solo".into(), url: "ws://s/ws".into() }];
        let mut app = App::with(cfg, Some("t".into()), TokenSource::Env, None);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(!t.iter().any(|x| x.contains("source")), "picker should hide: {t:?}");
    }

    /// Minimal live state with two disks carrying I/O, so the picker has
    /// a choice to offer.
    fn two_disk_state(app: &App) {
        let now = client::now_ms();
        let mut s = app.shared().lock().unwrap().clone();
        s.apply(Event::Connected, now);
        for k in 0..5u64 {
            let t = now - (5 - k) * 1000;
            s.apply(Event::Metrics {
                m: Metrics {
                    sampled_at_ms: t,
                    cpu_total: 10.0,
                    cpu_per_core: vec![10.0],
                    mem_total: 8 << 30,
                    mem_used: 4 << 30,
                    swap_total: 0,
                    swap_used: 0,
                    hostname: "t".into(),
                    cpu_temp_c: None,
                    disks: vec![],
                    networks: vec![],
                    gpus: None,
                    load_avg: None,
                    uptime_s: None,
                    disk_io: vec![
                        DiskIo { name: "nvme0n1".into(), read_bytes: k * 1_000_000, written_bytes: k * 100_000 },
                        DiskIo { name: "sda".into(), read_bytes: k * 100, written_bytes: k * 10 },
                    ],
                    cpu_model: None,
                },
                recv_ms: t + 20,
            }, now);
        }
        *app.shared().lock().unwrap() = s;
    }

    #[test]
    fn disk_box_shows_inline_picker_with_two_disks() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        two_disk_state(&app);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("auto")), "disk auto missing: {t:?}");
        // The picker costs no rows: graphs stay above the fold with it.
        assert!(t.iter().any(|x| x.contains("nvme0n1 r")), "busiest graph missing: {t:?}");
    }

    #[test]
    fn disk_box_hides_picker_with_one_disk() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        let t = render(&mut app, 960.0, 1000.0);
        assert!(!t.iter().any(|x| x.contains("auto")), "picker should hide: {t:?}");
        assert!(t.iter().any(|x| x.contains("nvme0n1 r")), "graph missing: {t:?}");
    }

    #[test]
    fn disk_pick_prefers_pinned_with_data() {
        let mut io = HashMap::new();
        io.insert("nvme0n1".to_string(), ring_with_rate(100.0, 100.0));
        io.insert("sda".to_string(), ring_with_rate(1.0, 1.0));
        assert_eq!(pick_disk(&io, Some("sda")), Some("sda"));
    }

    #[test]
    fn disk_pick_falls_back_to_busiest() {
        let mut io = HashMap::new();
        io.insert("nvme0n1".to_string(), ring_with_rate(100.0, 100.0));
        io.insert("sda".to_string(), ring_with_rate(1.0, 1.0));
        assert_eq!(pick_disk(&io, Some("gone")), Some("nvme0n1"));
        assert_eq!(pick_disk(&io, None), Some("nvme0n1"));
        assert_eq!(pick_disk(&HashMap::new(), None), None);
    }

    #[test]
    fn source_remove_keeps_active_valid() {
        let mut app = App::with(two_source_cfg(), None, TokenSource::Missing, None);
        app.remove_source(1);
        assert_eq!(app.cfg.sources.len(), 1);
        assert_eq!(app.cfg.active, 0);
        let mut app = App::with(two_source_cfg(), None, TokenSource::Missing, None);
        app.remove_source(0);
        assert_eq!(app.cfg.active, 0);
        assert_eq!(app.cfg.active_source().unwrap().name, "lab");
    }

    #[test]
    fn proc_detail_shows_header_and_graph_for_selection() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        app.selected = Some(4242);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("proc-detail")), "detail title missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("4242 hottest-proc")), "detail pid/name missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("kyler")), "detail user missing");
        assert!(t.iter().any(|x| x.contains("Running")), "detail state missing");
        assert!(t.iter().any(|x| x.contains("%") && x.contains("3.0 GiB")), "detail cpu/mem/rss missing: {t:?}");
        assert!(!t.iter().any(|x| x.contains("no detail")), "detail should be loaded");
        // Clearing the selection hides the pane entirely.
        app.selected = None;
        let t = render(&mut app, 960.0, 1000.0);
        assert!(!t.iter().any(|x| x.contains("proc-detail")), "detail should hide without selection");
    }

    #[test]
    fn proc_detail_shows_no_detail_while_waiting() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        // No reply at all yet: same copy as the other boxes.
        app.selected = Some(7);
        app.shared().lock().unwrap().proc_detail = None;
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("proc-detail")), "detail title missing: {t:?}");
        assert!(t.iter().any(|x| x == "waiting for data"), "waiting copy missing: {t:?}");
        // 4242 has detail in fake_state; pick a pid without one: stale state.
        fake_state(&app, client::now_ms());
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("no detail")), "stale state missing: {t:?}");
    }

    #[test]
    fn kill_button_lives_in_detail_pane() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        // No selection: no detail pane, no Kill button anywhere.
        let t = render(&mut app, 960.0, 1000.0);
        assert!(!t.iter().any(|x| x.contains("proc-detail")), "{t:?}");
        assert!(!t.iter().any(|x| x == "Kill"), "kill should hide without selection: {t:?}");
        // Selection with detail: header carries the Kill action.
        app.selected = Some(4242);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x == "Kill"), "kill button missing from detail header: {t:?}");
        // Root-owned selection: button present but the dialog never opens
        // from it (disabled). Disabled state is visual; the guard is covered
        // by the enable expression on is_root_owned.
        app.selected = Some(3); // pid 3 is root in fake_state
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x == "Kill"), "kill button should render disabled for root: {t:?}");
    }

    #[test]
    fn proc_detail_collapses_like_other_panels() {
        let mut cfg = Config::default();
        cfg.collapsed_proc_detail = true;
        let mut app = App::with(cfg, Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        app.selected = Some(4242);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("> proc-detail")), "collapsed detail title missing: {t:?}");
        assert!(!t.iter().any(|x| x.contains("4242 hottest-proc")), "detail header should hide when collapsed");
        app.cfg.collapsed_proc_detail = false;
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("v proc-detail")));
        assert!(t.iter().any(|x| x.contains("4242 hottest-proc")));
    }

    #[test]
    fn kill_dialog_names_process_and_pid() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        app.kill_confirm = Some(KillConfirm { pid: 4242, name: "hottest-proc".into(), start_time_ms: 1_700_000_002_500 });
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("Kill process")), "{t:?}");
        assert!(t.iter().any(|x| x.contains("Send SIGTERM to")), "{t:?}");
        assert!(t.iter().any(|x| x.contains("hottest-proc")), "{t:?}");
        assert!(t.iter().any(|x| x.contains("Send TERM")), "{t:?}");
        assert!(t.iter().any(|x| x.contains("Send INT")), "{t:?}");
        assert!(t.iter().any(|x| x.contains("Cancel")), "{t:?}");
        assert!(t.iter().any(|x| x.contains("The server will terminate")), "{t:?}");
    }

    #[test]
    fn kill_dialog_names_active_source() {
        let mut app = App::with(two_source_cfg(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        app.kill_confirm = Some(KillConfirm { pid: 4242, name: "hottest-proc".into(), start_time_ms: 1_700_000_002_500 });
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("lab will terminate")), "{t:?}");
    }

    #[test]
    fn kill_result_shows_in_status_bar() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        let now = client::now_ms();
        fake_state(&app, now);
        app.shared().lock().unwrap().apply(
            Event::KillResult {
                r: ebbtop_core::wire::KillResult { ok: true, pid: 4242, signal: "TERM".into(), error: None },
                recv_ms: now,
            },
            now,
        );
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("kill 4242 TERM sent")), "{t:?}");
        app.shared().lock().unwrap().apply(
            Event::KillResult {
                r: ebbtop_core::wire::KillResult {
                    ok: false, pid: 4242, signal: "TERM".into(), error: Some("stale start_time".into()),
                },
                recv_ms: now,
            },
            now,
        );
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("kill 4242 refused: stale start_time")), "{t:?}");
    }

    #[test]
    fn kill_success_expires_and_selection_clears_refusal() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        let now = client::now_ms();
        fake_state(&app, now);
        // A success received a minute ago is expired: hidden.
        app.shared().lock().unwrap().apply(
            Event::KillResult {
                r: ebbtop_core::wire::KillResult { ok: true, pid: 4242, signal: "TERM".into(), error: None },
                recv_ms: now - 60_000,
            },
            now,
        );
        let t = render(&mut app, 960.0, 1000.0);
        assert!(!t.iter().any(|x| x.contains("kill 4242")), "expired success should hide: {t:?}");
        // A fresh refusal shows...
        app.shared().lock().unwrap().apply(
            Event::KillResult {
                r: ebbtop_core::wire::KillResult {
                    ok: false, pid: 4242, signal: "TERM".into(), error: Some("stale start_time".into()),
                },
                recv_ms: now,
            },
            now,
        );
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("kill 4242 refused")), "{t:?}");
        // ...until the selection changes, which retires it via the mirror.
        app.selected = Some(7);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(!t.iter().any(|x| x.contains("kill 4242")), "selection change should clear: {t:?}");
    }

    fn rich_detail(app: &App) {
        let shared = app.shared();
        let mut s = shared.lock().unwrap();
        let p = &mut s.proc_detail.as_mut().unwrap().process;
        p.command = "/usr/bin/hottest-proc --serve --port 8080".into();
        p.shared_mem_bytes = Some(512 << 20);
        p.cpu_time_user = 1_500_000;
        p.cpu_time_system = 500_000;
        p.working_directory = Some("/home/user/work".into());
        p.executable_path = Some("/usr/bin/hottest-proc".into());
        p.child_processes = vec![
            kid(5001, "worker-a"),
            kid(5002, "worker-b"),
        ];
        p.threads = (0..7u32)
            .map(|k| ebbtop_core::wire::ThreadInfo {
                tid: 9001 + k,
                name: format!("t{k}"),
                cpu_time_user: (7 - k) as u64 * 1_000_000,
                cpu_time_system: 0,
                status: "Running".into(),
            })
            .collect();
    }

    fn kid(pid: u32, name: &str) -> ebbtop_core::wire::DetailedProcessInfo {
        ebbtop_core::wire::DetailedProcessInfo { pid, name: name.into(), ..Default::default() }
    }

    #[test]
    fn local_datetime_shapes() {
        assert_eq!(local_datetime(0), "--");
        let s = local_datetime(1_762_500_000);
        assert_eq!(s.len(), 16, "shape: {s}");
        assert!(s.starts_with("20"), "shape: {s}");
    }

    #[test]
    fn proc_detail_shows_extended_facts() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        rich_detail(&app);
        app.selected = Some(4242);
        let t = render(&mut app, 960.0, 1000.0);
        for s in ["vmem 4.0 GiB", "shared 512 MiB", "cpu 2.0s", "cwd /home/user/work", "Copy"] {
            assert!(t.iter().any(|x| x.contains(s)), "{s} missing: {t:?}");
        }
        assert!(t.iter().any(|x| x.contains("started 20")), "started missing: {t:?}");
    }

    #[test]
    fn proc_detail_lists_children() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        rich_detail(&app);
        app.selected = Some(4242);
        let t = render(&mut app, 960.0, 1000.0);
        for s in ["children:", "5001 worker-a", "5002 worker-b"] {
            assert!(t.iter().any(|x| x.contains(s)), "{s} missing: {t:?}");
        }
    }

    #[test]
    fn proc_detail_shows_top_threads() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        rich_detail(&app);
        app.selected = Some(4242);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("TID")), "thread header missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("9001")), "top thread missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("9001 t0 R 7.0s")), "top thread row wrong: {t:?}");
        assert!(t.iter().any(|x| x.contains("+2 more")), "overflow missing: {t:?}");
    }

    #[test]
    fn proc_detail_hides_empty_threads_and_children() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        app.selected = Some(4242);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(!t.iter().any(|x| x.contains("TID")), "threads should hide: {t:?}");
        assert!(!t.iter().any(|x| x.contains("children:")), "children should hide: {t:?}");
    }

    #[test]
    fn proc_detail_surfaces_command_facts_and_exe() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        {
            let shared = app.shared();
            let mut s = shared.lock().unwrap();
            let p = &mut s.proc_detail.as_mut().unwrap().process;
            p.command = "/usr/bin/hottest-proc --serve --port 8080".into();
            p.fd_count = Some(42);
            p.read_bytes = Some(1 << 30);
            p.write_bytes = Some(1 << 20);
            p.executable_path = Some("/usr/bin/hottest-proc".into());
        }
        app.selected = Some(4242);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("--serve --port 8080")), "command missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("threads 3 · fds 42 · ppid 1")), "facts missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("r 1.0 GiB · w 1.0 MiB")), "io missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("/usr/bin/hottest-proc")), "exe missing: {t:?}");
    }

    #[test]
    fn disk_box_caps_meters_and_drops_text_lines() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        let now = client::now_ms();
        fake_state(&app, now);
        // Six disks, increasing fullness: only the four fullest get meters.
        let disks: Vec<DiskInfo> = (0..6)
            .map(|i| DiskInfo {
                name: format!("/dev/sd{}", (b'a' + i) as char),
                total: 1 << 40,
                available: (1 << 40) - (i as u64 + 1) * (1 << 37),
                temperature: None,
                is_partition: false,
            })
            .collect();
        app.shared().lock().unwrap().apply(Event::Disks { d: disks, recv_ms: now }, now);
        let t = render(&mut app, 1920.0, 1080.0);
        assert!(t.iter().any(|x| x == "sdf"), "fullest disk missing: {t:?}");
        assert!(!t.iter().any(|x| x == "sda"), "emptiest disk should be capped: {t:?}");
        assert!(!t.iter().any(|x| x == "sdb"), "fifth disk should be capped: {t:?}");
        assert!(t.iter().any(|x| x.contains("+2 more")), "overflow count missing: {t:?}");
        assert!(!t.iter().any(|x| x.contains("/s  w")), "per-disk text lines should be gone: {t:?}");
    }

    #[test]
    fn status_bar_keeps_state_and_moves_diagnostics_to_tooltip() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        app.shared().lock().unwrap().reconnects = 3;
        let t = render(&mut app, 1920.0, 1080.0);
        assert!(t.iter().any(|x| x.contains(" LIVE ")), "{t:?}");
        assert!(t.iter().any(|x| x.contains("k-server")), "{t:?}");
        assert!(!t.iter().any(|x| x.contains("sample ")), "sample belongs in the tooltip: {t:?}");
        assert!(!t.iter().any(|x| x.contains("tick p95")), "tick p95 belongs in the tooltip: {t:?}");
        assert!(!t.iter().any(|x| x.contains("reconnects")), "reconnects belong in the tooltip: {t:?}");
    }

    #[test]
    fn escape_clears_selection() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        app.selected = Some(4242);
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("proc-detail")), "setup: detail should show");
        let t = render_with(&mut app, 960.0, 1000.0, vec![esc_key()]);
        assert!(!t.iter().any(|x| x.contains("proc-detail")), "esc should clear selection: {t:?}");
    }

    #[test]
    fn narrow_bar_keeps_refusal_but_hides_success() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        let now = client::now_ms();
        fake_state(&app, now);
        // Refusal stays visible at 640 px.
        app.shared().lock().unwrap().apply(
            Event::KillResult {
                r: ebbtop_core::wire::KillResult {
                    ok: false, pid: 4242, signal: "TERM".into(), error: Some("pid 1".into()),
                },
                recv_ms: now,
            },
            now,
        );
        let t = render(&mut app, 640.0, 520.0);
        assert!(t.iter().any(|x| x.contains("kill 4242 refused")), "refusal must show narrow: {t:?}");
        // Transient success hides at 640 px but shows wide.
        app.shared().lock().unwrap().apply(
            Event::KillResult {
                r: ebbtop_core::wire::KillResult { ok: true, pid: 4242, signal: "TERM".into(), error: None },
                recv_ms: now,
            },
            now,
        );
        let t = render(&mut app, 640.0, 520.0);
        assert!(!t.iter().any(|x| x.contains("kill 4242")), "success should hide narrow: {t:?}");
        let t = render(&mut app, 960.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("kill 4242 TERM sent")), "{t:?}");
    }

    /// Real GPU render to PNG for visual review. Run explicitly:
    /// EBBTOP_SHOT_DIR=/tmp/x cargo test -p ebbtop-gui screenshot -- --ignored
    #[test]
    #[ignore]
    fn screenshot() {
        let dir = std::path::PathBuf::from(std::env::var("EBBTOP_SHOT_DIR").expect("EBBTOP_SHOT_DIR"));
        for (w, h) in [(960.0f32, 1040.0f32), (1920.0, 1080.0), (640.0, 520.0)] {
            let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
            app.onboard = None;
            fake_state(&app, client::now_ms());
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(w, h))
                .wgpu()
                .build_ui(|ui| {
                    style(ui.ctx());
                    // Debug-only id-clash warnings draw a glyph this machine's
                    // fonts lack, which aborts the snapshot instead of saving it.
                    ui.ctx().options_mut(|o| o.warn_on_id_clash = false);
                    app.draw(ui)
                });
            for _ in 0..4 {
                harness.step();
            }
            let img = harness.render().expect("render");
            let path = dir.join(format!("ebbtop-{w}x{h}.png"));
            img.save(&path).expect("save");
            eprintln!("wrote {}", path.display());
        }
        // Extra shot with the hottest process selected so the extended
        // inspector (facts, children, threads, CPU+RSS graphs) is visible.
        {
            let (w, h) = (960.0f32, 1040.0f32);
            let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
            app.onboard = None;
            fake_state(&app, client::now_ms());
            rich_detail(&app);
            app.selected = Some(4242);
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(w, h))
                .wgpu()
                .build_ui(|ui| {
                    style(ui.ctx());
                    // Debug-only id-clash warnings draw a glyph this machine's
                    // fonts lack, which aborts the snapshot instead of saving it.
                    ui.ctx().options_mut(|o| o.warn_on_id_clash = false);
                    app.draw(ui)
                });
            for _ in 0..4 {
                harness.step();
            }
            let img = harness.render().expect("render");
            let path = dir.join(format!("ebbtop-{w}x{h}-detail.png"));
            img.save(&path).expect("save");
            eprintln!("wrote {}", path.display());
        }
        // First-run wizard, floating over an empty main view.
        {
            let (w, h) = (960.0f32, 1040.0f32);
            let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(w, h))
                .wgpu()
                .build_ui(|ui| {
                    style(ui.ctx());
                    // Debug-only id-clash warnings draw a glyph this machine's
                    // fonts lack, which aborts the snapshot instead of saving it.
                    ui.ctx().options_mut(|o| o.warn_on_id_clash = false);
                    app.draw(ui)
                });
            for _ in 0..4 {
                harness.step();
            }
            let img = harness.render().expect("render");
            let path = dir.join(format!("ebbtop-{w}x{h}-onboard.png"));
            img.save(&path).expect("save");
            eprintln!("wrote {}", path.display());
        }
        // Settings on the Sources tab with two hosts configured.
        {
            let (w, h) = (960.0f32, 1040.0f32);
            let mut app = App::with(two_source_cfg(), Some("t".into()), TokenSource::Env, None);
            fake_state(&app, client::now_ms());
            app.settings_open = true;
            app.settings_tab = SettingsTab::Sources;
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(w, h))
                .wgpu()
                .build_ui(|ui| {
                    style(ui.ctx());
                    // Debug-only id-clash warnings draw a glyph this machine's
                    // fonts lack, which aborts the snapshot instead of saving it.
                    ui.ctx().options_mut(|o| o.warn_on_id_clash = false);
                    app.draw(ui)
                });
            for _ in 0..4 {
                harness.step();
            }
            let img = harness.render().expect("render");
            let path = dir.join(format!("ebbtop-{w}x{h}-settings.png"));
            img.save(&path).expect("save");
            eprintln!("wrote {}", path.display());
        }
        // CPU and GPU panels collapsed: the header line is the only place the
        // model can appear, so this is the visual proof for the collapsed state.
        {
            let (w, h) = (960.0f32, 1040.0f32);
            let mut cfg = Config::default();
            cfg.collapsed_cpu = true;
            cfg.collapsed_gpu = true;
            let mut app = App::with(cfg, Some("t".into()), TokenSource::Env, None);
            app.onboard = None;
            fake_state(&app, client::now_ms());
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(w, h))
                .wgpu()
                .build_ui(|ui| {
                    style(ui.ctx());
                    ui.ctx().options_mut(|o| o.warn_on_id_clash = false);
                    app.draw(ui)
                });
            for _ in 0..4 {
                harness.step();
            }
            let img = harness.render().expect("render");
            let path = dir.join(format!("ebbtop-{w}x{h}-collapsed.png"));
            img.save(&path).expect("save");
            eprintln!("wrote {}", path.display());
        }
        // Workload roll-up pane and column, with container group rows.
        {
            let (w, h) = (1920.0f32, 1080.0f32);
            let now = client::now_ms();
            let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
            app.onboard = None;
            fake_state(&app, now);
            workload_rows(&app, now);
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(w, h))
                .wgpu()
                .build_ui(|ui| {
                    style(ui.ctx());
                    // Debug-only id-clash warnings draw a glyph this machine's
                    // fonts lack, which aborts the snapshot instead of saving it.
                    ui.ctx().options_mut(|o| o.warn_on_id_clash = false);
                    app.draw(ui)
                });
            for _ in 0..4 {
                harness.step();
            }
            let img = harness.render().expect("render");
            let path = dir.join(format!("ebbtop-{w}x{h}-workloads.png"));
            img.save(&path).expect("save");
            eprintln!("wrote {}", path.display());
        }
    }

    /// CPU cost of one UI frame (layout + tessellation input, no GPU), 400 procs.
    /// cargo test --release -p ebbtop-gui frame_cost -- --ignored --nocapture
    #[test]
    #[ignore]
    fn frame_cost() {
        let mut app = App::with(Config::default(), Some("t".into()), TokenSource::Env, None);
        app.onboard = None;
        fake_state(&app, client::now_ms());
        let ctx = egui::Context::default();
        style(&ctx);
        let run = |app: &mut App| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(960.0, 1040.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| app.draw(ui));
            out.textures_delta.clear();
            let prims = ctx.tessellate(out.shapes, 1.0);
            std::hint::black_box(prims);
        };
        for _ in 0..10 { run(&mut app); }
        let n = 200;
        let mut v: Vec<f64> = (0..n).map(|_| { let t = std::time::Instant::now(); run(&mut app); t.elapsed().as_secs_f64() * 1e3 }).collect();
        v.sort_by(f64::total_cmp);
        eprintln!("frame_cost_ms p50={:.2} p95={:.2} max={:.2} (n={n})", v[n / 2], v[n * 95 / 100], v[n - 1]);
    }

    /// Workloads pane and per-row workload column, rendered headlessly with
    /// real cgroup paths from k-server. Also covers the click-to-filter path
    /// (the filter state is what a click sets; the click itself is egui's).
    #[test]
    fn workloads_panel_groups_rows_and_filters_the_proc_table() {
        const PG_ID: &str = "65326a2caf20c491810c381f5b09e81c74c0fef5819e05ddd4205ec0335d25d7";
        const HS_ID: &str = "f2e4d8568a2bbe0a6d80e69459da094966a34b4695c4dc5488f09d440eb06db9";
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        let now = client::now_ms();
        let base = "/user.slice/user-1000.slice/user@1000.service/app.slice";
        let rows = vec![
            Process { pid: 1, name: "systemd".into(), cpu_usage: 0.1, mem_bytes: 1 << 20, user: "root".into(),
                state: "Sleeping".into(), start_time_ms: 1, cgroup: "/init.scope".into() },
            Process { pid: 2730, name: "postgres".into(), cpu_usage: 4.0, mem_bytes: 900 << 20, user: "100998".into(),
                state: "Sleeping".into(), start_time_ms: 2,
                cgroup: format!("{base}/davy-pg.service/libpod-payload-{PG_ID}") },
            Process { pid: 2727, name: "conmon".into(), cpu_usage: 0.5, mem_bytes: 3 << 20, user: "kyler".into(),
                state: "Sleeping".into(), start_time_ms: 3,
                cgroup: format!("{base}/davy-pg.service/runtime") },
            Process { pid: 2920, name: "hindsight-api".into(), cpu_usage: 1.0, mem_bytes: 700 << 20, user: "kyler".into(),
                state: "Sleeping".into(), start_time_ms: 4,
                cgroup: format!("{base}/davy-hindsight.service/libpod-payload-{HS_ID}") },
            // Legacy agent row: no cgroup reported, belongs to the host group.
            Process { pid: 999, name: "legacy-proc".into(), cpu_usage: 0.0, mem_bytes: 2 << 20, user: "kyler".into(),
                state: "Sleeping".into(), start_time_ms: 5, cgroup: String::new() },
        ];
        {
            let shared = app.shared();
            let mut s = shared.lock().unwrap();
            s.apply(Event::Connected, now);
            s.apply(Event::Processes {
                p: Processes { process_count: 5, rows, sampled_at_ms: now },
                recv_ms: now,
            }, now);
        }
        let t = render(&mut app, 1600.0, 1000.0);
        assert!(t.iter().any(|x| x.contains("workloads")), "pane title missing: {t:?}");
        assert!(t.iter().any(|x| x == "Workload"), "workload column header missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("davy-pg")), "davy-pg group missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("davy-hindsight")), "hindsight group missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("init.scope")), "unit group missing: {t:?}");
        assert!(t.iter().any(|x| x.contains("(host)")), "host group missing: {t:?}");

        app.workload_filter = Some("davy-pg.service".into());
        let t2 = render(&mut app, 1600.0, 1000.0);
        assert!(t2.iter().any(|x| x.contains("2 of 5")), "workload filter not applied: {t2:?}");
        assert!(t2.iter().any(|x| x.contains("workload: davy-pg")), "active chip missing: {t2:?}");
        assert!(!t2.iter().any(|x| x.contains("hindsight-api")), "unrelated row still listed: {t2:?}");
    }

    // ---- Linked tables: group scroll, independent inner scrolls, hover mirror ----

    fn text_rects(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect)> {
        fn walk(s: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match s {
                egui::Shape::Text(t) => out.push((t.galley.text().to_string(), t.visual_bounding_rect())),
                egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
                _ => {}
            }
        }
        let mut out = vec![];
        shapes.iter().for_each(|c| walk(&c.shape, &mut out));
        out
    }

    fn fill_rects(shapes: &[egui::epaint::ClippedShape]) -> Vec<(egui::Rect, egui::Color32)> {
        fn walk(s: &egui::Shape, out: &mut Vec<(egui::Rect, egui::Color32)>) {
            match s {
                egui::Shape::Rect(r) => out.push((r.rect, r.fill)),
                egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
                _ => {}
            }
        }
        let mut out = vec![];
        shapes.iter().for_each(|c| walk(&c.shape, &mut out));
        out
    }

    /// One frame on a caller-owned context, so scroll offsets and hover state
    /// accumulate across frames (unlike `render`, which resets the context).
    fn frame(ctx: &egui::Context, app: &mut App, w: f32, h: f32, events: Vec<egui::Event>) -> Vec<egui::epaint::ClippedShape> {
        {
            let shared = app.shared();
            let mut s = shared.lock().unwrap();
            if s.last_new_sample_recv_ms != 0 {
                s.last_new_sample_recv_ms = client::now_ms();
            }
        }
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
            focused: true,
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.draw(ui));
        out.textures_delta.clear();
        out.shapes
    }

    fn wheel(dy: f32) -> egui::Event {
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, dy),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// 30 workloads x 8 processes: both tables overflow their caps, so both
    /// inner scroll areas have a non-zero range to scroll independently.
    fn many_workloads(app: &App, now: u64) {
        let mut rows = vec![];
        let mut pid = 1000u32;
        for w in 0..30u32 {
            let cg = format!("/user.slice/user-1000.slice/user@1000.service/app.slice/wl-{w:02}.service");
            for k in 0..8u32 {
                rows.push(Process {
                    pid,
                    name: format!("p{w:02}-{k}"),
                    cpu_usage: k as f32,
                    mem_bytes: (w as u64 + 1) * (10 << 20),
                    user: "kyler".into(),
                    state: "Sleeping".into(),
                    start_time_ms: pid as u64,
                    cgroup: cg.clone(),
                });
                pid += 1;
            }
        }
        let shared = app.shared();
        let mut s = shared.lock().unwrap();
        s.apply(Event::Connected, now);
        s.apply(Event::Processes {
            p: Processes { process_count: rows.len() as u64, rows, sampled_at_ms: now },
            recv_ms: now,
        }, now);
    }

    #[test]
    fn tables_group_scrolls_when_panes_overflow() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        // Drop the resource boxes so the group gets a predictable viewport.
        app.cfg.show_cpu = false;
        app.cfg.show_gpu = false;
        app.cfg.show_mem = false;
        app.cfg.show_net = false;
        app.cfg.show_disk = false;
        let now = client::now_ms();
        many_workloads(&app, now);
        let ctx = egui::Context::default();
        style(&ctx);
        // Short window: workloads still shows (>=240px) but workloads+proc
        // overflow, so the GROUP scrolls instead of squeezing the proc table.
        // A couple of frames let the proc-chrome calibration settle.
        let mut s = vec![];
        for _ in 0..3 {
            s = frame(&ctx, &mut app, 1600.0, 350.0, vec![]);
        }
        let t = texts(&s);
        assert!(t.iter().any(|x| x.contains("workloads")), "workloads pane should show: {t:?}");
        assert!(t.iter().any(|x| x.contains("proc")), "proc pane should show: {t:?}");
        assert!(app.group_scroll_max > 0.0, "group should scroll when panes overflow");
        // Tall window: everything fits, no group scroll (looks like before).
        for _ in 0..3 {
            frame(&ctx, &mut app, 1600.0, 1200.0, vec![]);
        }
        assert_eq!(app.group_scroll_max, 0.0, "group should not scroll when content fits");
    }

    #[test]
    fn table_inner_scrolls_are_independent() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        let now = client::now_ms();
        many_workloads(&app, now);
        let ctx = egui::Context::default();
        style(&ctx);
        let (w, h) = (1600.0, 1000.0);
        let s1 = frame(&ctx, &mut app, w, h, vec![]);
        let tr1 = text_rects(&s1);
        // Topmost "wl-29" is the workloads pane's first row (biggest memory);
        // the same label also appears lower down in the proc Workload column.
        let wl_y = tr1.iter().filter(|(s, _)| s == "wl-29")
            .map(|(_, r)| r.center().y)
            .min_by(|a, b| a.partial_cmp(b).unwrap())
            .expect("wl-29 workload row visible");
        let proc_pos = tr1.iter().find(|(s, _)| s == "p00-7").map(|(_, r)| r.center())
            .expect("a proc row visible");
        let proc_y_before = tr1.iter().find(|(s, _)| s == "p00-7").map(|(_, r)| r.center().y).unwrap();
        // Wheel down over the proc table only.
        for _ in 0..3 {
            frame(&ctx, &mut app, w, h, vec![egui::Event::PointerMoved(proc_pos), wheel(-400.0)]);
        }
        let s2 = frame(&ctx, &mut app, w, h, vec![egui::Event::PointerMoved(proc_pos)]);
        let tr2 = text_rects(&s2);
        // The proc body scrolled: the aimed row moved up or off the top.
        let proc_y_after = tr2.iter().find(|(s, _)| s == "p00-7").map(|(_, r)| r.center().y);
        assert!(proc_y_after.map_or(true, |y| y < proc_y_before - 1.0),
            "proc table should have scrolled under the wheel");
        // The workloads body did NOT follow: its first row stays put.
        let wl_y_after = tr2.iter().filter(|(s, _)| s == "wl-29")
            .map(|(_, r)| r.center().y)
            .min_by(|a, b| a.partial_cmp(b).unwrap())
            .expect("wl-29 still visible");
        assert!((wl_y_after - wl_y).abs() < 0.5,
            "workloads must not follow the proc scroll: {wl_y:.1} -> {wl_y_after:.1}");
    }

    #[test]
    fn hover_detection_sets_shared_key() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        let now = client::now_ms();
        workload_rows(&app, now);
        let ctx = egui::Context::default();
        style(&ctx);
        let (w, h) = (1600.0, 1000.0);
        // Settle, then aim at the davy-pg workload row's NAME (topmost occurrence;
        // the same label also appears lower down in the proc Workload column).
        let s0 = frame(&ctx, &mut app, w, h, vec![]);
        let target = text_rects(&s0).iter()
            .filter(|(s, _)| s == "davy-pg")
            .map(|(_, r)| r.center())
            .min_by(|a, b| a.y.partial_cmp(&b.y).unwrap())
            .expect("davy-pg workload row");
        let mut shapes = s0;
        for _ in 0..3 {
            shapes = frame(&ctx, &mut app, w, h, vec![egui::Event::PointerMoved(target)]);
        }
        // Hovering the label text itself must set the key: a child label covering
        // the pointer defeats Response::hovered(), so detection uses container_hovered().
        assert_eq!(app.hover_workload.as_deref(), Some("davy-pg.service"),
            "hovering the workload name must set the shared key");
        // End-to-end: the detected key mirrors onto the member proc rows.
        let hover_fill = egui::Color32::from_gray(70);
        let tr = text_rects(&shapes);
        let pid_rect = tr.iter().find(|(s, _)| s == "2730").map(|(_, r)| *r).expect("proc row 2730");
        assert!(fill_rects(&shapes).iter().any(|(fr, c)| *c == hover_fill && fr.contains(pid_rect.center())),
            "detected hover must mirror onto the proc row");
        // Moving the pointer off the tables clears the key.
        for _ in 0..3 {
            frame(&ctx, &mut app, w, h, vec![egui::Event::PointerMoved(egui::pos2(5.0, 5.0))]);
        }
        assert_eq!(app.hover_workload, None, "hover must clear when the pointer leaves");
    }

    #[test]
    fn hover_mirrors_between_workloads_and_proc() {
        let mut app = App::with(Config::default(), None, TokenSource::Missing, None);
        let now = client::now_ms();
        workload_rows(&app, now);
        let ctx = egui::Context::default();
        style(&ctx);
        let (w, h) = (1600.0, 1000.0);
        let s1 = frame(&ctx, &mut app, w, h, vec![]);
        let tr = text_rects(&s1);
        let hover_fill = egui::Color32::from_gray(70);
        // A row is mirror-highlighted when a hovered-fill rect covers its label.
        let highlighted = |shapes: &[egui::epaint::ClippedShape], label: &str| -> bool {
            let Some(r) = tr.iter().find(|(s, _)| s == label).map(|(_, r)| *r) else { return false };
            fill_rects(shapes).iter().any(|(fr, c)| *c == hover_fill && fr.contains(r.center()))
        };
        // Workload -> proc: the davy-pg key lights its member process rows…
        app.hover_workload = Some("davy-pg.service".into());
        let s2 = frame(&ctx, &mut app, w, h, vec![]);
        assert!(highlighted(&s2, "2730"), "davy-pg proc row not mirrored");
        assert!(highlighted(&s2, "2862"), "second davy-pg proc row not mirrored");
        // …and not rows of another workload.
        assert!(!highlighted(&s2, "hindsight-api"), "unrelated proc row wrongly mirrored");
        // Proc -> workload: the davy-hindsight key lights its workload row and procs.
        app.hover_workload = Some("davy-hindsight.service".into());
        let s3 = frame(&ctx, &mut app, w, h, vec![]);
        assert!(highlighted(&s3, "hindsight-api"), "hindsight proc row not mirrored");
        assert!(highlighted(&s3, "davy-hindsight"), "hindsight workload row not mirrored");
        // Clearing the hover removes the mirror.
        app.hover_workload = None;
        let s4 = frame(&ctx, &mut app, w, h, vec![]);
        assert!(!highlighted(&s4, "2730"), "mirror not cleared");
    }

}
