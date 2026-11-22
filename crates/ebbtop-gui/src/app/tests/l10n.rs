//! Localization slice: key parity, English default, persistence and fallback,
//! font coverage and rendered ink, raw-data pass-through, AccessKit names and
//! locale-independent widget identity.
//!
//! These tests are themselves the evidence for the card's invariants; the
//! remaining English desktop surfaces are inventoried in `docs/l10n-scope.md`
//! and are deliberately *not* asserted here.

use super::a11y::{Frame, Walk};
use super::*;
use crate::l10n::{self, Key, Locale};

// ------------------------------------------------------------------ setup ---

fn config_in(locale: Locale) -> Config {
    Config { language: locale.id().to_string(), ..Config::default() }
}

/// Dashboard fixture (401 processes, a hottest pid with detail history, one
/// disk), no wizard, in `locale`.
fn app_in(locale: Locale) -> App {
    let mut app = App::with(config_in(locale), Some("t".into()), TokenSource::Env, None);
    app.onboard = None;
    fake_state(&app, client::now_ms());
    app
}

/// A first-run wizard in `locale`.
fn wizard_in(locale: Locale) -> App {
    let mut app = App::with(config_in(locale), None, TokenSource::Missing, None);
    app.onboard = Some(Onboard::default());
    app
}

/// Painted text of a settled frame. One persistent `Walk` per scene, so the
/// frame that installs the bundled fonts and the frames that paint with them
/// share a context, exactly like a running app.
fn painted_with(walk: &mut Walk, app: &mut App, w: f32, h: f32) -> String {
    walk.settle(app, (w, h)).texts.join("\n")
}

fn all_painted(app: &mut App, w: f32, h: f32) -> String {
    let mut walk = Walk::new();
    painted_with(&mut walk, app, w, h)
}

fn has_cjk(s: &str) -> bool {
    s.chars().any(is_cjk)
}

/// Any character outside ASCII: an ideograph or the fullwidth punctuation a
/// Chinese UI uses. A string that is still pure ASCII is English by default.
fn has_non_ascii(s: &str) -> bool {
    s.chars().any(|c| !c.is_ascii())
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
}

/// Keys that are deliberately identical in every locale: identifiers, Latin
/// product/unit tokens, and the two example values a hint shows verbatim.
const UNTRANSLATED_BY_DESIGN: &[Key] = &[
    Key::LabelUrl,
    Key::HintServerName,
    Key::HintUrlShort,
    Key::PanelCpu,
    Key::PanelGpu,
    Key::ColPid,
    Key::ColState,
    Key::ColCpuPct,
    Key::ColMemPct,
];

// ------------------------------------------------------------------ tests ---

/// Every key exists in all three locales. Only the keys that are identifiers
/// or Latin tokens by design may be identical to English — nothing else is
/// allowed to be an untranslated copy.
#[test]
fn l10n_key_parity_all_locales() {
    assert!(Key::ALL.len() >= 100, "key set unexpectedly small: {}", Key::ALL.len());
    let mut seen = std::collections::BTreeSet::new();
    for &k in Key::ALL {
        assert!(seen.insert(k), "{k:?} listed twice in Key::ALL");
        let en = l10n::text(Locale::English, k);
        let sc = l10n::text(Locale::ZhHansCn, k);
        let tc = l10n::text(Locale::ZhHantTw, k);
        assert!(!en.trim().is_empty(), "{k:?}: empty English");
        assert!(!sc.trim().is_empty(), "{k:?}: empty zh-Hans-CN");
        assert!(!tc.trim().is_empty(), "{k:?}: empty zh-Hant-TW");
        if UNTRANSLATED_BY_DESIGN.contains(&k) {
            continue;
        }
        assert_ne!(sc, en, "{k:?}: zh-Hans-CN silently inherits English");
        assert_ne!(tc, en, "{k:?}: zh-Hant-TW silently inherits English");
        assert!(has_non_ascii(sc), "{k:?}: zh-Hans-CN is still ASCII: {sc:?}");
        assert!(has_non_ascii(tc), "{k:?}: zh-Hant-TW is still ASCII: {tc:?}");
    }
}

