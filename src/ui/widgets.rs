//! Custom-painted controls: rotary knobs, channel faders, LED meters.

use std::f32::consts::PI;

use egui::{
    Align2, Color32, CornerRadius, CursorIcon, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2,
};

use crate::audio::{gain_to_db, messages::db_to_gain};
use crate::theme;

fn arc(center: Pos2, radius: f32, start_deg: f32, end_deg: f32, segments: usize) -> Vec<Pos2> {
    (0..=segments)
        .map(|i| {
            let t = i as f32 / segments as f32;
            let angle = (start_deg + (end_deg - start_deg) * t).to_radians();
            Pos2::new(
                center.x + angle.cos() * radius,
                center.y + angle.sin() * radius,
            )
        })
        .collect()
}

fn polyline(ui: &Ui, points: Vec<Pos2>, stroke: Stroke) {
    if points.len() < 2 {
        return;
    }
    ui.painter().add(egui::Shape::line(points, stroke));
}

/// Rotary knob. Returns `true` when the value changed.
#[allow(clippy::too_many_arguments)]
pub fn knob(
    ui: &mut Ui,
    rect: Rect,
    id: egui::Id,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    label: &str,
    value_text: &str,
    accent: Color32,
) -> bool {
    let min = *range.start();
    let max = *range.end();
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let mut changed = false;

    if response.dragged() {
        let delta = ui.input(|i| i.pointer.delta().y);
        if delta != 0.0 {
            let step = (max - min) / 160.0;
            *value = (*value - delta * step).clamp(min, max);
            changed = true;
        }
    }
    if response.double_clicked() {
        *value = default;
        changed = true;
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }

    let painter = ui.painter();
    let cx = rect.center().x;
    let compact = rect.height() < 60.0;
    let label_h = if compact { 9.0 } else { 13.0 };
    let value_h = if compact { 10.0 } else { 13.0 };
    let top = rect.top() + label_h;
    let bottom = rect.bottom() - value_h;
    let diameter = (bottom - top).min(rect.width()).max(18.0);
    let radius = diameter * 0.5 - 2.0;
    let center = Pos2::new(cx, (top + bottom) * 0.5);

    painter.text(
        Pos2::new(cx, rect.top() + 2.0),
        Align2::CENTER_TOP,
        label,
        theme::display(if compact { 6.0 } else { 7.8 }),
        theme::TEXT_DIM,
    );
    painter.text(
        Pos2::new(cx, rect.bottom() - 1.0),
        Align2::CENTER_BOTTOM,
        value_text,
        theme::mono(if compact { 7.0 } else { 9.0 }),
        accent,
    );

    let hovered = response.hovered();
    painter.circle_filled(
        center,
        radius + 3.0,
        Color32::from_rgba_unmultiplied(
            accent.r(),
            accent.g(),
            accent.b(),
            if hovered { 16 } else { 6 },
        ),
    );
    painter.circle_filled(center, radius, theme::PANEL_SUNK);
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(
            1.0,
            if hovered {
                theme::accent_alpha(accent, 150)
            } else {
                theme::BORDER
            },
        ),
    );

    // Ticks.
    for i in 0..=10 {
        let angle = (135.0 + 27.0 * i as f32).to_radians();
        let inner = radius + 3.0;
        let outer = if i % 5 == 0 {
            radius + 7.5
        } else {
            radius + 6.0
        };
        let dir = Vec2::new(angle.cos(), angle.sin());
        painter.line_segment(
            [center + dir * inner, center + dir * outer],
            Stroke::new(
                1.0,
                if i % 5 == 0 {
                    theme::TEXT_FAINT
                } else {
                    theme::BORDER_SOFT
                },
            ),
        );
    }

    let fraction = ((*value - min) / (max - min)).clamp(0.0, 1.0);
    polyline(
        ui,
        arc(center, radius - 3.0, 135.0, 405.0, 48),
        Stroke::new(3.0, theme::PANEL_RAISED),
    );
    if fraction > 0.001 {
        let points = arc(center, radius - 3.0, 135.0, 135.0 + 270.0 * fraction, 48);
        polyline(
            ui,
            points.clone(),
            Stroke::new(6.0, theme::accent_alpha(accent, 28)),
        );
        polyline(ui, points, Stroke::new(2.2, accent));
    }

    // Pointer.
    let pointer_angle = (135.0 + 270.0 * fraction).to_radians();
    let dir = Vec2::new(pointer_angle.cos(), pointer_angle.sin());
    painter.line_segment(
        [
            center + dir * (radius * 0.30),
            center + dir * (radius * 0.78),
        ],
        Stroke::new(2.0, if hovered { Color32::WHITE } else { accent }),
    );
    painter.circle_filled(center, 2.2, accent);

    changed
}

