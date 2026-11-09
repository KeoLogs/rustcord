//! Vector icons for the voice UI, drawn with egui shapes (no emoji, no
//! icon font). Each takes the square `rect` to draw into and a colour; all
//! geometry scales with the rect, so they work at 16–24 px.

use std::f32::consts::PI;

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

/// Points along a circular arc (angles in radians, 0 = right, clockwise
/// on screen because y grows downwards).
fn arc(center: Pos2, r: f32, from: f32, to: f32, steps: usize) -> Vec<Pos2> {
    (0..=steps)
        .map(|i| {
            let a = from + (to - from) * i as f32 / steps as f32;
            center + vec2(a.cos(), a.sin()) * r
        })
        .collect()
}

fn line_w(rect: &Rect) -> f32 {
    (rect.width() / 11.0).max(1.5)
}

/// Microphone: capsule, cradle, stem and base.
pub fn microphone(p: &Painter, rect: Rect, color: Color32) {
    let w = rect.width();
    let c = rect.center();
    let lw = line_w(&rect);
    let cap = Rect::from_center_size(pos2(c.x, rect.top() + w * 0.32), vec2(w * 0.32, w * 0.5));
    p.rect_filled(cap, w * 0.16, color);
    let cradle_c = pos2(c.x, rect.top() + w * 0.42);
    p.add(Shape::line(
        arc(cradle_c, w * 0.27, 0.0, PI, 16),
        Stroke::new(lw, color),
    ));
    let stem_top = cradle_c.y + w * 0.27;
    p.line_segment(
        [pos2(c.x, stem_top), pos2(c.x, rect.top() + w * 0.88)],
        Stroke::new(lw, color),
    );
    p.line_segment(
        [
            pos2(c.x - w * 0.18, rect.top() + w * 0.88),
            pos2(c.x + w * 0.18, rect.top() + w * 0.88),
        ],
        Stroke::new(lw, color),
    );
}

/// Headphones: headband arc and two ear cups.
pub fn headphones(p: &Painter, rect: Rect, color: Color32) {
    let w = rect.width();
    let lw = line_w(&rect);
    let band_c = pos2(rect.center().x, rect.top() + w * 0.55);
    let r = w * 0.36;
    p.add(Shape::line(
        arc(band_c, r, PI, 2.0 * PI, 20),
        Stroke::new(lw, color),
    ));
    for side in [-1.0f32, 1.0] {
        let x = band_c.x + side * r;
        let cup = Rect::from_center_size(
            pos2(x - side * w * 0.04, band_c.y + w * 0.13),
            vec2(w * 0.2, w * 0.34),
        );
        p.rect_filled(cup, w * 0.07, color);
        p.line_segment(
            [pos2(x, band_c.y), pos2(x, band_c.y + w * 0.05)],
            Stroke::new(lw, color),
        );
    }
}

/// The diagonal "off" slash over an icon. `gap` is the background colour
/// drawn either side so the slash reads clearly on top of the icon.
pub fn slash(p: &Painter, rect: Rect, color: Color32, gap: Color32) {
    let w = rect.width();
    let a = rect.left_top() + vec2(w * 0.12, w * 0.12);
    let b = rect.right_bottom() - vec2(w * 0.12, w * 0.12);
    let lw = line_w(&rect);
    p.line_segment([a, b], Stroke::new(lw * 2.6, gap));
    p.line_segment([a, b], Stroke::new(lw * 1.2, color));
}

/// Phone handset tilted down: Discord's "Disconnect" button.
pub fn hangup(p: &Painter, rect: Rect, color: Color32) {
    let w = rect.width();
    let c = rect.center() + vec2(0.0, w * 0.12);
    let lw = w * 0.16;
    // The handset: a flat arc with thick rounded ends.
    let pts = arc(
        c + vec2(0.0, w * 0.32),
        w * 0.42,
        PI + 0.55,
        2.0 * PI - 0.55,
        16,
    );
    p.add(Shape::line(pts.clone(), Stroke::new(lw, color)));
    for end in [pts[0], pts[pts.len() - 1]] {
        let outward = if end.x < c.x { -1.0 } else { 1.0 };
        let ear = Rect::from_center_size(
            end + vec2(outward * w * 0.02, w * 0.06),
            vec2(w * 0.2, w * 0.2),
        );
        p.rect_filled(ear, w * 0.06, color);
    }
}