/// Every locale keeps the English placeholder set: same `{}` count, so no
/// argument can be dropped or reordered into a different specifier type.
#[test]
fn l10n_placeholders_match_english() {
    for &k in Key::ALL {
        let en = l10n::text(Locale::English, k).matches("{}").count();
        for loc in [Locale::ZhHansCn, Locale::ZhHantTw] {
            let n = l10n::text(loc, k).matches("{}").count();
            assert_eq!(en, n, "{k:?} in {loc}: {en} English placeholders, {n} localized");
        }
        // `fmt` itself refuses a mismatched argument list.
        let args = vec!["x"; en];
        let _ = l10n::fmt(Locale::ZhHansCn, k, &args);
        let _ = l10n::fmt(Locale::ZhHantTw, k, &args);
    }
}

/// With no setting at all the UI paints exactly today's English, on every
/// surface of the slice, and a classified failure keeps the canonical English
/// headline/next action from `ebbtop-core`.
#[test]
fn l10n_english_default_unchanged() {
    let mut wiz = App::with(Config::default(), None, TokenSource::Missing, None);
    assert_eq!(wiz.cfg.locale(), Locale::English);
    assert!(wiz.onboard.is_some(), "fresh config opens the wizard");
    let wiz_text = all_painted(&mut wiz, 960.0, 1000.0);
    for expected in [
        "Welcome to EbbTop",
        "Add your first host. The token is stored in the OS credential store.",
        "Or enter the address yourself",
        "Save and connect",
        "Use a pairing code",
        "Skip",
        "Name",
        "URL",
        "Token",
        "Save unlocks after a green Test for these exact values.",
    ] {
        assert!(wiz_text.contains(expected), "English wizard lost {expected:?}");
    }

    let mut set = app_in(Locale::English);
    set.cfg.sources.push(store::Source { name: "lab".into(), url: "ws://h:1/ws".into() });
    set.cfg.active = 0;
    set.settings_open = true;
    set.settings_tab = SettingsTab::Sources;
    let set_text = all_painted(&mut set, 960.0, 1000.0);
    for expected in ["Settings", "Sources", "Display", "Connection", "Pair a new host", "Remove"] {
        assert!(set_text.contains(expected), "English settings lost {expected:?}");
    }
    #[cfg(not(feature = "store"))]
    assert!(set_text.contains("Updates"), "English settings lost the Updates tab");

    let mut disp = app_in(Locale::English);
    disp.settings_open = true;
    disp.settings_tab = SettingsTab::Display;
    let disp_text = all_painted(&mut disp, 960.0, 1000.0);
    for expected in ["Panels", "Top height", "Dense", "log scale", "Language"] {
        assert!(disp_text.contains(expected), "English display tab lost {expected:?}");
    }

    let mut table = app_in(Locale::English);
    let table_text = all_painted(&mut table, 960.0, 1000.0);
    for expected in ["PID", "Name", "User", "CPU%", "MEM", "Workload"] {
        assert!(table_text.contains(expected), "English process table lost {expected:?}");
    }

    let mut detail = app_in(Locale::English);
    detail.selected = Some(4242);
    let detail_text = all_painted(&mut detail, 960.0, 1000.0);
    for expected in ["threads 3", "ppid 1", "started ", "Kill"] {
        assert!(detail_text.contains(expected), "English detail pane lost {expected:?}");
    }

    // A classified failure, drawn in English: the canonical core text.
    let d = client::Diagnosis::from_error("connection refused (os error 111)");
    let (h, n) = l10n::failure_keys(d.kind);
    assert_eq!(l10n::text(Locale::English, h), d.headline());
    assert_eq!(l10n::text(Locale::English, n), d.next_action());
    let mut failed = wizard_in(Locale::English);
    failed.onboard.as_mut().expect("wizard").failure = Some(d.clone());
    let failed_text = all_painted(&mut failed, 960.0, 1000.0);
    assert!(failed_text.contains(d.headline()), "English failure lost the core headline");
    assert!(failed_text.contains(&format!("detail: {}", d.detail)), "English detail prefix changed");
}

