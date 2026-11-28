//! btop-style painted widgets: line/area graphs and meters.

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2};

pub const BG: Color32 = Color32::from_rgb(0x10, 0x12, 0x16);
pub const BOX: Color32 = Color32::from_rgb(0x16, 0x19, 0x1f);
pub const LINE: Color32 = Color32::from_rgb(0x2c, 0x31, 0x3a);
pub const DIM: Color32 = Color32::from_rgb(0x7d, 0x85, 0x90);
pub const TEXT: Color32 = Color32::from_rgb(0xd8, 0xdc, 0xe2);
pub const ACCENT: Color32 = Color32::from_rgb(0x5f, 0xb3, 0xe6);
pub const OK: Color32 = Color32::from_rgb(0x5c, 0xc8, 0x6e);
pub const WARN: Color32 = Color32::from_rgb(0xe6, 0xb4, 0x50);
pub const BAD: Color32 = Color32::from_rgb(0xe6, 0x5c, 0x5c);
pub const RX: Color32 = Color32::from_rgb(0x5f, 0xb3, 0xe6);
pub const TX: Color32 = Color32::from_rgb(0xc0, 0x7f, 0xe0);
/// Per-metric hues (phase 1). `heat()` stays for CPU%/temp cells only.
/// Values reuse the existing btop-ish palette, so no new look-shock.
pub const CPU: Color32 = Color32::from_rgb(0x5f, 0xb3, 0xe6);
pub const MEM: Color32 = Color32::from_rgb(0x5c, 0xc8, 0x6e);
pub const GPU: Color32 = Color32::from_rgb(0xc0, 0x7f, 0xe0);
pub const DISK: Color32 = Color32::from_rgb(0xe6, 0xb4, 0x50);

/// Fill of the window's top bar: `BOX` lifted 35% toward `LINE`, i.e.
/// `lerp(BOX, LINE, 0.35)`. The bar reads as its own surface above the page
/// (distinct from `BG`) without introducing a hue the palette does not
/// already carry — no new cyan, no gradient, one flat fill
/// (`bar_colors_derive_from_the_palette`).
pub const BAR: Color32 = Color32::from_rgb(0x1e, 0x21, 0x28);

/// The bar's bottom edge: one pixel of `ACCENT` pulled back 70% toward
/// `LINE`, `lerp(LINE, ACCENT, 0.30)`. Visible against both the bar fill and
/// the page below it, still the app's own blue rather than a new accent.
pub const BAR_RULE_COLOR: Color32 = Color32::from_rgb(0x3b, 0x58, 0x6e);

/// Height (points) of the bar's bottom rule: one logical pixel, painted
/// inside the bar's own last row so it costs the layout no height.
pub const BAR_RULE: f32 = 1.0;

