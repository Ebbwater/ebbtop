//! btop-style painted widgets: line/area graphs and meters.

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

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

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Green -> amber -> red as `frac` goes 0 -> 1 (btop's default gradient).
pub fn heat(frac: f32) -> Color32 {
    if frac < 0.5 { lerp(OK, WARN, frac * 2.0) } else { lerp(WARN, BAD, (frac - 0.5) * 2.0) }
}

/// btop-like line + area graph, newest sample at the right edge. `slots`
/// spans the width so the time scale is fixed whatever the number of
/// samples held. Missing samples at the left stay empty. `color` None
/// uses `heat` per-point. Area is a translucent wash (grid shows
/// through), the value polyline is the bright top edge btop draws.
/// Hover paints a crosshair + value readout (allocation only while
/// hovered, plus one point Vec per graph — measured in `frame_cost`).
pub fn graph(ui: &mut Ui, vals: &[f64], slots: usize, max: f64, height: f32, color: Option<Color32>) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, BG);
    // 80/90% thresholds (phase 1) replace the old 25/75 grid; 50% stays.
    for f in [0.5f32, 0.8, 0.9] {
        let y = rect.bottom() - rect.height() * f;
        p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, LINE));
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
        p.rect_filled(Rect::from_min_max(pos2(x0, top), pos2(x0 + w + 0.6, rect.bottom())), 0.0, wash);
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
    if resp.hovered() && n > 0 {
        if let Some(ptr) = resp.interact_pointer_pos() {
            if rect.contains(ptr) && w > 0.0 {
                let slot = (((ptr.x - rect.left()) / w).floor() as usize).min(slots - 1);
                if slot >= slots - n {
                    let k = slot - (slots - n);
                    let v = vals[vals.len() - n + k];
                    let x = rect.left() + (slot as f32 + 0.5) * w;
                    p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, ACCENT));
                    let font = egui::TextStyle::Monospace.resolve(ui.style());
                    let txt = if max == 100.0 { format!("{v:.1}%") } else { format!("{v:.0}") };
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
        Pos2::new((rect.right() - right_w).max(rect.left() + label_w + 10.0), rect.bottom() - 3.0),
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

/// Titled box with btop-like border. Title is proportional (phase 1);
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
            let font = egui::TextStyle::Body.resolve(ui.style());
            let fmt = |color: Color32| egui::text::TextFormat { font_id: font.clone(), color, ..Default::default() };
            let mut job = egui::text::LayoutJob::default();
            job.append(&format!("{arrow}{title}"), 0.0, fmt(ACCENT));
            if let Some(m) = model.map(str::trim).filter(|m| !m.is_empty()) {
                job.append(" · ", 0.0, fmt(LINE));
                job.append(m, 0.0, fmt(DIM));
            }
            // Clickable label (not selectable_label): same height as the
            // old plain label, so headers don't push content out of the
            // scroll view at half-screen (see `renders_...` test).
            let resp = ui.add(
                egui::Label::new(job)
                    .truncate()
                    .sense(Sense::click()),
            );
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                *open = !*open;
            }
            if *open {
                ui.add_space(2.0);
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
    let pos = pos2(rect.right() - galley.size().x, rect.center().y - galley.size().y / 2.0);
    p.galley(pos, galley, TEXT);
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
}