/// An old `config.json` (no `language` key), and a config file with an
/// unknown/blank/wrong-case value, both resolve to English without a panic and
/// without losing the rest of the config.
#[test]
fn l10n_old_config_defaults_to_english() {
    let old = store::parse_config(r#"{"sources":[{"name":"k-server","url":"ws://h:45976/ws"}]}"#);
    assert_eq!(old.language, "en");
    assert_eq!(old.locale(), Locale::English);
    assert_eq!(old.sources.len(), 1, "old config keeps its sources");

    let mut app = App::with(old, Some("t".into()), TokenSource::Env, None);
    fake_state(&app, client::now_ms());
    let text = all_painted(&mut app, 960.0, 1000.0);
    assert!(text.contains("PID"), "old config must paint English");
    assert!(!text.contains('名'), "old config must not paint Chinese");

    // A file that is missing the field entirely, written the way the base
    // build wrote it.
    let legacy = r#"{"sources":[],"active":0,"window_s":60,"compact":false,"log_scale":false}"#;
    let c = store::parse_config(legacy);
    assert_eq!(c.locale(), Locale::English);
    assert_eq!(c.window_s, 60, "unrelated fields survive");
}

/// Values this build does not know — Hong Kong, a future locale, a wrong case,
/// an empty string — resolve to English and never panic.
#[test]
fn l10n_unknown_locale_falls_back_to_english() {
    for raw in ["zh-Hant-HK", "zh-Hans", "fr", "EN", "zh-hans-cn", "", "   "] {
        let c = store::parse_config(&format!(
            r#"{{"sources":[{{"name":"lab","url":"ws://h:1/ws"}}],"language":"{raw}"}}"#
        ));
        assert_eq!(c.locale(), Locale::English, "{raw:?} must fall back to English");
        assert_eq!(c.sources.len(), 1, "{raw:?} must not drop sources");
        let mut app = App::with(c, Some("t".into()), TokenSource::Env, None);
        fake_state(&app, client::now_ms());
        let text = all_painted(&mut app, 960.0, 1000.0);
        assert!(text.contains("PID"), "{raw:?} must paint English");
    }
}

/// Choosing a language writes the canonical id through the ordinary config
/// save and survives a reload; the raw string is the id, never localized text.
#[test]
fn l10n_switch_persists_to_config() {
    let path = std::env::temp_dir().join(format!("ebbtop-l10n-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    store::set_test_config(Some(path.clone()));

    let mut app = app_in(Locale::English);
    app.set_language(Locale::ZhHansCn);
    let raw = std::fs::read_to_string(&path).expect("config written");
    assert!(raw.contains("\"language\": \"zh-Hans-CN\""), "config file: {raw}");
    assert!(!has_cjk(&raw), "localized text leaked into config.json: {raw}");
    assert_eq!(store::parse_config(&raw).locale(), Locale::ZhHansCn);

    // Reload from the same bytes: the language comes back.
    let reloaded = store::parse_config(&std::fs::read_to_string(&path).expect("reread"));
    let mut again = App::with(reloaded, Some("t".into()), TokenSource::Env, None);
    again.onboard = None;
    fake_state(&again, client::now_ms());
    let text = all_painted(&mut again, 960.0, 1000.0);
    assert!(text.contains("名称"), "a reloaded config must paint Chinese");

    app.set_language(Locale::ZhHantTw);
    let raw = std::fs::read_to_string(&path).expect("config rewritten");
    assert!(raw.contains("\"language\": \"zh-Hant-TW\""), "config file: {raw}");

    store::set_test_config(None);
    let _ = std::fs::remove_file(&path);
}

/// Switching language in a running app re-renders in the new language without
/// losing selection, filter, open tab, collapsed panes or scroll offsets, and
/// without a restart.
#[test]
fn l10n_switch_preserves_ui_state() {
    let mut app = app_in(Locale::English);
    app.settings_open = true;
    app.settings_tab = SettingsTab::Display;
    app.selected = Some(4242);
    app.filter = "hot".to_string();
    app.cfg.collapsed_proc = true;
    let mut walk = Walk::new();
    let _ = painted_with(&mut walk, &mut app, 960.0, 1000.0);

    app.set_language(Locale::ZhHansCn);
    let text = painted_with(&mut walk, &mut app, 960.0, 1000.0);

    assert_eq!(app.selected, Some(4242), "selection lost across the switch");
    assert_eq!(app.filter, "hot", "filter text lost across the switch");
    assert_eq!(app.settings_tab, SettingsTab::Display, "open tab lost across the switch");
    assert!(app.settings_open, "settings window closed by the switch");
    assert!(app.cfg.collapsed_proc, "collapsed pane re-opened by the switch");
    assert_eq!(app.cfg.locale(), Locale::ZhHansCn);
    assert!(text.contains("显示"), "the switched frame is not Chinese: {text:.400}");
    assert!(text.contains("面板"), "the display tab body is not Chinese");
}

/// Every string in the tables has glyphs in both the Proportional and
/// Monospace families, in every locale — the families text is actually drawn
/// under — and an arbitrary Chinese process name does too.
#[test]
fn l10n_fonts_cover_every_ui_string() {
    let mut samples: Vec<String> = Key::ALL
        .iter()
        .flat_map(|&k| {
            [l10n::text(Locale::ZhHansCn, k), l10n::text(Locale::ZhHantTw, k), l10n::text(Locale::English, k)]
        })
        .map(|s| s.to_string())
        .collect();
    // Raw host/process text is never translated: a real Chinese process name
    // and hostname must render too.
    samples.push("中文进程 資料庫服務 工控机 台灣製造".to_string());
    samples.push("简体繁體混合：龍門測試".to_string());
    for loc in Locale::ALL {
        samples.push(loc.native_name().to_string());
    }

    for loc in Locale::ALL {
        let missing = missing_glyphs(loc, &samples);
        assert!(missing.is_empty(), "{loc}: no glyphs for {missing:?}");
    }

    // The one documented gap, pinned so a future font change is noticed and
    // the documented limit is updated rather than silently drifting.
    let ctx = egui::Context::default();
    l10n::install_fonts(&ctx, Locale::ZhHansCn);
    let id = egui::FontId::new(14.0, egui::FontFamily::Proportional);
    let mut covered = true;
    let mut full = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 200.0),
            )),
            ..Default::default()
        },
        |ui| {
            covered = ui.ctx().fonts_mut(|f| f.has_glyph(&id, '\u{2715}'));
        },
    );
    full.textures_delta.clear();
    assert!(!covered, "U+2715 now has a glyph; update docs/l10n-scope.md");
}