/// Product name on the bar's leading edge. One place in the GUI: the
/// wordmark is a constant, never a source/hostname from the wire. The
/// window title (`ViewportCommand::Title`) keeps its own `EBBTOP <source>`
/// form and is not derived from this.
pub const BRAND: &str = "EbbTop";

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Green -> amber -> red as `frac` goes 0 -> 1 (btop's default gradient).
pub fn heat(frac: f32) -> Color32 {
    if frac < 0.5 {
        lerp(OK, WARN, frac * 2.0)
    } else {
        lerp(WARN, BAD, (frac - 0.5) * 2.0)
    }
}

/// btop-like line + area graph, newest sample at the right edge. `slots`
/// spans the width so the time scale is fixed whatever the number of
/// samples held. Missing samples at the left stay empty. `color` None
/// uses `heat` per-point. Area is a translucent wash (grid shows
/// through), the value polyline is the bright top edge btop draws.
/// Hover paints a crosshair + value readout (allocation only while
/// hovered, plus one point Vec per graph — measured in `frame_cost`).
pub fn graph(
    ui: &mut Ui,
    vals: &[f64],
    slots: usize,
    max: f64,
    height: f32,
    color: Option<Color32>,
) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, BG);
    // 80/90% thresholds (phase 1) replace the old 25/75 grid; 50% stays.
    for f in [0.5f32, 0.8, 0.9] {
        let y = rect.bottom() - rect.height() * f;
        p.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0, LINE),
        );
    }
    let slots = slots.max(2);
    let w = rect.width() / slots as f32;
    let n = vals.len().min(slots);
    let max = if max > 0.0 { max } else { 1.0 };
    for (k, v) in vals[vals.len() - n..].iter().enumerate() {
        let frac = (*v / max).clamp(0.0, 1.0) as f32;
        if frac <= 0.0 {
            continue;
        }
        let slot = slots - n + k;
        let x0 = rect.left() + slot as f32 * w;
        let top = rect.bottom() - rect.height() * frac;
        let c = color.unwrap_or_else(|| heat(frac));
        // Translucent wash, not a solid bar: overdraw joins columns and
        // lets the threshold grid show through, like btop's shaded area.
        let wash = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 78);
        p.rect_filled(
            Rect::from_min_max(pos2(x0, top), pos2(x0 + w + 0.6, rect.bottom())),
            0.0,
            wash,
        );
    }
    if n > 1 {
        if let Some(solid) = color {
            let bright = lerp(solid, Color32::WHITE, 0.45);
            let mut pts = Vec::with_capacity(n);
            for (k, v) in vals[vals.len() - n..].iter().enumerate() {
                let frac = (*v / max).clamp(0.0, 1.0) as f32;
                let slot = slots - n + k;
                let cx = rect.left() + (slot as f32 + 0.5) * w;
                pts.push(pos2(cx, rect.bottom() - rect.height() * frac));
            }
            p.add(egui::Shape::line(pts, Stroke::new(1.5, bright)));
        } else {
            let mut prev: Option<Pos2> = None;
            for (k, v) in vals[vals.len() - n..].iter().enumerate() {
                let frac = (*v / max).clamp(0.0, 1.0) as f32;
                let slot = slots - n + k;
                let cx = rect.left() + (slot as f32 + 0.5) * w;
                let cur = pos2(cx, rect.bottom() - rect.height() * frac);
                let bright = lerp(heat(frac), Color32::WHITE, 0.35);
                if let Some(p0) = prev {
                    p.line_segment([p0, cur], Stroke::new(1.5, bright));
                }
                prev = Some(cur);
            }
        }
    }
    if resp.hovered()
        && n > 0
        && let Some(ptr) = resp.interact_pointer_pos()
        && rect.contains(ptr)
        && w > 0.0
    {
        let slot = (((ptr.x - rect.left()) / w).floor() as usize).min(slots - 1);
        if slot >= slots - n {
            let k = slot - (slots - n);
            let v = vals[vals.len() - n + k];
            let x = rect.left() + (slot as f32 + 0.5) * w;
            p.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.0, ACCENT),
            );
            let font = egui::TextStyle::Monospace.resolve(ui.style());
            let txt = if max == 100.0 {
                format!("{v:.1}%")
            } else {
                format!("{v:.0}")
            };
            let galley = p.layout_no_wrap(txt, font, TEXT);
            let pos = pos2(
                (x + 4.0).min((rect.right() - galley.size().x - 4.0).max(rect.left() + 2.0)),
                rect.top() + 2.0,
            );
            p.rect_filled(
                Rect::from_min_size(pos - vec2(2.0, 1.0), galley.size() + vec2(4.0, 2.0)),
                2.0,
                BG,
            );
            p.galley(pos, galley, TEXT);
        }
    }
    rect
}

/// Horizontal meter with left label and right text, e.g. "Used [#####   ] 12.3 GiB".
pub fn meter(ui: &mut Ui, label: &str, frac: f32, right: &str, color: Option<Color32>) {
    let h = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter_at(rect);
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    // Measure both texts first; the bar takes what is left between them.
    let lg = p.layout_no_wrap(label.to_string(), font.clone(), DIM);
    let rg = p.layout_no_wrap(right.to_string(), font, TEXT);
    let label_w = (lg.size().x + 6.0).max(30.0);
    let right_w = rg.size().x + 6.0;
    let bar = Rect::from_min_max(
        Pos2::new(rect.left() + label_w, rect.top() + 3.0),
        Pos2::new(
            (rect.right() - right_w).max(rect.left() + label_w + 10.0),
            rect.bottom() - 3.0,
        ),
    );
    p.rect_filled(bar, 2.0, BG);
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let fill = Rect::from_min_size(bar.min, Vec2::new(bar.width() * f, bar.height()));
        p.rect_filled(fill, 2.0, color.unwrap_or_else(|| heat(f)));
    }
    let ly = rect.center().y - lg.size().y / 2.0;
    p.galley(pos2(rect.left(), ly), lg, DIM);
    p.galley(pos2(rect.right() - rg.size().x, ly), rg, TEXT);
}

