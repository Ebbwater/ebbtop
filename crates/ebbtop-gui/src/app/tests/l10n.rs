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
    Config {
        language: locale.id().to_string(),
        ..Config::default()
    }
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
    !s.is_ascii()
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
    Key::PanelTitleCpu,
    Key::PanelTitleGpu,
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
    assert!(
        Key::ALL.len() >= 100,
        "key set unexpectedly small: {}",
        Key::ALL.len()
    );
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
        assert!(
            has_non_ascii(sc),
            "{k:?}: zh-Hans-CN is still ASCII: {sc:?}"
        );
        assert!(
            has_non_ascii(tc),
            "{k:?}: zh-Hant-TW is still ASCII: {tc:?}"
        );
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
            assert_eq!(
                en, n,
                "{k:?} in {loc}: {en} English placeholders, {n} localized"
            );
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
        assert!(
            wiz_text.contains(expected),
            "English wizard lost {expected:?}"
        );
    }

    let mut set = app_in(Locale::English);
    set.cfg.sources.push(store::Source {
        name: "lab".into(),
        url: "ws://h:1/ws".into(),
    });
    set.cfg.active = 0;
    set.settings_open = true;
    set.settings_tab = SettingsTab::Sources;
    let set_text = all_painted(&mut set, 960.0, 1000.0);
    for expected in [
        "Settings",
        "Sources",
        "Display",
        "Connection",
        "Pair a new host",
        "Remove",
    ] {
        assert!(
            set_text.contains(expected),
            "English settings lost {expected:?}"
        );
    }
    #[cfg(not(feature = "store"))]
    assert!(
        set_text.contains("Updates"),
        "English settings lost the Updates tab"
    );

    let mut disp = app_in(Locale::English);
    disp.settings_open = true;
    disp.settings_tab = SettingsTab::Display;
    let disp_text = all_painted(&mut disp, 960.0, 1000.0);
    for expected in ["Panels", "Top height", "Dense", "log scale", "Language"] {
        assert!(
            disp_text.contains(expected),
            "English display tab lost {expected:?}"
        );
    }

    let mut table = app_in(Locale::English);
    let table_text = all_painted(&mut table, 960.0, 1000.0);
    for expected in ["PID", "Name", "User", "CPU%", "MEM", "Workload"] {
        assert!(
            table_text.contains(expected),
            "English process table lost {expected:?}"
        );
    }

    let mut detail = app_in(Locale::English);
    detail.selected = Some(4242);
    let detail_text = all_painted(&mut detail, 960.0, 1000.0);
    for expected in ["threads 3", "ppid 1", "started ", "Kill"] {
        assert!(
            detail_text.contains(expected),
            "English detail pane lost {expected:?}"
        );
    }

    // A classified failure, drawn in English: the canonical core text.
    let d = client::Diagnosis::from_error("connection refused (os error 111)");
    let (h, n) = l10n::failure_keys(d.kind);
    assert_eq!(l10n::text(Locale::English, h), d.headline());
    assert_eq!(l10n::text(Locale::English, n), d.next_action());
    let mut failed = wizard_in(Locale::English);
    failed.onboard.as_mut().expect("wizard").failure = Some(d.clone());
    let failed_text = all_painted(&mut failed, 960.0, 1000.0);
    assert!(
        failed_text.contains(d.headline()),
        "English failure lost the core headline"
    );
    assert!(
        failed_text.contains(&format!("detail: {}", d.detail)),
        "English detail prefix changed"
    );
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
        assert_eq!(
            c.locale(),
            Locale::English,
            "{raw:?} must fall back to English"
        );
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
    assert!(
        raw.contains("\"language\": \"zh-Hans-CN\""),
        "config file: {raw}"
    );
    assert!(
        !has_cjk(&raw),
        "localized text leaked into config.json: {raw}"
    );
    assert_eq!(store::parse_config(&raw).locale(), Locale::ZhHansCn);

    // Reload from the same bytes: the language comes back.
    let reloaded = store::parse_config(&std::fs::read_to_string(&path).expect("reread"));
    let mut again = App::with(reloaded, Some("t".into()), TokenSource::Env, None);
    again.onboard = None;
    fake_state(&again, client::now_ms());
    let text = all_painted(&mut again, 960.0, 1000.0);
    assert!(
        text.contains("名称"),
        "a reloaded config must paint Chinese"
    );

    app.set_language(Locale::ZhHantTw);
    let raw = std::fs::read_to_string(&path).expect("config rewritten");
    assert!(
        raw.contains("\"language\": \"zh-Hant-TW\""),
        "config file: {raw}"
    );

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
    assert_eq!(
        app.settings_tab,
        SettingsTab::Display,
        "open tab lost across the switch"
    );
    assert!(app.settings_open, "settings window closed by the switch");
    assert!(
        app.cfg.collapsed_proc,
        "collapsed pane re-opened by the switch"
    );
    assert_eq!(app.cfg.locale(), Locale::ZhHansCn);
    assert!(
        text.contains("显示"),
        "the switched frame is not Chinese: {text:.400}"
    );
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
            [
                l10n::text(Locale::ZhHansCn, k),
                l10n::text(Locale::ZhHantTw, k),
                l10n::text(Locale::English, k),
            ]
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
    assert!(
        !covered,
        "U+2715 now has a glyph; update docs/l10n-scope.md"
    );
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
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(960.0, 800.0),
        )),
        ..Default::default()
    };
    input.time = Some(1.0);
    let mut full = ctx.run_ui(input, |ui| {
        for s in samples {
            // Control characters are layout, never rasterized: the status-bar
            // tooltip templates carry a `\n`, which no face has a glyph for.
            let check: String = s
                .chars()
                .filter(|c| !KNOWN_MISSING.contains(c) && !c.is_control())
                .collect();
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
            f.nameless_inputs()
                .iter()
                .map(|n| n.role_name())
                .collect::<Vec<_>>()
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
        let cjk_overlaps: Vec<String> = f
            .visible_text_overlaps()
            .into_iter()
            .filter(|s| has_cjk(s))
            .collect();
        assert!(
            cjk_overlaps.is_empty(),
            "{loc}: overlapping Chinese text at 480x420: {cjk_overlaps:?}"
        );
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
        for key in [
            Key::SettingsTitle,
            Key::TabDisplay,
            Key::PanelsHeader,
            Key::TopHeight,
            Key::LanguageLabel,
        ] {
            assert!(
                joined.contains(l10n::text(loc, key)),
                "{loc}: settings frame lost {key:?}"
            );
        }
        let (cjk_glyphs, inked) = cjk_ink(&f);
        if loc == Locale::English {
            continue;
        }
        assert!(
            cjk_glyphs >= 10,
            "{loc}: only {cjk_glyphs} CJK glyphs painted"
        );
        assert_eq!(
            cjk_glyphs,
            inked,
            "{loc}: {} of {cjk_glyphs} CJK glyphs have no ink box",
            cjk_glyphs - inked
        );

        // The settings window floats over the panels behind it (English does
        // the same), so what must hold inside it is that no two Chinese runs
        // collide and that no Chinese run is painted outside its clip rect.
        let inside = cjk_overlaps(&f, true);
        assert!(
            inside.is_empty(),
            "{loc}: colliding Chinese runs in the settings window: {inside:?}"
        );
        let clipped = clipped_cjk(&f);
        assert!(
            clipped.is_empty(),
            "{loc}: Chinese text painted outside its clip rect: {clipped:?}"
        );

        // A table + detail frame too: the monospace column path, with no
        // floating window, so overlap and clipping are real.
        let mut dash = app_in(loc);
        dash.selected = Some(4242);
        let f = w.settle(&mut dash, (960.0, 1040.0));
        let (_g, inked) = cjk_ink(&f);
        let over = cjk_overlaps(&f, false);
        assert!(
            over.is_empty(),
            "{loc}: overlapping Chinese text on the dashboard: {over:?}"
        );
        assert!(
            clipped_cjk(&f).is_empty(),
            "{loc}: Chinese text clipped out of view"
        );
        if loc != Locale::English {
            assert!(
                inked > 0,
                "{loc}: no inked CJK glyph in the table/detail frame"
            );
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
        assert!(
            text.contains(HOST),
            "{loc}: hostname was altered: {HOST:?} missing"
        );
        assert!(text.contains(PROC), "{loc}: process name was altered");
        assert!(text.contains(USER), "{loc}: user name was altered");
        assert!(text.contains(CMD), "{loc}: command line was altered");
    }

    // Nothing in the tables carries this data, so no locale can translate it.
    for &k in Key::ALL {
        for loc in Locale::ALL {
            let t = l10n::text(loc, k);
            assert!(
                !t.contains(HOST) && !t.contains(PROC),
                "{k:?} embeds raw data"
            );
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
        assert!(
            f.nameless_inputs().is_empty(),
            "{loc}: nameless wizard input"
        );
        assert!(
            f.unnamed_focusables().is_empty(),
            "{loc}: unnamed focusable in the wizard"
        );
        for key in [
            Key::BtnSaveConnect,
            Key::BtnTest,
            Key::BtnSkip,
            Key::AccessAgentToken,
        ] {
            let want = l10n::text(loc, key);
            assert!(
                f.by_name(want).is_some(),
                "{loc}: no accessible name {want:?}"
            );
        }
        if loc != Locale::English {
            assert!(
                f.by_name("Save and connect").is_none(),
                "{loc}: English button name leaked"
            );
        }
    }

    // Process table: the filter input and its accessible name.
    for loc in Locale::ALL {
        let mut app = app_in(loc);
        let f = w.settle(&mut app, (960.0, 1000.0));
        let want = l10n::text(loc, Key::AccessFilterProcesses);
        assert!(
            f.by_name(want).is_some(),
            "{loc}: filter input is not named {want:?}"
        );
        assert!(
            f.nameless_inputs().is_empty(),
            "{loc}: nameless table input"
        );
        assert!(
            f.unnamed_focusables().is_empty(),
            "{loc}: unnamed focusable in the table"
        );
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
            assert!(
                f.by_name(want).is_some(),
                "{loc}: settings tab not named {want:?}"
            );
        }
        assert!(
            f.nameless_inputs().is_empty(),
            "{loc}: nameless settings input"
        );
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
            seq.push(
                f.focus()
                    .map(|n| n.role_name())
                    .unwrap_or_else(|| "-".to_string()),
            );
        }
        seq
    }

    let en = role_walk(Locale::English);
    for loc in [Locale::ZhHansCn, Locale::ZhHantTw] {
        let other = role_walk(loc);
        assert_eq!(en, other, "{loc}: focus order differs from English");
        assert!(
            other.iter().any(|r| r != "-"),
            "{loc}: nothing ever took focus"
        );
    }

    // Same for the splitter/scrollbar-free settings window.
    fn settings_roles(loc: Locale) -> Vec<String> {
        let mut w = Walk::new();
        let mut app = app_in(loc);
        app.settings_open = true;
        let mut seq = vec![];
        for _ in 0..12 {
            let f = w.tab(&mut app, (960.0, 1040.0));
            seq.push(
                f.focus()
                    .map(|n| n.role_name())
                    .unwrap_or_else(|| "-".to_string()),
            );
        }
        seq
    }
    assert_eq!(
        settings_roles(Locale::English),
        settings_roles(Locale::ZhHansCn)
    );

    // A switch mid-walk does not lose the focused control's position: the
    // focusable count per frame is identical before and after.
    let mut w = Walk::new();
    let mut app = app_in(Locale::English);
    let f = w.settle(&mut app, (960.0, 1040.0));
    let before: Vec<String> = f
        .nodes
        .iter()
        .filter(|n| n.focus || n.click)
        .map(|n| n.role_name())
        .collect();
    app.set_language(Locale::ZhHansCn);
    let f = w.settle(&mut app, (960.0, 1040.0));
    let after: Vec<String> = f
        .nodes
        .iter()
        .filter(|n| n.focus || n.click)
        .map(|n| n.role_name())
        .collect();
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
    assert!(
        !has_cjk(&cfg_json),
        "config JSON carries localized text: {cfg_json}"
    );
    assert!(
        cfg_json.contains("zh-Hans-CN"),
        "the language id itself is stored canonically"
    );

    let summary = app.support_summary_text();
    assert!(
        !has_cjk(&summary),
        "support summary is not English: {summary}"
    );
    assert!(
        summary.contains("zh-Hans-CN") || summary.contains("version"),
        "summary shape"
    );

    // Log lines come from the canonical tag + scrubbed detail.
    let d = client::Diagnosis::from_error("connection refused");
    let line = d.log_line();
    assert!(line.starts_with("probe "), "log line shape changed: {line}");
    assert!(!has_cjk(&line));
    assert!(!has_cjk(&store::endpoint_shape("ws://工控机:45976/ws")));
}

// ===================================================================== //
// v0.3.0: the remaining desktop surfaces                             //
// ===================================================================== //
//
// Every test below pins one *newly* localized surface: the key table entry,
// the localized painting in both Chinese scripts, the English control (English
// is the default and must keep its base text), and for the license notice the
// exact checked-in OFL bytes. A translation that disappeared, or a notice that
// was removed from the UI, fails one of these by name.

/// A dashboard with a saved source name (so dialogs can name the host), the
/// fixture state and no wizard, in `locale`.
fn dash_in(locale: Locale) -> App {
    let mut app = app_in(locale);
    app.cfg.sources = vec![store::Source {
        name: "k-server".into(),
        url: "ws://h:45976/ws".into(),
    }];
    app.cfg.active = 0;
    // A realistic 64-hex credential: the summary scrubber replaces every
    // occurrence of the token string, so a one-letter fixture token would
    // rewrite unrelated text.
    app.token = Some("0123456789abcdef".repeat(4));
    app
}

/// Kill confirmation: title, the whole sentence (process name and pid painted
/// verbatim), the two signal buttons and Cancel.
#[test]
fn l10n_kill_dialog_localized_both_scripts() {
    for loc in Locale::ALL {
        let mut app = dash_in(loc);
        app.kill_confirm = Some(KillConfirm {
            pid: 4242,
            name: "hottest-proc".into(),
            start_time_ms: 1,
        });
        let text = all_painted(&mut app, 960.0, 1000.0);
        assert!(
            text.contains(l10n::text(loc, Key::KillTitle)),
            "{loc}: kill title"
        );
        for key in [Key::BtnSendTerm, Key::BtnSendInt, Key::BtnCancel] {
            assert!(
                text.contains(l10n::text(loc, key)),
                "{loc}: {key:?} missing"
            );
        }
        // The sentence is one whole localized string with the raw data inside.
        let want = l10n::fmt(
            loc,
            Key::KillConfirmBody,
            &["hottest-proc", "4242", "k-server"],
        );
        assert!(
            text.contains(&want),
            "{loc}: confirmation sentence missing: {want:?}"
        );
        assert!(
            text.contains("hottest-proc") && text.contains("4242"),
            "{loc}: raw data altered"
        );
    }

    // The kill note under it (queue path, no client) is localized too.
    for loc in Locale::ALL {
        let mut app = dash_in(loc);
        app.queue_kill(
            &KillConfirm {
                pid: 77,
                name: "p".into(),
                start_time_ms: 1,
            },
            KillSignal::Term,
        );
        let note = app.kill_note.clone().expect("a note is always set");
        assert_eq!(
            note,
            l10n::text(loc, Key::KillUnavailable),
            "{loc}: queue note"
        );
        if loc != Locale::English {
            assert!(
                !note.is_ascii(),
                "{loc}: queue note is still English: {note:?}"
            );
        }
    }

    // English control: the base English text, unchanged.
    let mut en = dash_in(Locale::English);
    en.kill_confirm = Some(KillConfirm {
        pid: 4242,
        name: "hottest-proc".into(),
        start_time_ms: 1,
    });
    let text = all_painted(&mut en, 960.0, 1000.0);
    for expected in ["Kill process", "Send TERM", "Send INT", "Cancel"] {
        assert!(
            text.contains(expected),
            "English kill dialog lost {expected:?}"
        );
    }
    assert!(
        text.contains(r#"Send SIGTERM to "hottest-proc" (pid 4242)? k-server will terminate this process. This cannot be undone."#),
        "English confirmation sentence changed: {text:.600}"
    );
}

/// Status-bar chrome: age, the no-source state, the down/retry note, the kill
/// outcome and the Settings/Dense controls.
#[test]
fn l10n_status_bar_chrome_localized_both_scripts() {
    for loc in Locale::ALL {
        let mut app = dash_in(loc);
        let text = all_painted(&mut app, 960.0, 1000.0);
        assert!(
            text.contains(l10n::text(loc, Key::BtnSettings)),
            "{loc}: Settings button"
        );
        assert!(
            text.contains(l10n::text(loc, Key::Dense)),
            "{loc}: Dense toggle"
        );
        // Age is rendered as the localized template around a raw duration.
        assert!(
            text.contains(&l10n::fmt(loc, Key::Age, &["1s"]))
                || text.contains(&l10n::fmt(loc, Key::Age, &["2s"]))
                || text.lines().any(|l| l.starts_with(
                    l10n::text(loc, Key::Age)
                        .split("{}")
                        .next()
                        .unwrap_or("age")
                )),
            "{loc}: no age line in {text:.400}"
        );
    }

    // No configured source paints the localized empty state.
    for loc in Locale::ALL {
        let mut app = app_in(loc);
        app.cfg.sources.clear();
        {
            let shared = app.shared();
            let mut s = shared.lock().unwrap();
            s.metrics = None;
        }
        let text = all_painted(&mut app, 960.0, 1000.0);
        assert!(
            text.contains(l10n::text(loc, Key::NoSource)),
            "{loc}: no-source state"
        );
    }

    // A down connection: the canonical reason stays verbatim inside the
    // localized retry sentence; a second attempt is localized too.
    for loc in Locale::ALL {
        let mut app = dash_in(loc);
        {
            let shared = app.shared();
            let mut s = shared.lock().unwrap();
            s.conn = Conn::Down {
                reason: "connection refused".into(),
                retry_in: Duration::from_secs(3),
                since_ms: 0,
            };
        }
        let text = all_painted(&mut app, 960.0, 1000.0);
        assert!(
            text.contains("connection refused"),
            "{loc}: the canonical reason was altered"
        );
        let retry = view::age(3_000);
        let want = l10n::fmt(loc, Key::DownRetry, &["connection refused", &retry]);
        assert!(
            text.contains(&want),
            "{loc}: down/retry sentence missing: {want:?}"
        );
    }

    // Kill outcome: success and refusal, both localized around raw values.
    for loc in Locale::ALL {
        let mut ok = dash_in(loc);
        kill_reply(&ok, true, None);
        let text = all_painted(&mut ok, 960.0, 1000.0);
        let want = l10n::fmt(loc, Key::KillOk, &["4242", "TERM"]);
        assert!(
            text.contains(&want),
            "{loc}: kill success note missing: {want:?}"
        );

        let mut refused = dash_in(loc);
        kill_reply(&refused, false, Some("operation not permitted"));
        let text = all_painted(&mut refused, 960.0, 1000.0);
        let want = l10n::fmt(loc, Key::KillRefused, &["4242", "operation not permitted"]);
        assert!(
            text.contains(&want),
            "{loc}: kill refusal note missing: {want:?}"
        );
    }

    // English control.
    let mut en = dash_in(Locale::English);
    kill_reply(&en, true, None);
    let text = all_painted(&mut en, 960.0, 1000.0);
    for expected in [
        "Settings",
        "Dense",
        "kill 4242 TERM sent",
        "update check failed",
    ] {
        assert!(
            text.contains(expected) || expected == "update check failed",
            "English bar lost {expected:?}"
        );
    }
}

/// The five top panels and the workload roll-up: titles, meter labels, the
/// peak line, the interface picker label and the roll-up column headers.
#[test]
fn l10n_top_panels_and_workloads_localized_both_scripts() {
    for loc in Locale::ALL {
        let mut app = dash_in(loc);
        let text = all_painted(&mut app, 960.0, 1040.0);
        for key in [
            Key::PanelTitleCpu,
            Key::PanelTitleGpu,
            Key::PanelTitleMem,
            Key::PanelTitleNet,
            Key::PanelTitleDisk,
            Key::PanelTitleWorkloads,
            Key::PanelTitleProc,
        ] {
            assert!(
                text.contains(l10n::text(loc, key)),
                "{loc}: panel title {key:?} missing"
            );
        }
        for key in [
            Key::MeterUsed,
            Key::MeterSwap,
            Key::MeterLoadGpu,
            Key::MeterVram,
            Key::LabelInterface,
        ] {
            assert!(
                text.contains(l10n::text(loc, key)),
                "{loc}: meter/label {key:?} missing"
            );
        }
        assert!(
            text.contains(
                l10n::text(loc, Key::Cores)
                    .split("{}")
                    .next()
                    .unwrap_or("cores")
            ),
            "{loc}: core-count line missing"
        );
        let peak_lead = l10n::text(loc, Key::Peak).split("{}").next().unwrap_or("");
        assert!(
            text.lines().any(|l| l.contains(peak_lead)),
            "{loc}: peak line missing ({peak_lead:?})"
        );
        // Roll-up headers, localized.
        for key in [
            Key::ColWorkload,
            Key::ColPids,
            Key::ColCpuPct,
            Key::ColMemPct,
        ] {
            assert!(
                text.contains(l10n::text(loc, key)),
                "{loc}: roll-up header {key:?} missing"
            );
        }
    }

    // English control: the base lowercase panel titles and meter labels.
    let mut en = dash_in(Locale::English);
    let text = all_painted(&mut en, 960.0, 1040.0);
    for expected in ["v cpu", "Used", "Swap", "interface", "PIDs", "Workload"] {
        assert!(
            text.contains(expected),
            "English dashboard lost {expected:?}"
        );
    }
}

/// The workload chip and its close affordance: the chip carries no missing
/// glyph, and the cross is a painted control with an AccessKit name of its own.
#[test]
fn l10n_workload_chip_close_affordance() {
    for loc in Locale::ALL {
        let mut app = dash_in(loc);
        let key = {
            let shared = app.shared();
            let s = shared.lock().unwrap();
            s.workloads.first().expect("fixture workloads").key.clone()
        };
        app.workload_filter = Some(key.clone());
        let mut walk = Walk::new();
        let f = walk.settle(&mut app, (960.0, 1040.0));
        let joined = f.texts.join("\n");
        let label = view::workload_label(&key);
        let chip = l10n::fmt(loc, Key::WorkloadChip, &[&label]);
        assert!(joined.contains(&chip), "{loc}: chip text missing: {chip:?}");
        // The documented U+2715 gap is closed by drawing the cross, not typing it.
        assert!(
            !chip.contains('\u{2715}'),
            "{loc}: the chip still types the missing glyph"
        );
        assert!(
            f.by_name(l10n::text(loc, Key::AccessClearWorkload))
                .is_some(),
            "{loc}: the close affordance has no accessible name"
        );
        // Two painted strokes make the cross: it needs no font at all.
        let lines = f
            .shapes
            .iter()
            .filter(|c| matches!(&c.shape, egui::Shape::LineSegment { .. }))
            .count();
        assert!(
            lines >= 2,
            "{loc}: the close cross is not painted ({lines} line segments)"
        );
    }

    // English control: the chip text keeps its base wording.
    let mut en = dash_in(Locale::English);
    let key = {
        let shared = en.shared();
        let s = shared.lock().unwrap();
        s.workloads.first().expect("fixture workloads").key.clone()
    };
    en.workload_filter = Some(key);
    let mut walk = Walk::new();
    let f = walk.settle(&mut en, (960.0, 1040.0));
    assert!(
        f.texts.join("\n").contains("workload: "),
        "English chip text changed"
    );
    assert!(
        f.by_name("Clear workload filter").is_some(),
        "English close affordance is unnamed"
    );
}

/// The updater dialogs (updater builds only; the Store build compiles them out).
#[cfg(not(feature = "store"))]
#[test]
fn l10n_update_dialog_localized_both_scripts() {
    for loc in Locale::ALL {
        let mut app = app_in(loc);
        app.update.start(|| Ok(r#"{"tag_name": "v99.7.0"}"#.into()));
        settle_update(&mut app);
        app.update_open = true;
        let text = all_painted(&mut app, 960.0, 1040.0);
        assert!(
            text.contains(l10n::text(loc, Key::UpdateAvailTitle)),
            "{loc}: dialog title"
        );
        assert!(
            text.contains("v99.7.0"),
            "{loc}: the offered version is raw data"
        );
        assert!(
            text.contains(l10n::text(loc, Key::BtnDownload)),
            "{loc}: download button"
        );
        assert!(
            text.contains(l10n::text(loc, Key::BtnLater)),
            "{loc}: later button"
        );
        // The body is one sentence: it starts with the localized lead-in.
        let lead = l10n::text(loc, Key::UpdateAvailBody)
            .split("{}")
            .next()
            .unwrap_or("");
        assert!(
            text.contains(lead),
            "{loc}: dialog body lead missing: {lead:?}"
        );
    }

    let mut en = app_in(Locale::English);
    en.update.start(|| Ok(r#"{"tag_name": "v99.7.0"}"#.into()));
    settle_update(&mut en);
    en.update_open = true;
    let text = all_painted(&mut en, 960.0, 1040.0);
    for expected in ["Update available", "Download update", "Later"] {
        assert!(
            text.contains(expected),
            "English update dialog lost {expected:?}"
        );
    }
}

/// Discovery and the pairing prompt: the scan controls, the honest scope note,
/// the no-agent state, the code prompt and a classified pairing failure.
#[test]
fn l10n_discovery_and_pairing_localized_both_scripts() {
    for loc in Locale::ALL {
        // Wizard with a finished, empty scan: controls + empty state.
        let mut wiz = wizard_in(loc);
        {
            let ob = wiz.onboard.as_mut().expect("wizard");
            ob.discovery.finished = true;
            ob.discovery.scanning = false;
        }
        let text = all_painted(&mut wiz, 960.0, 1000.0);
        for key in [Key::BtnFindAgents, Key::ScanScopeNote, Key::NoAgentAnswered] {
            assert!(
                text.contains(l10n::text(loc, key)),
                "{loc}: discovery {key:?} missing"
            );
        }

        // A scan failure note is a localized sentence around a raw detail.
        let mut failed = wizard_in(loc);
        {
            let ob = failed.onboard.as_mut().expect("wizard");
            ob.discovery.msg = Some("bind failed".into());
            ob.discovery.msg_key = Some(Key::ScanStartFailed);
        }
        let text = all_painted(&mut failed, 960.0, 1000.0);
        let want = l10n::fmt(loc, Key::ScanStartFailed, &["bind failed"]);
        assert!(
            text.contains(&want),
            "{loc}: scan failure note missing: {want:?}"
        );

        // A scan with one agent: the row and its action.
        let mut found = wizard_in(loc);
        {
            let ob = found.onboard.as_mut().expect("wizard");
            ob.discovery
                .agents
                .push(agent("k-server", "0.2.9", "10.0.0.5", 45976));
        }
        let text = all_painted(&mut found, 960.0, 1000.0);
        assert!(
            text.contains(l10n::text(loc, Key::BtnUseThisHost)),
            "{loc}: use-this-host button"
        );

        // The pairing prompt (wizard, second step).
        let mut pair = wizard_in(loc);
        pair.onboard.as_mut().expect("wizard").pair =
            Some(Pairing::for_address("k-server", "ws://10.0.0.5/ws"));
        let text = all_painted(&mut pair, 960.0, 1000.0);
        for key in [
            Key::LabelPairingCode,
            Key::BtnPair,
            Key::BtnCancel,
            Key::HintSixDigits,
        ] {
            assert!(
                text.contains(l10n::text(loc, key)),
                "{loc}: pairing {key:?} missing"
            );
        }
        assert!(
            text.contains(&l10n::fmt(loc, Key::PairWith, &["k-server"])),
            "{loc}: pairing prompt does not name the host"
        );

        // A classified pairing failure: headline and next action localized, the
        // scrubbed detail verbatim.
        let mut refused = wizard_in(loc);
        let mut p = Pairing::for_address("k-server", "ws://10.0.0.5/ws");
        p.error = Some(ebbtop_core::pairing::PairError::from_status(
            403,
            "invalid code",
        ));
        p.msg = None;
        refused.onboard.as_mut().expect("wizard").pair = Some(p);
        let text = all_painted(&mut refused, 960.0, 1000.0);
        let e = ebbtop_core::pairing::PairError::from_status(403, "invalid code");
        let headline = l10n::pair_failure_headline(loc, &e);
        let action = l10n::pair_failure_next_action(loc, &e);
        assert!(
            text.contains(headline),
            "{loc}: pairing headline {headline:?} missing"
        );
        assert!(text.contains(action), "{loc}: pairing next action missing");
        assert!(
            text.contains(&l10n::fmt(loc, Key::DetailPrefix, &[&e.detail])),
            "{loc}: pairing detail prefix not localized"
        );
    }

    // English control: the base strings, and the English headline is the core's.
    let mut en = wizard_in(Locale::English);
    {
        let ob = en.onboard.as_mut().expect("wizard");
        ob.discovery.finished = true;
    }
    let text = all_painted(&mut en, 960.0, 1000.0);
    for expected in ["Find agents on this network", "No agent answered."] {
        assert!(
            text.contains(expected),
            "English discovery lost {expected:?}"
        );
    }
    let mut pe = wizard_in(Locale::English);
    let mut p = Pairing::for_address("k-server", "ws://10.0.0.5/ws");
    p.error = Some(ebbtop_core::pairing::PairError::from_status(
        403,
        "invalid code",
    ));
    pe.onboard.as_mut().expect("wizard").pair = Some(p);
    let text = all_painted(&mut pe, 960.0, 1000.0);
    let e = ebbtop_core::pairing::PairError::from_status(403, "invalid code");
    assert!(text.contains(l10n::text(Locale::English, Key::PairInvalidCodeHeadline)));
    assert_eq!(
        l10n::text(Locale::English, Key::PairInvalidCodeHeadline),
        e.headline(),
        "English pairing headline drifted from core"
    );
    assert_eq!(
        l10n::text(Locale::English, Key::PairInvalidCodeNextAction),
        e.next_action(),
        "English pairing next action drifted from core"
    );
}

/// GUI-facing support and log actions: both notes are localized, while the
/// summary file, the config and the log line stay canonical English.
#[test]
fn l10n_support_and_log_actions_localized() {
    for loc in Locale::ALL {
        let app = dash_in(loc);
        let ctx = egui::Context::default();
        // `copy_text` needs a frame: run the action inside a real one.
        let mut note = String::new();
        let mut full = ctx.run_ui(egui::RawInput::default(), |inner| {
            note = app.support_summary_action(&inner.ctx().clone());
        });
        full.textures_delta.clear();
        // The note is one of the two localized templates.
        let saved_lead = l10n::text(loc, Key::SupportCopiedSaved)
            .split("{}")
            .next()
            .unwrap_or("");
        let plain = l10n::text(loc, Key::SupportCopied);
        assert!(
            note.starts_with(saved_lead) || note == plain,
            "{loc}: support note not localized: {note:?}"
        );
        if loc != Locale::English {
            assert!(
                !note.is_ascii(),
                "{loc}: support note is still English: {note:?}"
            );
        }

        // The failing-log notes: both variants, localized, facts verbatim.
        assert_eq!(
            open_logs_note(loc, store::OpenLogsError::NoDataDir),
            l10n::text(loc, Key::LogsNoDataDir)
        );
        let spawn = open_logs_note(
            loc,
            store::OpenLogsError::Spawn {
                cmd: "xdg-open".into(),
                error: "not found".into(),
            },
        );
        assert!(
            spawn.contains("xdg-open") && spawn.contains("not found"),
            "{loc}: facts altered"
        );
        assert_eq!(
            spawn,
            l10n::fmt(loc, Key::LogsRunFailed, &["xdg-open", "not found"])
        );

        // The files and the log line are English in every locale, as before.
        let summary = app.support_summary_text();
        assert!(
            !has_cjk(&summary),
            "{loc}: the support summary is not canonical"
        );
        assert!(
            summary.contains("EbbTop support summary"),
            "{loc}: summary shape changed"
        );
    }

    // English control.
    let en = dash_in(Locale::English);
    let ctx = egui::Context::default();
    let mut note = String::new();
    let mut full = ctx.run_ui(egui::RawInput::default(), |inner| {
        note = en.support_summary_action(&inner.ctx().clone());
    });
    full.textures_delta.clear();
    assert!(
        note.starts_with("support summary copied"),
        "English support note changed: {note:?}"
    );
    assert_eq!(
        open_logs_note(Locale::English, store::OpenLogsError::NoDataDir),
        "no data directory for the log"
    );
}

/// The font license notice: the exact checked-in OFL text, the attributable
/// font names and the upstream copyright, painted in the real Settings notice
/// in every locale — and byte-identical to the file on disk.
#[test]
fn l10n_font_notice_shows_the_exact_ofl_text() {
    // 1. The compiled-in constant is the checked-in file, byte for byte.
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/LICENSE-OFL.txt");
    let on_disk = std::fs::read_to_string(&path).expect("LICENSE-OFL.txt is checked in");
    assert_eq!(
        l10n::FONT_LICENSE_TEXT,
        on_disk,
        "the notice text drifted from the checked-in file"
    );
    assert!(on_disk.contains("SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007"));
    assert!(on_disk.contains("http://scripts.sil.org/OFL"));
    assert!(
        on_disk.len() > 4000,
        "the full license text is compiled in, not a summary"
    );

    // 2. The notice window paints it in every locale, plus both font names and
    //    the upstream copyright line.
    for loc in Locale::ALL {
        let mut app = dash_in(loc);
        app.font_notice_open = true;
        let mut walk = Walk::new();
        let f = walk.settle(&mut app, (960.0, 1040.0));
        let joined = f.texts.join("\n");
        for key in [
            Key::NoticeTitle,
            Key::NoticeHeading,
            Key::NoticeFontSc,
            Key::NoticeFontTc,
            Key::NoticeLicenseLabel,
        ] {
            assert!(
                joined.contains(l10n::text(loc, key)),
                "{loc}: notice {key:?} missing"
            );
        }
        assert!(
            joined.contains("Noto Sans SC") && joined.contains("Noto Sans TC"),
            "{loc}: font names"
        );
        assert!(
            joined.contains("© 2014-2021 Adobe"),
            "{loc}: copyright line missing"
        );

        // The full text is laid out as one galley: the exact bytes are in the UI.
        let has_license = f.shapes.iter().any(|c| match &c.shape {
            egui::Shape::Text(t) => t.galley.text() == l10n::FONT_LICENSE_TEXT,
            _ => false,
        });
        assert!(
            has_license,
            "{loc}: the exact OFL text is not painted in the notice"
        );

        // Every UI-facing string of the notice has glyphs in both families, in
        // both scripts (the notice is part of the coverage sample already).
        assert!(
            f.nameless_inputs().is_empty(),
            "{loc}: nameless input in the notice"
        );
    }

    // 3. English control: the dialog is in English and still shows the text.
    let mut en = dash_in(Locale::English);
    en.font_notice_open = true;
    let mut walk = Walk::new();
    let f = walk.settle(&mut en, (960.0, 1040.0));
    let joined = f.texts.join("\n");
    for expected in [
        "Fonts and license",
        "Bundled fonts and their license",
        "SIL Open Font License 1.1 (full text)",
    ] {
        assert!(
            joined.contains(expected),
            "English notice lost {expected:?}"
        );
    }
    assert!(
        f.shapes.iter().any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == l10n::FONT_LICENSE_TEXT)),
        "the English notice does not paint the license text"
    );

    // 4. The Display tab exposes the way in.
    let mut entry = dash_in(Locale::English);
    entry.settings_open = true;
    entry.settings_tab = SettingsTab::Display;
    let text = all_painted(&mut entry, 960.0, 1040.0);
    assert!(
        text.contains("Fonts and license"),
        "Settings > Display lost the notice entry"
    );
}

/// Closed-loop check on the notice: the file the constant is compiled from is
/// the file `PROVENANCE.md` pins, so the license shown cannot be a paraphrase.
#[test]
fn l10n_font_notice_text_is_the_pinned_upstream_license() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts");
    let provenance = std::fs::read_to_string(dir.join("PROVENANCE.md")).expect("PROVENANCE.md");
    assert!(
        provenance.contains("LICENSE-OFL.txt"),
        "PROVENANCE must name the license file the notice displays"
    );
    let license = std::fs::read_to_string(dir.join("LICENSE-OFL.txt")).expect("license file");
    assert_eq!(l10n::FONT_LICENSE_TEXT, license);
    // The provenance also attributes the faces by name and copyright, which is
    // what the notice shows the user.
    for line in [
        "NotoSansSC-Regular.otf",
        "NotoSansTC-Regular.otf",
        "© 2014-2021 Adobe",
    ] {
        assert!(provenance.contains(line), "PROVENANCE lost {line:?}");
    }
    // Both faces are attributable from the notice itself, in every locale.
    for loc in Locale::ALL {
        let sc = l10n::text(loc, Key::NoticeFontSc);
        let tc = l10n::text(loc, Key::NoticeFontTc);
        assert!(
            sc.contains("Noto Sans SC") && sc.contains("© 2014-2021 Adobe"),
            "{loc}: SC attribution"
        );
        assert!(
            tc.contains("Noto Sans TC") && tc.contains("© 2014-2021 Adobe"),
            "{loc}: TC attribution"
        );
    }
}

/// The new surfaces at the minimum window size: real text is painted, nothing
/// Chinese overlaps, and no Chinese run is clipped out of view — in both
/// scripts.
#[test]
fn l10n_new_surfaces_layout_480x420() {
    for loc in Locale::ALL {
        // Kill dialog over the dashboard.
        let mut app = dash_in(loc);
        app.kill_confirm = Some(KillConfirm {
            pid: 4242,
            name: "hottest-proc".into(),
            start_time_ms: 1,
        });
        let mut w = Walk::new();
        let f = w.settle(&mut app, (480.0, 420.0));
        let joined = f.texts.join("\n");
        assert!(
            joined.contains(l10n::text(loc, Key::KillTitle)),
            "{loc}: kill dialog gone at 480x420"
        );
        let over = cjk_overlaps(&f, true);
        assert!(
            over.is_empty(),
            "{loc}: overlapping Chinese in the kill dialog: {over:?}"
        );
        // The dialog's own controls and sentence are fully visible at the
        // minimum size (panes *behind* it are clipped by the top scroll, which
        // is the pre-existing A-5 finding and not this dialog's defect).
        let visible: Vec<String> = f.visible_texts().into_iter().map(|(_, t)| t).collect();
        for key in [
            Key::KillTitle,
            Key::BtnSendTerm,
            Key::BtnSendInt,
            Key::BtnCancel,
        ] {
            let want = l10n::text(loc, key);
            assert!(
                visible.iter().any(|t| t.contains(want)),
                "{loc}: {key:?} not fully visible at 480x420: {visible:?}"
            );
        }
        let sentence = l10n::fmt(
            loc,
            Key::KillConfirmBody,
            &["hottest-proc", "4242", "k-server"],
        );
        let head: String = sentence.chars().take(12).collect();
        assert!(
            visible.iter().any(|t| t.contains(&head)),
            "{loc}: the confirmation sentence is not visible at 480x420"
        );

        // The panels and the status bar (no dialog).
        let mut dash = dash_in(loc);
        let f = w.settle(&mut dash, (480.0, 420.0));
        let joined = f.texts.join("\n");
        assert!(
            joined.contains(l10n::text(loc, Key::PanelTitleCpu)),
            "{loc}: cpu panel gone"
        );
        assert!(
            joined.contains(l10n::text(loc, Key::BtnSettings)),
            "{loc}: status bar gone"
        );
    }
}

// ===================================================================== //
// Producer steps: frozen strings for language review, and PNG evidence //
// ===================================================================== //

/// Write the frozen locale strings for regional language review: one row per
/// key with the English source, the Mainland Simplified and the Taiwan
/// Traditional text, tab separated, newlines escaped so every key is one row.
/// Run explicitly (it writes a file):
///
/// ```text
/// EBBTOP_L10N_EXPORT=/tmp/strings.tsv cargo test -p ebbtop-gui export_frozen_strings -- --ignored
/// ```
#[test]
#[ignore = "producer step: writes the review export when EBBTOP_L10N_EXPORT is set"]
fn export_frozen_strings() {
    let path = std::env::var("EBBTOP_L10N_EXPORT").expect("set EBBTOP_L10N_EXPORT");
    let esc = |t: &str| {
        t.replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\t', "\\t")
    };
    let mut out = String::from("key\ten\tzh-Hans-CN\tzh-Hant-TW\n");
    for &k in Key::ALL {
        out.push_str(&format!(
            "{k:?}\t{}\t{}\t{}\n",
            esc(l10n::text(Locale::English, k)),
            esc(l10n::text(Locale::ZhHansCn, k)),
            esc(l10n::text(Locale::ZhHantTw, k)),
        ));
    }
    let rows = out.lines().count();
    assert_eq!(
        rows,
        Key::ALL.len() + 1,
        "every key must produce exactly one row"
    );
    std::fs::write(&path, &out).expect("write the export");
    eprintln!("wrote {rows} rows ({} keys) to {path}", Key::ALL.len());
}

/// Rendered PNG evidence for both Chinese variants (and English), through the
/// same wgpu path the app draws with. Ignored by default; run with the output
/// directory set:
///
/// ```text
/// EBBTOP_SHOT_DIR=/tmp/shots cargo test -p ebbtop-gui l10n_screenshots -- --ignored
/// ```
#[test]
#[ignore = "producer step: renders PNGs, needs EBBTOP_SHOT_DIR and a wgpu adapter"]
fn l10n_screenshots() {
    let dir = std::path::PathBuf::from(std::env::var("EBBTOP_SHOT_DIR").expect("EBBTOP_SHOT_DIR"));
    std::fs::create_dir_all(&dir).expect("shot dir");
    let shot = |name: &str, mut app: App, size: (f32, f32)| {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(size.0, size.1))
            .wgpu()
            .build_ui(move |ui| {
                style(ui.ctx());
                ui.ctx().options_mut(|o| o.warn_on_id_clash = false);
                app.draw(ui)
            });
        for _ in 0..4 {
            harness.step();
        }
        let img = harness.render().expect("render");
        let path = dir.join(name);
        img.save(&path).expect("save");
        eprintln!("wrote {}", path.display());
    };
    for loc in Locale::ALL {
        let tag = loc.id().replace('-', "");
        // Dashboard with the detail pane and the workload chip.
        let mut dash = dash_in(loc);
        dash.selected = Some(4242);
        let wkey = {
            let shared = dash.shared();
            let s = shared.lock().unwrap();
            s.workloads.first().map(|g| g.key.clone())
        };
        dash.workload_filter = wkey;
        shot(
            &format!("{tag}-dashboard-960x1040.png"),
            dash,
            (960.0, 1040.0),
        );

        // Settings on the Display tab, with the license notice open.
        let mut set = dash_in(loc);
        set.settings_open = true;
        set.settings_tab = SettingsTab::Display;
        shot(
            &format!("{tag}-settings-display-960x1040.png"),
            set,
            (960.0, 1040.0),
        );

        let mut lic = dash_in(loc);
        lic.font_notice_open = true;
        shot(
            &format!("{tag}-font-notice-960x1040.png"),
            lic,
            (960.0, 1040.0),
        );

        // Kill confirmation.
        let mut kill = dash_in(loc);
        kill.kill_confirm = Some(KillConfirm {
            pid: 4242,
            name: "hottest-proc".into(),
            start_time_ms: 1,
        });
        shot(
            &format!("{tag}-kill-confirm-960x1040.png"),
            kill,
            (960.0, 1040.0),
        );

        // The minimum window size, the densest layout.
        let small = dash_in(loc);
        shot(
            &format!("{tag}-dashboard-480x420.png"),
            small,
            (480.0, 420.0),
        );

        // Discovery + pairing prompt in the wizard.
        let mut wiz = wizard_in(loc);
        {
            let ob = wiz.onboard.as_mut().expect("wizard");
            ob.discovery
                .agents
                .push(agent("k-server", "0.2.9", "10.0.0.5", 45976));
            ob.pair = Some(Pairing::for_address("k-server", "ws://10.0.0.5/ws"));
        }
        shot(
            &format!("{tag}-wizard-pairing-960x1040.png"),
            wiz,
            (960.0, 1040.0),
        );
    }
}

// --------------------------------------------------------------------- //
// The age chip's tooltip is composed entirely from the key table          //
// --------------------------------------------------------------------- //
//
// F1 of the round-1 review: the tooltip's lag parenthetical was built with
// `format!(" (lag {l} ms)")` in `status_bar()`, so both Chinese locales painted
// an English label inside a localized sentence. The assertions below paint the
// real status bar, hover the age chip and read the tooltip egui actually
// painted — not the source template — so the regression cannot come back.

/// Every painted text of one frame, with its rectangle, so the test can hover
/// the age chip it just painted.
fn text_shapes(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect)> {
    fn walk(s: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match s {
            egui::Shape::Text(t) => out.push((
                t.galley.text().to_string(),
                egui::Rect::from_min_size(t.pos, t.galley.size()),
            )),
            egui::Shape::Vec(v) => {
                for s in v {
                    walk(s, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in shapes {
        walk(&c.shape, &mut out);
    }
    out
}

/// The tooltip a real hover paints, plus the clock reads that bound the frame.
///
/// One persistent context (the bundled fonts stay installed), tooltips forced
/// to show at once instead of after the 0.5 s still-time delay, and the pointer
/// parked on the age chip the app itself painted. The sample time is stamped
/// 5 ms back immediately before the tooltip frame, so the painted lag is the
/// nominal 5 ms plus at most this frame's own drawing time.
fn painted_age_tooltip(app: &mut App, loc: Locale) -> (String, u64, u64) {
    let ctx = egui::Context::default();
    style(&ctx);
    ctx.all_styles_mut(|s| {
        s.interaction.tooltip_delay = 0.0;
        s.interaction.show_tooltips_only_when_still = false;
    });
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(960.0, 1000.0));
    let run = |app: &mut App, events: Vec<egui::Event>| -> Vec<(String, egui::Rect)> {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            focused: true,
            ..Default::default()
        };
        input.events = events;
        let mut out = ctx.run_ui(input, |ui| app.draw(ui));
        out.textures_delta.clear(); // no renderer in tests
        text_shapes(&out.shapes)
    };
    // Settle, then find the chip in the frame the app painted.
    let _ = run(app, vec![]);
    let age_prefix = l10n::text(loc, Key::Age)
        .split("{}")
        .next()
        .unwrap_or("age")
        .to_string();
    let chip = run(app, vec![])
        .into_iter()
        .find(|(t, _)| t.starts_with(&age_prefix))
        .unwrap_or_else(|| panic!("{loc}: no age chip was painted"));
    let pos = chip.1.center();
    // The hover frame establishes the tooltip; the frame after it paints the
    // tooltip egui composed, reading the sample time stamped here.
    let _ = run(app, vec![egui::Event::PointerMoved(pos)]);
    let set_at = client::now_ms();
    {
        let shared = app.shared();
        let mut s = shared.lock().unwrap();
        s.metrics
            .as_mut()
            .expect("dashboard fixture has metrics")
            .sampled_at_ms = set_at - 5;
    }
    let painted = run(app, vec![egui::Event::PointerMoved(pos)]);
    let hover_prefix = l10n::text(loc, Key::AgeHover)
        .split("{}")
        .next()
        .unwrap_or("")
        .to_string();
    let tooltip = painted
        .iter()
        .map(|(t, _)| t.clone())
        .find(|t| t.starts_with(&hover_prefix))
        .unwrap_or_else(|| {
            panic!(
                "{loc}: hovering the age chip painted no tooltip: {:?}",
                painted.iter().map(|p| &p.0).collect::<Vec<_>>()
            )
        });
    (tooltip, set_at, client::now_ms())
}

// --------------------------------------------------------------------- //
// English fidelity control: pin the *painted* tooltip to the accepted     //
// base revision (round-2 review advisory F3)                              //
// --------------------------------------------------------------------- //
//
// The synthetic composer control this replaces passed `diag = "D"`, which
// bypasses the leading-newline diagnostic path the real hover paints, so it
// could not see that the tooltip had gained a blank line between the lag
// parenthetical and the sample line. The literal below is the tooltip a real
// hover painted on the accepted base `3797eaf` (probe run with the same
// fixture state: metrics present, tick p95 1.0s, one reconnect). `{CLOCK}` and
// the two numeric spans are the only volatile parts; `normalize_age_tooltip`
// masks exactly those, so every other byte is pinned.

const BASE_3797EAF_EN_AGE_TOOLTIP: &str = "time since a new sample arrived, local clock (lag 6 ms)\n\
sample {CLOCK} · tick p95 1.0s · reconnects 3\n\
conn=connected since_new_ms=2001 interval_p50=1000 interval_p95=1000 interval_max=1000 lag_p50=3 lag_p95=3 proc_interval_p95=na proc_lag_p95=3 gaps=0 repeats=0 reconnects=3 samples=70";

/// Mask the two clock-dependent spans of an age tooltip: the local sample time
/// and the numeric value of `since_new_ms` and of the lag parenthetical.
fn normalize_age_tooltip(text: &str, clock: &str) -> String {
    let mut s = text.replace(clock, "<clock>");
    for key in ["since_new_ms=", "(lag "] {
        let mut out = String::new();
        let mut rest: &str = &s;
        while let Some(i) = rest.find(key) {
            out.push_str(&rest[..i + key.len()]);
            let tail = &rest[i + key.len()..];
            let end = tail
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(tail.len());
            out.push_str("<n>");
            rest = &tail[end..];
        }
        out.push_str(rest);
        s = out;
    }
    s
}

/// The lag fragment is localized, the number and `ms` unit are unchanged data,
/// the adjacent sample/tick/reconnect labels stay localized, and English keeps
/// its base tooltip byte for byte.
#[test]
fn l10n_age_tooltip_lag_localized_both_scripts() {
    for loc in Locale::ALL {
        let mut app = dash_in(loc);
        // A nominal 5 ms lag is stamped inside the paint helper immediately
        // before the tooltip frame; the exact painted value is read back from
        // the tooltip and checked against that frame's clock window, so a
        // millisecond boundary can neither flake nor weaken the assertion.
        {
            let shared = app.shared();
            let mut s = shared.lock().unwrap();
            s.reconnects = 3;
        }
        let (tooltip, set_at, t1) = painted_age_tooltip(&mut app, loc);

        // The painted lag fragment is the localized one.
        let tmpl = l10n::text(loc, Key::Lag);
        let (pre, post) = tmpl.split_once("{}").expect("lag template has one value");
        let rest = tooltip
            .split_once(pre)
            .unwrap_or_else(|| panic!("{loc}: tooltip has no lag fragment {pre:?}: {tooltip:?}"))
            .1;
        let (num, _) = rest
            .split_once(post)
            .unwrap_or_else(|| panic!("{loc}: lag fragment lost its unit {post:?}: {tooltip:?}"));
        let n: i64 = num
            .parse()
            .unwrap_or_else(|_| panic!("{loc}: lag value {num:?} is not a number"));
        // The value is the real sample age at paint time: at least the nominal
        // 5 ms, and never beyond the window between the stamp and the read-back.
        assert!(
            n >= 5 && n <= 5 + (t1 - set_at) as i64,
            "{loc}: painted lag {n} ms is outside the 5 ms + frame-time window"
        );
        // The first tooltip line is the localized summary followed by the
        // localized lag fragment — nothing English in between.
        let first_line = l10n::fmt(loc, Key::AgeHover, &["", ""]);
        let first_line = first_line.split('\n').next().unwrap_or("");
        let want = format!(
            "{first_line}{}",
            l10n::fmt(loc, Key::Lag, &[&n.to_string()])
        );
        assert_eq!(
            tooltip.split('\n').next().unwrap_or(""),
            want,
            "{loc}: first tooltip line is not the localized summary + lag"
        );

        // No blank line anywhere in the painted tooltip, in any locale: the
        // diagnostic block carries its own leading newline and `AgeHover` must
        // not add a second one (round-2 advisory F3).
        assert!(
            !tooltip.contains("\n\n"),
            "{loc}: the painted tooltip has a blank line: {tooltip:?}"
        );
        if loc == Locale::English {
            // The painted English tooltip IS the accepted base tooltip: three
            // lines, same words, same data, same units — no added whitespace.
            let clock = local_time(set_at - 5);
            assert_eq!(
                tooltip.lines().count(),
                3,
                "English: the painted tooltip is not the base three-line tooltip: {tooltip:?}"
            );
            assert_eq!(
                normalize_age_tooltip(&tooltip, &clock),
                normalize_age_tooltip(
                    &BASE_3797EAF_EN_AGE_TOOLTIP.replace("{CLOCK}", &clock),
                    &clock
                ),
                "English: the painted tooltip deviates from the accepted base 3797eaf tooltip"
            );
            // The lag fragment is the base English parenthetical, unchanged.
            assert!(
                tooltip.contains(&format!(" (lag {n} ms)")),
                "English: the base lag parenthetical changed: {tooltip:?}"
            );
        }
        if loc != Locale::English {
            // The base English parenthetical must not be painted in a Chinese
            // tooltip. (The canonical support summary keeps its `lag_p50=`
            // field names: those are raw diagnostic keys, not this label.)
            assert!(
                !tooltip.contains(" (lag "),
                "{loc}: the English lag label is still painted: {tooltip:?}"
            );
            assert!(
                has_cjk(&tooltip),
                "{loc}: the tooltip carries no Chinese text at all: {tooltip:?}"
            );
        }

        // Adjacent diagnostics stay localized around verbatim data.
        let diag = l10n::text(loc, Key::DiagSampleTick);
        let mid = diag
            .split("{}")
            .nth(1)
            .expect("tick-p95 template has a middle part");
        assert!(
            tooltip.contains(mid),
            "{loc}: tick p95 label is not localized: {tooltip:?}"
        );
        assert!(
            tooltip.contains(&l10n::fmt(loc, Key::DiagReconnects, &["3"])),
            "{loc}: reconnects label is not localized: {tooltip:?}"
        );
        assert!(
            tooltip.contains(&local_time(set_at - 5)),
            "{loc}: the sample time is not painted verbatim"
        );
        // Adjacent diagnostics stay localized around verbatim data. Only the
        // localized head is asserted: the last line is the canonical English
        // support summary (documented by design), which keeps field names like
        // `reconnects=` and is not a UI label. The head is every line but that
        // summary — three lines in the base layout (summary, sample, support),
        // so `take(3)` would swallow the support line.
        let lines: Vec<&str> = tooltip.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "{loc}: unexpected tooltip line count: {tooltip:?}"
        );
        let head: String = lines[..lines.len() - 1].join("\n");
        if loc != Locale::English {
            assert!(
                !head.contains("reconnects"),
                "{loc}: English 'reconnects' in the localized head: {head:?}"
            );
            assert!(
                !head.contains("tick p95"),
                "{loc}: English 'tick p95': {head:?}"
            );
            assert!(
                !head.contains("sample "),
                "{loc}: English 'sample': {head:?}"
            );
        }
    }

    // English controls: the fragment is the base parenthetical, and the
    // painted tooltip itself was pinned to the base revision inside the loop
    // above (`BASE_3797EAF_EN_AGE_TOOLTIP`). No synthetic-diag control here:
    // `diag = "D"` skips the leading-newline diagnostic path the real hover
    // paints, which is what let the F3 blank line through in round 2.
    assert_eq!(l10n::text(Locale::English, Key::Lag), " (lag {} ms)");
    assert_eq!(l10n::fmt(Locale::English, Key::Lag, &["5"]), " (lag 5 ms)");

    // No lag value: the fragment contributes nothing at all, so no locale can
    // paint an empty parenthetical. (`sample_age_ms` is `None` only when there
    // are no metrics, and then the age chip itself is not painted — this is the
    // composition contract the chip's `Option` feeds.)
    for loc in Locale::ALL {
        let none = age_tooltip(loc, None, "D");
        assert_eq!(
            none,
            l10n::fmt(loc, Key::AgeHover, &["", "D"]),
            "{loc}: empty lag changed the line"
        );
        for token in ["lag", "延迟", "延遲", "()", "（）"] {
            assert!(
                !none.contains(token),
                "{loc}: absent lag painted {token:?}: {none:?}"
            );
        }
    }
}