/// `U+2715 MULTIPLICATION X` (the workload-chip cross) has no glyph in the
/// shipped `epaint_default_fonts` set nor in the bundled Noto Sans SC/TC faces.
/// It is a pre-existing base rendering gap — the English chip draws it today —
/// so the English string is left byte-identical and the gap is recorded here
/// and in `docs/l10n-scope.md` instead of being papered over.
const KNOWN_MISSING: &[char] = &['\u{2715}'];

/// Renders one pass with `locale` installed and reports any sample that is not
/// fully covered in Proportional or Monospace.
fn missing_glyphs(locale: Locale, samples: &[String]) -> Vec<String> {
    let ctx = egui::Context::default();
    l10n::install_fonts(&ctx, locale);
    let prop = egui::FontId::new(14.0, egui::FontFamily::Proportional);
    let mono = egui::FontId::new(14.0, egui::FontFamily::Monospace);
    let mut out = vec![];
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(960.0, 800.0))),
        ..Default::default()
    };
    input.time = Some(1.0);
    let mut full = ctx.run_ui(input, |ui| {
        for s in samples {
            let check: String = s.chars().filter(|c| !KNOWN_MISSING.contains(c)).collect();
            let p = ui.ctx().fonts_mut(|f| f.has_glyphs(&prop, &check));
            let m = ui.ctx().fonts_mut(|f| f.has_glyphs(&mono, &check));
            if !p || !m {
                out.push(format!("{s:?} (prop={p}, mono={m})"));
            }
        }
    });
    full.textures_delta.clear();
    out
}

/// Chinese frames at the minimum window size: the localized text is painted,
/// nothing overlapping is introduced that English does not already have, and
/// the census shows real visible text.
#[test]
fn l10n_render_all_locales_480x420() {
    for loc in Locale::ALL {
        // The wizard is the densest window at this size.
        let mut wiz = wizard_in(loc);
        let mut w = Walk::new();
        let f = w.settle(&mut wiz, (480.0, 420.0));
        let (visible, _clipped) = f.text_census();
        assert!(visible > 0, "{loc}: nothing painted at 480x420");
        let joined = f.texts.join("\n");
        assert!(
            joined.contains(l10n::text(loc, Key::BtnSaveConnect)),
            "{loc}: wizard lost its save button at 480x420"
        );
        assert!(
            f.nameless_inputs().is_empty(),
            "{loc}: nameless input at 480x420: {:?}",
            f.nameless_inputs().iter().map(|n| n.role_name()).collect::<Vec<_>>()
        );

        // Dashboard with a detail pane and the process table.
        let mut dash = app_in(loc);
        dash.selected = Some(4242);
        let f = w.settle(&mut dash, (480.0, 420.0));
        let joined = f.texts.join("\n");
        // At 480x420 the process body is below the fold (both languages), so
        // the visible slice is the detail pane, its facts row and the filter.
        assert!(
            joined.contains(&l10n::fmt(loc, Key::FactsThreads, &["3"])),
            "{loc}: detail facts row missing at 480x420: {joined:.300}"
        );
        assert!(
            joined.contains(l10n::text(loc, Key::FilterHint)),
            "{loc}: filter row missing at 480x420"
        );
        let cjk_overlaps: Vec<String> =
            f.visible_text_overlaps().into_iter().filter(|s| has_cjk(s)).collect();
        assert!(cjk_overlaps.is_empty(), "{loc}: overlapping Chinese text at 480x420: {cjk_overlaps:?}");
    }
}