/// Short form of a GPU name: the body rows and the panel header both drop
/// the "NVIDIA " vendor prefix, so one model never reads two ways.
pub fn short_gpu_name(name: &str) -> &str {
    name.strip_prefix("NVIDIA ").unwrap_or(name)
}

/// Point size added to [`egui::TextStyle::Body`] for a panel title. Body
/// text is 13 pt, so a title renders at 15 pt: the heading line box is
/// taller than any body line in the same frame, which is what makes a
/// section boundary visible at a glance
/// (`panel_titles_render_larger_than_body_text`).
pub const TITLE_STEP: f32 = 2.0;

/// Extra letter tracking (points) on the title run. Tracking widens the
/// wordmark without touching glyph height, so it reads as a heading without
/// growing the header row any further.
pub const TITLE_TRACKING: f32 = 0.5;

/// Height (points) of the rule drawn under the title row: a `LINE` hairline
/// with an `ACCENT` hairline on top. Painted into the 2 px gap the header
/// already left, so the section rule costs the layout nothing.
pub const TITLE_RULE: f32 = 2.0;

/// The title font: body family, one step larger. Kept a function of the
/// body style so a theme change moves both together.
pub fn title_font(style: &egui::Style) -> egui::FontId {
    let body = egui::TextStyle::Body.resolve(style);
    egui::FontId::new(body.size + TITLE_STEP, body.family)
}

/// Titled box with btop-like border. Title is proportional and one step
/// larger than body text, with an accent rule under it (phase 1);
/// numbers inside stay monospace. Clicking the title collapses/expands
/// (`open` false = collapsed, content is skipped so frame cost drops
/// and siblings reflow). Arrow is ASCII (proportional lacks ▼/▶).
pub fn panel(ui: &mut Ui, title: &str, open: &mut bool, add: impl FnOnce(&mut Ui)) {
    panel_with_model(ui, title, None, open, add);
}

/// `panel` plus a hardware model on the title line, e.g. `v cpu · i7-9750H`.
///
/// The model rides on the header label itself, not a second widget, so it
/// stays visible while the panel is collapsed and costs no extra row. The
/// label is truncated (`…`, auto tooltip) rather than wrapped, so an
/// over-long model cannot grow the header row or push siblings aside at the
/// minimum width. `model` None/blank leaves the title alone — no separator.
pub fn panel_with_model(
    ui: &mut Ui,
    title: &str,
    model: Option<&str>,
    open: &mut bool,
    add: impl FnOnce(&mut Ui),
) {
    egui::Frame::new()
        .fill(BOX)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(4.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let arrow = if *open { "v " } else { "> " };
            let font = title_font(ui.style());
            let fmt = |color: Color32| egui::text::TextFormat {
                font_id: font.clone(),
                color,
                extra_letter_spacing: TITLE_TRACKING,
                ..Default::default()
            };
            let mut job = egui::text::LayoutJob::default();
            job.append(&format!("{arrow}{title}"), 0.0, fmt(ACCENT));
            if let Some(m) = model.map(str::trim).filter(|m| !m.is_empty()) {
                job.append(" · ", 0.0, fmt(LINE));
                job.append(m, 0.0, fmt(DIM));
            }
            // Clickable label (not selectable_label): the whole header line is
            // one hit target, and `sense(click)` still gives the pointer cursor.
            let resp = ui.add(egui::Label::new(job).truncate().sense(Sense::click()));
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                *open = !*open;
            }
            // Accent rule under the title: a section boundary that survives
            // colour blindness and a monochrome screenshot (weight cannot —
            // the bundled family ships one weight). It spans the panel, not
            // the label, so a short title rules off the whole section. Painted
            // into the 2 px gap the header already left, so it costs no height.
            let line_rect = egui::Rect::from_min_size(
                egui::pos2(resp.rect.left(), resp.rect.bottom()),
                egui::vec2(ui.available_width(), TITLE_RULE),
            );
            ui.painter().rect_filled(line_rect, 0.0, LINE);
            ui.painter().rect_filled(
                egui::Rect::from_min_size(line_rect.min, egui::vec2(line_rect.width(), 1.0)),
                0.0,
                ACCENT,
            );
            if *open {
                add(ui);
            }
        });
}