/// Vertical channel fader. `db` is in decibels, range -60..=+6.
pub fn fader(ui: &mut Ui, rect: Rect, id: egui::Id, db: &mut f32, accent: Color32) -> bool {
    const MIN_DB: f32 = -60.0;
    const MAX_DB: f32 = 6.0;
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let mut changed = false;

    let usable = rect.shrink2(Vec2::new(0.0, 7.0));
    if response.dragged() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let t = ((usable.bottom() - pointer.y) / usable.height()).clamp(0.0, 1.0);
            *db = MIN_DB + t * (MAX_DB - MIN_DB);
            changed = true;
        }
    }
    if response.double_clicked() {
        *db = 0.0;
        changed = true;
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }

    let painter = ui.painter();
    let cx = rect.center().x;
    let top = usable.top();
    let bottom = usable.bottom();
    let slot = Rect::from_min_max(Pos2::new(cx - 2.0, top), Pos2::new(cx + 2.0, bottom));
    painter.rect_filled(slot, CornerRadius::same(2), theme::BG_DEEP);
    painter.rect_stroke(
        slot,
        CornerRadius::same(2),
        Stroke::new(1.0, theme::BORDER_SOFT),
        egui::StrokeKind::Inside,
    );

    let t = ((*db - MIN_DB) / (MAX_DB - MIN_DB)).clamp(0.0, 1.0);
    let y = bottom - t * (bottom - top);
    painter.line_segment(
        [Pos2::new(cx, bottom), Pos2::new(cx, y)],
        Stroke::new(2.0, theme::accent_alpha(accent, 160)),
    );

    // Scale ticks.
    for db_tick in [-60.0, -48.0, -36.0, -24.0, -12.0, 0.0, 6.0] {
        let tt = (db_tick - MIN_DB) / (MAX_DB - MIN_DB);
        let ty = bottom - tt * (bottom - top);
        painter.line_segment(
            [Pos2::new(cx - 6.0, ty), Pos2::new(cx + 6.0, ty)],
            Stroke::new(1.0, theme::BORDER),
        );
    }

    let handle = Rect::from_center_size(Pos2::new(cx, y), Vec2::new(rect.width().min(24.0), 10.0));
    painter.rect_filled(handle, CornerRadius::same(2), theme::PANEL_RAISED);
    painter.rect_stroke(
        handle,
        CornerRadius::same(2),
        Stroke::new(
            1.0,
            if response.hovered() {
                Color32::WHITE
            } else {
                accent
            },
        ),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [
            Pos2::new(handle.left() + 3.0, y),
            Pos2::new(handle.right() - 3.0, y),
        ],
        Stroke::new(1.0, accent),
    );

    changed
}

/// LED-segment level meter. `peak` is a linear magnitude.
pub fn meter(ui: &mut Ui, rect: Rect, peak: f32) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(1), theme::BG_DEEP);

    let db = gain_to_db(peak.max(1.0e-7));
    let t = ((db + 60.0) / 66.0).clamp(0.0, 1.0);
    let segments = ((rect.height() / 4.0).floor() as usize).max(6);
    let lit = (t * segments as f32).ceil() as usize;

    for i in 0..segments {
        let y0 = rect.bottom() - (i as f32 + 0.15) * rect.height() / segments as f32;
        let y1 = rect.bottom() - (i as f32 + 0.82) * rect.height() / segments as f32;
        let seg = Rect::from_min_max(
            Pos2::new(rect.left() + 1.0, y0.min(y1)),
            Pos2::new(rect.right() - 1.0, y0.max(y1)),
        );
        let frac = i as f32 / segments as f32;
        let color = if frac > 0.92 {
            theme::RED
        } else if frac > 0.80 {
            theme::AMBER
        } else {
            theme::GREEN
        };
        let fill = if i < lit {
            color
        } else {
            Color32::from_rgb(0x0e, 0x1d, 0x22)
        };
        painter.rect_filled(seg, CornerRadius::same(1), fill);
    }
    if lit > 0 {
        let top_y = rect.bottom() - lit as f32 / segments as f32 * rect.height();
        let glow = Color32::from_rgba_unmultiplied(
            theme::GREEN.r(),
            theme::GREEN.g(),
            theme::GREEN.b(),
            60,
        );
        painter.line_segment(
            [
                Pos2::new(rect.left(), top_y),
                Pos2::new(rect.right(), top_y),
            ],
            Stroke::new(2.0, glow),
        );
    }
}

/// Small tactile toggle used for MUTE / SOLO / SCENE actions.
pub fn toggle_chip(
    ui: &mut Ui,
    rect: Rect,
    id: egui::Id,
    label: &str,
    active: bool,
    accent: Color32,
) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    let hovered = response.hovered();
    let fill = if active {
        theme::accent_alpha(accent, 45)
    } else if hovered {
        theme::PANEL_RAISED
    } else {
        theme::PANEL_SUNK
    };
    let border = if active {
        accent
    } else if hovered {
        theme::accent_alpha(accent, 110)
    } else {
        theme::BORDER
    };
    ui.painter().rect_filled(rect, CornerRadius::same(2), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::display(8.0),
        if active { accent } else { theme::TEXT_DIM },
    );
    response
}

/// Draw a small status LED, including a soft halo when lit.
pub fn led(ui: &Ui, center: Pos2, radius: f32, color: Color32, lit: bool) {
    let painter = ui.painter();
    if lit {
        painter.circle_filled(center, radius * 2.6, theme::accent_alpha(color, 35));
        painter.circle_filled(center, radius, color);
    } else {
        painter.circle_filled(center, radius, theme::BORDER);
    }
}

/// dB scale label for the mixer.
pub fn db_text(db: f32) -> String {
    if db <= -60.0 {
        "-inf".to_string()
    } else {
        format!("{db:+.1}")
    }
}

/// Convert a fader dB setting to the linear gain used by the engine.
pub fn fader_gain(db: f32) -> f32 {
    db_to_gain(db)
}

/// Draw a rotated rectangle (used by the logo mark).
pub fn diamond(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32, stroke: bool) {
    let pts = vec![
        Pos2::new(center.x, center.y - radius),
        Pos2::new(center.x + radius, center.y),
        Pos2::new(center.x, center.y + radius),
        Pos2::new(center.x - radius, center.y),
    ];
    if stroke {
        painter.add(egui::Shape::closed_line(pts, Stroke::new(1.0, color)));
    } else {
        painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
    }
}

/// Angle helper for decorative rings.
pub fn ring_point(center: Pos2, radius: f32, degrees: f32) -> Pos2 {
    let rad = degrees * PI / 180.0;
    Pos2::new(center.x + rad.cos() * radius, center.y + rad.sin() * radius)
}