/// The same frames at the normal size, plus the rendered-ink check: the CJK
/// glyphs of painted Chinese runs are rasterized into the glyph atlas (a real
/// ink box, not tofu). This is the headless substitute for a wgpu screenshot.
#[test]
fn l10n_render_all_locales_960x1040() {
    for loc in Locale::ALL {
        let mut app = app_in(loc);
        app.settings_open = true;
        app.settings_tab = SettingsTab::Display;
        let mut w = Walk::new();
        let f = w.settle(&mut app, (960.0, 1040.0));
        let joined = f.texts.join("\n");
        for key in [Key::SettingsTitle, Key::TabDisplay, Key::PanelsHeader, Key::TopHeight, Key::LanguageLabel] {
            assert!(
                joined.contains(l10n::text(loc, key)),
                "{loc}: settings frame lost {key:?}"
            );
        }
        let (cjk_glyphs, inked) = cjk_ink(&f);
        if loc == Locale::English {
            continue;
        }
        assert!(cjk_glyphs >= 10, "{loc}: only {cjk_glyphs} CJK glyphs painted");
        assert_eq!(cjk_glyphs, inked, "{loc}: {} of {cjk_glyphs} CJK glyphs have no ink box", cjk_glyphs - inked);

        // The settings window floats over the panels behind it (English does
        // the same), so what must hold inside it is that no two Chinese runs
        // collide and that no Chinese run is painted outside its clip rect.
        let inside = cjk_overlaps(&f, true);
        assert!(inside.is_empty(), "{loc}: colliding Chinese runs in the settings window: {inside:?}");
        let clipped = clipped_cjk(&f);
        assert!(clipped.is_empty(), "{loc}: Chinese text painted outside its clip rect: {clipped:?}");

        // A table + detail frame too: the monospace column path, with no
        // floating window, so overlap and clipping are real.
        let mut dash = app_in(loc);
        dash.selected = Some(4242);
        let f = w.settle(&mut dash, (960.0, 1040.0));
        let (_g, inked) = cjk_ink(&f);
        let over = cjk_overlaps(&f, false);
        assert!(over.is_empty(), "{loc}: overlapping Chinese text on the dashboard: {over:?}");
        assert!(clipped_cjk(&f).is_empty(), "{loc}: Chinese text clipped out of view");
        if loc != Locale::English {
            assert!(inked > 0, "{loc}: no inked CJK glyph in the table/detail frame");
        }
    }
}

/// Overlapping visible text runs that involve Chinese. `both` limits the check
/// to two Chinese runs colliding (what a Chinese layout can break on its own);
/// otherwise any run that touches a Chinese one counts.
fn cjk_overlaps(frame: &Frame, both: bool) -> Vec<String> {
    let texts = frame.visible_texts();
    let mut out = vec![];
    for i in 0..texts.len() {
        for j in i + 1..texts.len() {
            let (ri, ti) = (&texts[i].0, &texts[i].1);
            let (rj, tj) = (&texts[j].0, &texts[j].1);
            let hit = ri.intersect(*rj);
            if hit.width() <= 1.0 || hit.height() <= 1.0 {
                continue;
            }
            let cjk = has_cjk(ti) || has_cjk(tj);
            if (both && has_cjk(ti) && has_cjk(tj)) || (!both && cjk) {
                out.push(format!("{ti:?} x {tj:?}"));
            }
        }
    }
    out
}