/// Inline CPU% cell: translucent heat bar behind monospace text.
/// One extra rect per *visible* row only; no per-frame Vec allocation.
pub fn cpu_bar_cell(ui: &mut Ui, frac: f32, text: &str) {
    let h = ui.text_style_height(&egui::TextStyle::Monospace);
    let w = ui.available_width();
    if w <= 0.0 {
        return;
    }
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, BG);
    let f = frac.clamp(0.0, 1.0);
    if f > 0.005 {
        let base = heat(f);
        let wash = Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 42);
        let fill = Rect::from_min_size(rect.min, Vec2::new(rect.width() * f, rect.height()));
        p.rect_filled(fill, 2.0, wash);
    }
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let galley = p.layout_no_wrap(text.to_string(), font, TEXT);
    let pos = pos2(
        rect.right() - galley.size().x,
        rect.center().y - galley.size().y / 2.0,
    );
    p.galley(pos, galley, TEXT);
}

/// One classified failure, drawn the same way on every surface (E4): the
/// class as a red headline, one concrete next action, then the scrubbed
/// detail in monospace. Callers pass an already-classified, already-scrubbed
/// triple (`client::Diagnosis`, `pairing::PairError`); this only draws, so a
/// new failure source cannot invent its own styling or forget the action.
pub fn error_block(
    ui: &mut Ui,
    locale: crate::l10n::Locale,
    headline: &str,
    next_action: &str,
    detail: &str,
) {
    ui.add_space(4.0);
    ui.label(RichText::new(headline).strong().color(BAD));
    ui.add(egui::Label::new(RichText::new(next_action).small().color(TEXT)).wrap());
    ui.add(
        egui::Label::new(
            RichText::new(crate::l10n::fmt(
                locale,
                crate::l10n::Key::DetailPrefix,
                &[detail],
            ))
            .small()
            .monospace()
            .color(DIM),
        )
        .wrap(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heat_endpoints() {
        assert_eq!(heat(0.0), OK);
        assert_eq!(heat(0.5), WARN);
        assert_eq!(heat(1.0), BAD);
        assert_eq!(heat(7.0), BAD);
    }

    /// The bar's colours are derived from the existing palette, not new hues:
    /// the fill is `BOX` lifted toward `LINE`, the rule is `ACCENT` pulled back
    /// toward `LINE`, and neither is the page fill.
    #[test]
    fn bar_colors_derive_from_the_palette() {
        assert_eq!(
            BAR,
            lerp(BOX, LINE, 0.35),
            "bar fill must be BOX lifted toward LINE"
        );
        assert_eq!(
            BAR_RULE_COLOR,
            lerp(LINE, ACCENT, 0.30),
            "bar rule must be ACCENT pulled toward LINE"
        );
        assert_ne!(BAR, BG, "the bar must not be the page fill");
        assert_ne!(
            BAR, BOX,
            "the bar must read as its own surface, not a panel"
        );
        assert_eq!(BAR_RULE, 1.0, "the bottom rule is one pixel");
        assert!(
            !BRAND.is_empty() && BRAND.len() <= 8,
            "wordmark stays short: {BRAND}"
        );
    }

    // ---------------------------------------------------------- WCAG 2.2 ---
    // Contrast is measured, not eyeballed: these tests compute the WCAG 2.2
    // relative-luminance ratio for every foreground/background pair the app
    // actually paints, and pin the numbers so a palette edit cannot slip a
    // pair below AA unnoticed. Findings A-6/A-4 in
    // `docs/accessibility-audit.md` are the pairs that fail today; they are
    // asserted as failures on purpose and flip when the palette is fixed.

    /// WCAG 2.2 relative luminance (sRGB, per channel de-gamma).
    fn luminance(c: Color32) -> f64 {
        fn channel(v: u8) -> f64 {
            let c = v as f64 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    /// WCAG 2.2 contrast ratio, 1.0 .. 21.0.
    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// `fg` at `alpha` over an opaque `bg`, the way egui's `disabled_alpha`
    /// and the translucent meter washes composite.
    fn over(fg: Color32, bg: Color32, alpha: f32) -> Color32 {
        let mix = |f: u8, b: u8| (f as f32 * alpha + b as f32 * (1.0 - alpha)).round() as u8;
        Color32::from_rgb(
            mix(fg.r(), bg.r()),
            mix(fg.g(), bg.g()),
            mix(fg.b(), bg.b()),
        )
    }

    /// Assert a measured ratio equals the audited value (2 decimal places).
    fn pin(what: &str, got: f64, want: f64) {
        assert!(
            (got - want).abs() < 0.02,
            "{what}: measured {got:.2}:1, the audit recorded {want:.2}:1 — re-measure and update the report"
        );
    }

    /// Text the app paints, at the AA threshold for its size: 4.5:1 for body
    /// and table text (13 pt monospace), 3:1 for the 15 pt panel titles.
    #[test]
    fn wcag_aa_text_contrast_holds_for_every_passing_pair() {
        let ctx = egui::Context::default();
        crate::app::style(&ctx);
        let v = ctx.global_style().visuals.clone();
        let faint = v.faint_bg_color;

        // (what, fg, bg, minimum, recorded ratio)
        let cases: &[(&str, Color32, Color32, f64, f64)] = &[
            ("body text on the page", TEXT, BG, 4.5, 13.62),
            ("body text in a panel", TEXT, BOX, 4.5, 12.79),
            ("body text in the status bar", TEXT, BAR, 4.5, 11.70),
            ("body text in a table row", TEXT, faint, 4.5, 12.01),
            ("secondary text on the page", DIM, BG, 4.5, 5.03),
            ("secondary text in a panel", DIM, BOX, 4.5, 4.72),
            (
                "pane titles and the wordmark in panels",
                ACCENT,
                BOX,
                3.0,
                7.61,
            ),
            ("pane titles in the status bar", ACCENT, BAR, 3.0, 6.96),
            ("LIVE badge", OK, BAR, 4.5, 7.63),
            ("STALE / CONNECTING badge", WARN, BAR, 4.5, 8.45),
            ("DOWN badge", BAD, BAR, 4.5, 4.64),
            ("warnings in a panel", WARN, BOX, 4.5, 9.23),
            ("errors in a panel", BAD, BOX, 4.5, 5.07),
            ("errors on the page", BAD, BG, 4.5, 5.40),
            (
                "primary text on a selected row",
                TEXT,
                v.selection.bg_fill,
                4.5,
                5.37,
            ),
            (
                "button label",
                v.widgets.inactive.fg_stroke.color,
                v.widgets.inactive.weak_bg_fill,
                4.5,
                8.91,
            ),
            (
                "hovered button label",
                v.widgets.hovered.fg_stroke.color,
                v.widgets.hovered.weak_bg_fill,
                4.5,
                9.51,
            ),
            (
                "focused/active button label",
                v.widgets.active.fg_stroke.color,
                v.widgets.active.weak_bg_fill,
                4.5,
                14.74,
            ),
            (
                "non-interactive label in a window",
                v.widgets.noninteractive.fg_stroke.color,
                BOX,
                4.5,
                5.23,
            ),
        ];
        for (what, fg, bg, min, recorded) in cases {
            let got = contrast(*fg, *bg);
            pin(what, got, *recorded);
            assert!(got >= *min, "{what}: {got:.2}:1 is below AA {min}:1");
        }
    }

    /// Findings A-6 (minor) and A-4 (major): the pairs that fail AA today.
    /// Pinned as failures so the fixing slice inverts them instead of the gate
    /// quietly going green.
    #[test]
    fn wcag_aa_contrast_gaps_are_pinned() {
        let ctx = egui::Context::default();
        crate::app::style(&ctx);
        let v = ctx.global_style().visuals.clone();
        let faint = v.faint_bg_color;
        let sel = v.selection.bg_fill;

        // Secondary text on the two surfaces that sit behind it.
        let dim_on_bar = contrast(DIM, BAR);
        pin("secondary text in the status bar", dim_on_bar, 4.32);
        assert!(
            dim_on_bar < 4.5,
            "A-6: the stale age / kill note is below AA on the bar"
        );

        let dim_on_row = contrast(DIM, faint);
        pin("secondary text in a striped row", dim_on_row, 4.43);
        assert!(
            dim_on_row < 4.5,
            "A-6: PID, state, MEM and workload cells are below AA at 4.43:1"
        );

        // The selected-row surface is where contrast is worst.
        let dim_on_sel = contrast(DIM, sel);
        pin("secondary text on a selected row", dim_on_sel, 1.98);
        assert!(
            dim_on_sel < 4.5,
            "A-4: a selected row's secondary cells are unreadable at 1.98:1"
        );

        let accent_on_sel = contrast(ACCENT, sel);
        pin("selected workload row label", accent_on_sel, 3.19);
        assert!(
            accent_on_sel < 4.5,
            "A-4: the active workload label is below AA on the selection fill"
        );

        // 1.4.11 non-text: the selection fill is the only cue a row is selected.
        let sel_on_box = contrast(sel, BOX);
        pin("selection fill against a panel", sel_on_box, 2.38);
        assert!(
            sel_on_box < 3.0,
            "A-4: the selection fill is below the 3:1 component minimum"
        );
    }

    /// 1.4.11 non-text contrast for the load bars: every stop of `heat()` is a
    /// graphical object the user must read against the page or a panel, and the
    /// value text on a CPU cell keeps AA over its translucent wash.
    #[test]
    fn wcag_non_text_contrast_for_load_bars() {
        for (frac, name) in [
            (0.0, "green"),
            (0.25, "green-amber"),
            (0.5, "amber"),
            (0.75, "amber-red"),
            (1.0, "red"),
        ] {
            let c = heat(frac);
            for (surface, bg) in [("page", BG), ("panel", BOX)] {
                let r = contrast(c, bg);
                assert!(
                    r >= 3.0,
                    "load bar {name} on {surface}: {r:.2}:1 is below 3:1"
                );
            }
        }
        // The CPU cell paints `TEXT` over a 42/255 wash of `heat()` on `BG`.
        for frac in [0.0f32, 0.3, 0.6, 1.0] {
            let wash = over(heat(frac), BG, 42.0 / 255.0);
            let r = contrast(TEXT, wash);
            assert!(
                r >= 4.5,
                "CPU cell text over a {frac} wash: {r:.2}:1 is below AA"
            );
        }
        // Decorative separators are exempt from 1.4.11, and stay decorative.
        assert!(
            contrast(LINE, BG) < 3.0,
            "panel borders must stay quiet, not become UI indicators"
        );
    }

    /// Disabled controls are exempt from 1.4.3 (WCAG: "inactive user interface
    /// components have no contrast requirement"), and egui halves their alpha.
    /// Measured so the report can state the number instead of the exemption
    /// doing hidden work.
    #[test]
    fn wcag_disabled_text_is_exempt_but_measured() {
        let ctx = egui::Context::default();
        crate::app::style(&ctx);
        let v = ctx.global_style().visuals.clone();
        let alpha = v.disabled_alpha;
        let fg = v.widgets.noninteractive.fg_stroke.color;
        let got = contrast(over(fg, BOX, alpha), BOX);
        pin("disabled label over a panel", got, 2.28);
        assert!(
            got < 4.5,
            "disabled text is exempt, not readable: {got:.2}:1"
        );
    }
}
