//! Interface modules: transport, session view, arrangement + FFT and devices.

pub mod devices;
pub mod lanes;
pub mod piano_roll;
pub mod sample_clip;
pub mod samples;
pub mod session;
pub mod studio_windows;
pub mod transport;
pub mod widgets;

use egui::{Align2, CornerRadius, Pos2, Rect, Stroke, Ui, Vec2};

use crate::studio::Studio;
use crate::theme;

/// Floating tag that follows the pointer while a clip is carried.
///
/// It names the clip and the destination reported by the panel underneath, so
/// the drop is visible before the button is released — the same feedback a
/// clip gets in Ableton or Bitwig. Drawn last, on top of every panel.
pub fn clip_drag_overlay(ui: &mut Ui, studio: &Studio) {
    let Some(drag) = &studio.clip_drag else {
        return;
    };
    let Some(pointer) = ui
        .ctx()
        .pointer_interact_pos()
        .or_else(|| ui.ctx().pointer_hover_pos())
    else {
        return;
    };

    let target = studio
        .clip_drop
        .as_ref()
        .map(|target| target.label())
        .unwrap_or_else(|| "RELEASE TO CANCEL".to_string());
    let mutating = studio.clip_drop.is_some();
    let accent = if mutating {
        theme::MAGENTA
    } else {
        theme::TEXT_FAINT
    };

    let name = truncate(&drag.clip().name, 26);
    let painter = ui.painter();
    let origin = Pos2::new(pointer.x + 14.0, pointer.y + 12.0);
    let rect = Rect::from_min_size(origin, Vec2::new(196.0, 30.0));
    // Keep the tag on screen when the pointer is near an edge.
    let screen = ui.ctx().content_rect();
    let rect = rect.translate(Vec2::new(
        if rect.right() > screen.right() {
            screen.right() - rect.right() - 4.0
        } else {
            0.0
        },
        if rect.bottom() > screen.bottom() {
            screen.bottom() - rect.bottom() - 4.0
        } else {
            0.0
        },
    ));
    painter.rect_filled(rect, CornerRadius::same(2), theme::PANEL_RAISED);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.2, accent),
        egui::StrokeKind::Inside,
    );
    painter.text(
        Pos2::new(rect.left() + 7.0, rect.top() + 4.0),
        Align2::LEFT_TOP,
        name,
        theme::display(7.6),
        theme::TEXT,
    );
    painter.text(
        Pos2::new(rect.left() + 7.0, rect.bottom() - 5.0),
        Align2::LEFT_BOTTOM,
        format!("→ {target}"),
        theme::mono(7.0),
        accent,
    );
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