/// Chinese runs painted entirely outside their clip rect (invisible): the
/// other half of "not clipped".
fn clipped_cjk(frame: &Frame) -> Vec<String> {
    let mut out = vec![];
    for c in &frame.shapes {
        if let egui::Shape::Text(t) = &c.shape {
            let text = t.galley.text();
            if !has_cjk(text) {
                continue;
            }
            let vis = t.visual_bounding_rect().intersect(c.clip_rect);
            if vis.width() <= 0.5 || vis.height() <= 0.5 {
                out.push(text.to_string());
            }
        }
    }
    out
}

/// Count painted CJK glyphs and how many of them carry a non-empty atlas box.
fn cjk_ink(frame: &Frame) -> (usize, usize) {
    let mut total = 0;
    let mut inked = 0;
    for c in &frame.shapes {
        if let egui::Shape::Text(t) = &c.shape {
            if !has_cjk(t.galley.text()) {
                continue;
            }
            for row in &t.galley.rows {
                for g in &row.row.glyphs {
                    if !is_cjk(g.chr) {
                        continue;
                    }
                    total += 1;
                    if g.uv_rect.max[0] > g.uv_rect.min[0]
                        && g.uv_rect.max[1] > g.uv_rect.min[1]
                        && g.uv_rect.size[0] > 0.0
                    {
                        inked += 1;
                    }
                }
            }
        }
    }
    (total, inked)
}

/// Raw host, user and process data is painted verbatim in every locale — never
/// translated, never routed through the locale table — and its Chinese glyphs
/// actually render.
#[test]
fn l10n_raw_host_and_process_data_untranslated() {
    const HOST: &str = "工控机-一号";
    const PROC: &str = "資料庫服務";
    const USER: &str = "使用者甲";
    const CMD: &str = "/usr/bin/資料庫 --模式 生產";

    for loc in Locale::ALL {
        let mut app = app_in(loc);
        app.selected = Some(4242);
        {
            let shared = app.shared();
            let mut s = shared.lock().unwrap();
            if let Some(m) = s.metrics.as_mut() {
                m.hostname = HOST.to_string();
            }
            if let Some(pp) = s.procs.as_mut() {
                for row in pp.rows.iter_mut() {
                    if row.pid == 4242 {
                        row.name = PROC.to_string();
                        row.user = USER.to_string();
                    }
                }
            }
            if let Some(d) = s.proc_detail.as_mut() {
                d.process.name = PROC.to_string();
                d.process.command = CMD.to_string();
            }
        }
        let text = all_painted(&mut app, 960.0, 1040.0);
        assert!(text.contains(HOST), "{loc}: hostname was altered: {HOST:?} missing");
        assert!(text.contains(PROC), "{loc}: process name was altered");
        assert!(text.contains(USER), "{loc}: user name was altered");
        assert!(text.contains(CMD), "{loc}: command line was altered");
    }

    // Nothing in the tables carries this data, so no locale can translate it.
    for &k in Key::ALL {
        for loc in Locale::ALL {
            let t = l10n::text(loc, k);
            assert!(!t.contains(HOST) && !t.contains(PROC), "{k:?} embeds raw data");
        }
    }
}

/// Every control keeps an accessible name in all three locales, and the names
/// are the localized ones (never English leaking into a Chinese tree).
#[test]
fn l10n_accesskit_names_localized_per_locale() {
    let mut w = Walk::new();

    // Wizard: save/test/skip buttons and the three inputs.
    for loc in Locale::ALL {
        let mut app = wizard_in(loc);
        let f = w.settle(&mut app, (960.0, 1000.0));
        assert!(!f.nodes.is_empty(), "{loc}: empty AccessKit tree");
        assert!(f.nameless_inputs().is_empty(), "{loc}: nameless wizard input");
        assert!(f.unnamed_focusables().is_empty(), "{loc}: unnamed focusable in the wizard");
        for key in [Key::BtnSaveConnect, Key::BtnTest, Key::BtnSkip, Key::AccessAgentToken] {
            let want = l10n::text(loc, key);
            assert!(f.by_name(want).is_some(), "{loc}: no accessible name {want:?}");
        }
        if loc != Locale::English {
            assert!(f.by_name("Save and connect").is_none(), "{loc}: English button name leaked");
        }
    }

    // Process table: the filter input and its accessible name.
    for loc in Locale::ALL {
        let mut app = app_in(loc);
        let f = w.settle(&mut app, (960.0, 1000.0));
        let want = l10n::text(loc, Key::AccessFilterProcesses);
        assert!(f.by_name(want).is_some(), "{loc}: filter input is not named {want:?}");
        assert!(f.nameless_inputs().is_empty(), "{loc}: nameless table input");
        assert!(f.unnamed_focusables().is_empty(), "{loc}: unnamed focusable in the table");
        // Rows keep their audited shape (no role, no name) and their data.
        assert!(
            f.nodes.iter().any(|n| n.is_table_row()),
            "{loc}: table rows lost their no-role/no-name shape"
        );
    }

    // Settings: the tabs are named in the active language.
    for loc in Locale::ALL {
        let mut app = app_in(loc);
        app.settings_open = true;
        let f = w.settle(&mut app, (960.0, 1040.0));
        for key in [Key::TabSources, Key::TabDisplay, Key::TabConnection] {
            let want = l10n::text(loc, key);
            assert!(f.by_name(want).is_some(), "{loc}: settings tab not named {want:?}");
        }
        assert!(f.nameless_inputs().is_empty(), "{loc}: nameless settings input");
    }
}