/// Speaker with sound waves: the voice channel icon.
pub fn speaker(p: &Painter, rect: Rect, color: Color32) {
    let w = rect.width();
    let l = rect.left() + w * 0.1;
    let cy = rect.center().y;
    let body = vec![
        pos2(l, cy - w * 0.13),
        pos2(l + w * 0.18, cy - w * 0.13),
        pos2(l + w * 0.42, cy - w * 0.34),
        pos2(l + w * 0.42, cy + w * 0.34),
        pos2(l + w * 0.18, cy + w * 0.13),
        pos2(l, cy + w * 0.13),
    ];
    p.add(Shape::convex_polygon(body, color, Stroke::NONE));
    let lw = line_w(&rect);
    let wc = pos2(l + w * 0.42, cy);
    for r in [w * 0.2, w * 0.36] {
        p.add(Shape::line(
            arc(wc, r, -0.85, 0.85, 10),
            Stroke::new(lw, color),
        ));
    }
}

/// Connection quality bars (three, rising), all in `color`.
pub fn signal_bars(p: &Painter, rect: Rect, color: Color32) {
    let w = rect.width();
    let bw = w * 0.2;
    let bottom = rect.bottom() - w * 0.12;
    for (i, h) in [0.3f32, 0.5, 0.72].iter().enumerate() {
        let x = rect.left() + w * 0.12 + i as f32 * (bw + w * 0.08);
        let r = Rect::from_min_max(pos2(x, bottom - w * h), pos2(x + bw, bottom));
        p.rect_filled(r, w * 0.04, color);
    }
}

/// Video camera (someone has their camera on).
pub fn camera(p: &Painter, rect: Rect, color: Color32) {
    let w = rect.width();
    let c = rect.center();
    let body = Rect::from_min_size(
        pos2(rect.left() + w * 0.06, c.y - w * 0.24),
        vec2(w * 0.6, w * 0.48),
    );
    p.rect_filled(body, w * 0.1, color);
    let lens = vec![
        pos2(body.right() + w * 0.04, c.y - w * 0.06),
        pos2(rect.right() - w * 0.04, c.y - w * 0.22),
        pos2(rect.right() - w * 0.04, c.y + w * 0.22),
        pos2(body.right() + w * 0.04, c.y + w * 0.06),
    ];
    p.add(Shape::convex_polygon(lens, color, Stroke::NONE));
}

/// Padlock: end-to-end encryption is on.
pub fn lock(p: &Painter, rect: Rect, color: Color32) {
    let w = rect.width();
    let c = rect.center();
    let body = Rect::from_min_max(
        pos2(c.x - w * 0.32, c.y - w * 0.04),
        pos2(c.x + w * 0.32, rect.bottom() - w * 0.08),
    );
    p.rect_filled(body, w * 0.08, color);
    p.add(Shape::line(
        arc(pos2(c.x, c.y - w * 0.04), w * 0.2, PI, 2.0 * PI, 12),
        Stroke::new(line_w(&rect) * 1.2, color),
    ));
    // Keep the shackle legs attached to the body.
    for side in [-1.0f32, 1.0] {
        p.line_segment(
            [
                pos2(c.x + side * w * 0.2, c.y - w * 0.04),
                pos2(c.x + side * w * 0.2, c.y + w * 0.02),
            ],
            Stroke::new(line_w(&rect) * 1.2, color),
        );
    }
}

/// Small pill with text, e.g. the red "LIVE" badge for Go Live.
pub fn pill(p: &Painter, rect: Rect, fill: Color32, text: &str, text_color: Color32) {
    p.rect_filled(rect, rect.height() / 2.0, fill);
    p.text(
        rect.center(),
        eframe::egui::Align2::CENTER_CENTER,
        text,
        crate::theme::bold_font(9.5),
        text_color,
    );
}

/// Green ring around a speaking user's avatar.
pub fn speaking_ring(p: &Painter, avatar: Rect, color: Color32) {
    p.circle_stroke(
        avatar.center(),
        avatar.width() / 2.0 + 1.0,
        Stroke::new(2.0, color),
    );
}

/// Rounded hover background for icon buttons.
pub fn button_bg(p: &Painter, rect: Rect, fill: Color32) {
    p.rect_filled(rect, 4.0, fill);
}

/// Thin outline (used for focus/hover rings in previews).
pub fn outline(p: &Painter, rect: Rect, color: Color32) {
    p.rect_stroke(rect, 4.0, Stroke::new(1.0, color), StrokeKind::Inside);
}
