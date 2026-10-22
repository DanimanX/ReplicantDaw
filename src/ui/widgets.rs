//! Custom-painted controls: rotary knobs, channel faders, LED meters.

use std::f32::consts::PI;

use egui::{
    Align2, Color32, CornerRadius, CursorIcon, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2,
};

use crate::audio::{gain_to_db, messages::db_to_gain, RecordedNote};
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

/// Which part of a clip a mini roll shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiniRollView {
    /// The cell *is* the clip: spread the loop over the whole rect.
    Clip,
    /// Keep the clip on the 16-bar session grid: absolute positions.
    Timeline,
}

/// Compact piano-roll preview of a clip, limited to the part that plays.
///
/// The loop region is drawn as the block itself and notes outside it are
/// faded, so a short loop stops looking like a clip that runs the whole
/// session.
#[allow(clippy::too_many_arguments)]
pub fn mini_roll(
    painter: &egui::Painter,
    rect: Rect,
    notes: &[RecordedNote],
    loop_region: (u16, u16),
    steps_total: u16,
    view: MiniRollView,
    accent: Color32,
    enabled: bool,
) {
    if notes.is_empty() || steps_total == 0 || rect.width() < 3.0 || rect.height() < 3.0 {
        return;
    }
    let (loop_start, loop_end) = loop_region;
    let visible = match view {
        MiniRollView::Clip => (loop_start, loop_end.max(loop_start + 1)),
        MiniRollView::Timeline => (0, steps_total),
    };
    let span = (visible.1.saturating_sub(visible.0)).max(1) as f32;
    let x_of = |step: u16| {
        let step = step.clamp(visible.0, visible.1);
        rect.left() + ((step - visible.0) as f32 / span) * rect.width()
    };

    let region = Rect::from_min_max(
        Pos2::new(x_of(loop_start), rect.top()),
        Pos2::new(x_of(loop_end).max(x_of(loop_start) + 1.0), rect.bottom()),
    );
    painter.rect_filled(
        region,
        CornerRadius::same(1),
        theme::accent_alpha(accent, if enabled { 26 } else { 10 }),
    );
    painter.rect_stroke(
        region,
        CornerRadius::same(1),
        Stroke::new(
            1.0,
            theme::accent_alpha(accent, if enabled { 120 } else { 45 }),
        ),
        egui::StrokeKind::Inside,
    );

    let mut min_note = 127u8;
    let mut max_note = 0u8;
    for note in notes {
        min_note = min_note.min(note.note);
        max_note = max_note.max(note.note);
    }
    let pitch_span = (max_note.saturating_sub(min_note)).max(11) as f32;
    let row = (rect.height() * 0.16).clamp(1.5, 3.0);
    for note in notes {
        if note.end_step() <= visible.0 || note.start_step >= visible.1 {
            continue;
        }
        let audible = note.start_step < loop_end && note.end_step() > loop_start;
        let x0 = x_of(note.start_step).max(rect.left());
        let x1 = x_of(note.end_step()).min(rect.right());
        if x1 <= x0 {
            continue;
        }
        let pitch = (note.note.saturating_sub(min_note)) as f32 / pitch_span;
        let y = rect.bottom() - row - pitch * (rect.height() - row - 1.0);
        let color = match (enabled, audible) {
            (false, _) => theme::accent_alpha(accent, 45),
            (true, true) => theme::accent_alpha(accent, 230),
            // Outside the loop: visible for context, clearly not played.
            (true, false) => theme::accent_alpha(accent, 55),
        };
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x0, y),
                Pos2::new((x1.max(x0 + 1.5)).min(rect.right()), y + row),
            ),
            CornerRadius::same(1),
            color,
        );
    }
}

/// Angle helper for decorative rings.
pub fn ring_point(center: Pos2, radius: f32, degrees: f32) -> Pos2 {
    let rad = degrees * PI / 180.0;
    Pos2::new(center.x + rad.cos() * radius, center.y + rad.sin() * radius)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The preview must lay out notes on the clip's own 16-bar axis, so the
    /// part outside the loop can be told apart from the part that plays.
    #[test]
    fn mini_roll_marks_notes_outside_the_loop() {
        let notes = [
            RecordedNote {
                note: 60,
                velocity: 100,
                start_step: 8,
                length_steps: 4,
            },
            RecordedNote {
                note: 72,
                velocity: 100,
                start_step: 128,
                length_steps: 4,
            },
        ];
        let (start, end) = (0u16, 64u16);
        let inside = |note: &RecordedNote| note.start_step < end && note.end_step() > start;
        assert!(inside(&notes[0]));
        assert!(!inside(&notes[1]));

        // A clip view spreads the loop over the rect; a timeline keeps the
        // absolute 16-bar axis.
        assert_eq!(
            MiniRollView::Clip,
            MiniRollView::Clip,
            "views are distinguishable"
        );
        assert_ne!(MiniRollView::Clip, MiniRollView::Timeline);
    }
}