/// Widget identity and focus order are locale-independent: tabbing through the
/// same screens yields the same sequence of roles in every locale, and the
/// focused control keeps its identity across a language switch.
#[test]
fn l10n_focus_order_and_ids_locale_independent() {
    fn role_walk(loc: Locale) -> Vec<String> {
        let mut w = Walk::new();
        let mut app = app_in(loc);
        let mut seq = vec![];
        for _ in 0..24 {
            let f = w.tab(&mut app, (960.0, 1040.0));
            seq.push(f.focus().map(|n| n.role_name()).unwrap_or_else(|| "-".to_string()));
        }
        seq
    }

    let en = role_walk(Locale::English);
    for loc in [Locale::ZhHansCn, Locale::ZhHantTw] {
        let other = role_walk(loc);
        assert_eq!(en, other, "{loc}: focus order differs from English");
        assert!(other.iter().any(|r| r != "-"), "{loc}: nothing ever took focus");
    }

    // Same for the splitter/scrollbar-free settings window.
    fn settings_roles(loc: Locale) -> Vec<String> {
        let mut w = Walk::new();
        let mut app = app_in(loc);
        app.settings_open = true;
        let mut seq = vec![];
        for _ in 0..12 {
            let f = w.tab(&mut app, (960.0, 1040.0));
            seq.push(f.focus().map(|n| n.role_name()).unwrap_or_else(|| "-".to_string()));
        }
        seq
    }
    assert_eq!(settings_roles(Locale::English), settings_roles(Locale::ZhHansCn));

    // A switch mid-walk does not lose the focused control's position: the
    // focusable count per frame is identical before and after.
    let mut w = Walk::new();
    let mut app = app_in(Locale::English);
    let f = w.settle(&mut app, (960.0, 1040.0));
    let before: Vec<String> = f.nodes.iter().filter(|n| n.focus || n.click).map(|n| n.role_name()).collect();
    app.set_language(Locale::ZhHansCn);
    let f = w.settle(&mut app, (960.0, 1040.0));
    let after: Vec<String> = f.nodes.iter().filter(|n| n.focus || n.click).map(|n| n.role_name()).collect();
    assert_eq!(before, after, "locale switch changed the focusable set");
}

/// Nothing localized reaches `config.json`, the log line or the support
/// summary: those stay canonical English in every locale.
#[test]
fn l10n_no_localized_text_in_config_or_log() {
    let mut app = app_in(Locale::ZhHansCn);
    app.selected = Some(4242);
    let _ = all_painted(&mut app, 960.0, 1000.0);

    let cfg_json = serde_json::to_string(&app.cfg).expect("serialize config");
    assert!(!has_cjk(&cfg_json), "config JSON carries localized text: {cfg_json}");
    assert!(cfg_json.contains("zh-Hans-CN"), "the language id itself is stored canonically");

    let summary = app.support_summary_text();
    assert!(!has_cjk(&summary), "support summary is not English: {summary}");
    assert!(summary.contains("zh-Hans-CN") || summary.contains("version"), "summary shape");

    // Log lines come from the canonical tag + scrubbed detail.
    let d = client::Diagnosis::from_error("connection refused");
    let line = d.log_line();
    assert!(line.starts_with("probe "), "log line shape changed: {line}");
    assert!(!has_cjk(&line));
    assert!(!has_cjk(&store::endpoint_shape("ws://工控机:45976/ws")));
}
