//! Top transport bar: brand, play/stop, musical position, tempo, CPU and link
//! status.

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::audio::{gain_to_db, AudioRuntime, Command};
use crate::state::UiState;
use crate::theme;
use crate::ui::widgets::led;

pub fn draw(ui: &mut Ui, rect: Rect, state: &mut UiState, audio: &mut AudioRuntime) {
    let painter = ui.painter().clone();
    painter.rect_filled(rect, CornerRadius::same(3), theme::PANEL);
    painter.rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );
    // Accent hairline under the bar.
    painter.line_segment(
        [
            rect.left_bottom(),
            Pos2::new(rect.left() + rect.width() * 0.33, rect.bottom()),
        ],
        Stroke::new(1.0, theme::accent_alpha(theme::CYAN, 150)),
    );
    painter.line_segment(
        [
            Pos2::new(rect.left() + rect.width() * 0.33, rect.bottom()),
            rect.right_bottom(),
        ],
        Stroke::new(1.0, theme::accent_alpha(theme::MAGENTA, 110)),
    );

    let mid = rect.center().y;
    let mut x = rect.left() + 14.0;

    // ---- Brand ---------------------------------------------------------
    let logo = Pos2::new(x + 10.0, mid);
    painter.circle_stroke(
        logo,
        11.0,
        Stroke::new(1.0, theme::accent_alpha(theme::CYAN, 120)),
    );
    crate::ui::widgets::diamond(&painter, logo, 7.0, theme::CYAN, true);
    painter.circle_filled(logo, 2.0, theme::MAGENTA);
    x += 30.0;

    painter.text(
        Pos2::new(x, mid - 7.0),
        Align2::LEFT_CENTER,
        "REPLICANT",
        theme::display(16.0),
        theme::CYAN,
    );
    painter.text(
        Pos2::new(x + 1.0, mid + 10.0),
        Align2::LEFT_CENTER,
        "DAW // 2035",
        theme::mono(9.0),
        theme::TEXT_DIM,
    );
    x += 138.0;

    divider(ui, x, rect.top() + 10.0, rect.bottom() - 10.0);
    x += 12.0;

    // ---- Play / Stop ---------------------------------------------------
    let play_rect = Rect::from_center_size(Pos2::new(x + 17.0, mid), Vec2::splat(34.0));
    let play = ui.interact(
        play_rect,
        ui.scope_id().with("transport_play"),
        Sense::click(),
    );
    let play_fill = if state.playing {
        theme::accent_alpha(theme::CYAN, 35)
    } else if play.hovered() {
        theme::PANEL_RAISED
    } else {
        theme::PANEL_SUNK
    };
    painter.rect_filled(play_rect, CornerRadius::same(2), play_fill);
    painter.rect_stroke(
        play_rect,
        CornerRadius::same(2),
        Stroke::new(
            1.0,
            if state.playing || play.hovered() {
                theme::CYAN
            } else {
                theme::accent_alpha(theme::CYAN, 90)
            },
        ),
        egui::StrokeKind::Inside,
    );
    let tri = vec![
        Pos2::new(play_rect.center().x - 5.0, play_rect.center().y - 7.0),
        Pos2::new(play_rect.center().x + 8.0, play_rect.center().y),
        Pos2::new(play_rect.center().x - 5.0, play_rect.center().y + 7.0),
    ];
    painter.add(egui::Shape::convex_polygon(
        tri,
        if state.playing {
            Color32::WHITE
        } else {
            theme::CYAN
        },
        Stroke::NONE,
    ));
    if play.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if play.clicked() {
        let _ = audio.command_tx.push(Command::Play);
        state.playing = true;
    }

    let stop_rect = Rect::from_center_size(Pos2::new(x + 50.0, mid), Vec2::splat(28.0));
    let stop = ui.interact(
        stop_rect,
        ui.scope_id().with("transport_stop"),
        Sense::click(),
    );
    painter.rect_filled(
        stop_rect,
        CornerRadius::same(2),
        if stop.hovered() {
            theme::PANEL_RAISED
        } else {
            theme::PANEL_SUNK
        },
    );
    painter.rect_stroke(
        stop_rect,
        CornerRadius::same(2),
        Stroke::new(
            1.0,
            if stop.hovered() {
                theme::MAGENTA
            } else {
                theme::BORDER
            },
        ),
        egui::StrokeKind::Inside,
    );
    painter.rect_filled(
        Rect::from_center_size(stop_rect.center(), Vec2::splat(9.0)),
        CornerRadius::same(1),
        if stop.hovered() {
            theme::MAGENTA
        } else {
            theme::TEXT_DIM
        },
    );
    if stop.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if stop.clicked() {
        let _ = audio.command_tx.push(Command::Stop);
        state.playing = false;
        state.position_frames = 0;
    }

    // Record arm / capture.
    let rec_rect = Rect::from_center_size(Pos2::new(x + 78.0, mid), Vec2::new(44.0, 26.0));
    let rec = ui.interact(
        rec_rect,
        ui.scope_id().with("transport_rec"),
        Sense::click(),
    );
    let rec_hover = rec.hovered();
    painter.rect_filled(
        rec_rect,
        CornerRadius::same(2),
        if state.recording {
            theme::accent_alpha(theme::RED, 40)
        } else if rec_hover {
            theme::PANEL_RAISED
        } else {
            theme::PANEL_SUNK
        },
    );
    painter.rect_stroke(
        rec_rect,
        CornerRadius::same(2),
        Stroke::new(
            1.0,
            if state.recording {
                theme::RED
            } else if rec_hover {
                theme::accent_alpha(theme::RED, 150)
            } else {
                theme::BORDER
            },
        ),
        egui::StrokeKind::Inside,
    );
    painter.circle_filled(
        Pos2::new(rec_rect.left() + 10.0, rec_rect.center().y),
        3.5,
        if state.recording {
            theme::RED
        } else {
            theme::accent_alpha(theme::RED, 160)
        },
    );
    painter.text(
        Pos2::new(rec_rect.left() + 18.0, rec_rect.center().y),
        Align2::LEFT_CENTER,
        "REC",
        theme::display(8.0),
        if state.recording {
            theme::RED
        } else {
            theme::TEXT_DIM
        },
    );
    if rec_hover {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if rec.clicked() {
        if state.recording {
            let _ = audio.command_tx.push(Command::RecordStop);
            state.recording = false;
        } else {
            let track = state.selected_track as u8;
            let scene = state.launched[track as usize].unwrap_or(0);
            state.clear_recorded(track as usize, scene as usize);
            state.recording = true;
            state.record_track = track;
            let _ = audio.command_tx.push(Command::RecordStart { track, scene });
        }
    }
    if state.recording && rec_rect.right() < rect.right() - 10.0 {
        painter.text(
            Pos2::new(rec_rect.left(), rect.bottom() - 8.0),
            Align2::LEFT_BOTTOM,
            format!(
                "TRK {:02} · {} NOTES",
                state.record_track + 1,
                state.recorded_total
            ),
            theme::mono(7.0),
            theme::accent_alpha(theme::RED, 200),
        );
    }
    x += 108.0;

    divider(ui, x, rect.top() + 10.0, rect.bottom() - 10.0);
    x += 12.0;

    // ---- Musical position ---------------------------------------------
    let (bar, beat, step, _) = state.transport_display();
    painter.text(
        Pos2::new(x, rect.top() + 8.0),
        Align2::LEFT_TOP,
        "POSITION  BAR . BEAT . STEP",
        theme::mono(7.5),
        theme::TEXT_FAINT,
    );
    painter.text(
        Pos2::new(x, mid + 3.0),
        Align2::LEFT_CENTER,
        format!("{bar:02}.{beat:02}.{step:02}"),
        theme::mono(23.0),
        if state.playing {
            theme::CYAN
        } else {
            theme::TEXT
        },
    );
    painter.text(
        Pos2::new(x, rect.bottom() - 7.0),
        Align2::LEFT_BOTTOM,
        format!(
            "LOOP 1-16   {:02}:{:05.2}",
            (state.elapsed_secs() as u64) / 60,
            state.elapsed_secs() % 60.0
        ),
        theme::mono(7.5),
        theme::TEXT_FAINT,
    );
    x += 152.0;

    divider(ui, x, rect.top() + 10.0, rect.bottom() - 10.0);
    x += 12.0;

    // ---- Tempo ---------------------------------------------------------
    painter.text(
        Pos2::new(x, rect.top() + 8.0),
        Align2::LEFT_TOP,
        "TEMPO · 4/4",
        theme::mono(7.5),
        theme::TEXT_FAINT,
    );
    let bpm_rect = Rect::from_min_size(Pos2::new(x, mid - 4.0), Vec2::new(84.0, 26.0));
    painter.rect_filled(bpm_rect, CornerRadius::same(2), theme::PANEL_SUNK);
    painter.rect_stroke(
        bpm_rect,
        CornerRadius::same(2),
        Stroke::new(1.0, theme::accent_alpha(theme::CYAN, 55)),
        egui::StrokeKind::Inside,
    );
    let mut bpm = state.bpm;
    ui.scope(|ui| {
        ui.style_mut().override_font_id = Some(theme::mono(20.0));
        ui.visuals_mut().override_text_color = Some(theme::CYAN);
        ui.put(
            bpm_rect.shrink2(Vec2::new(8.0, 1.0)),
            egui::DragValue::new(&mut bpm)
                .range(40.0..=240.0)
                .speed(0.2)
                .fixed_decimals(2)
                .update_while_editing(true),
        );
    });
    if (bpm - state.bpm).abs() > f32::EPSILON {
        state.bpm = bpm;
        let _ = audio.command_tx.push(Command::SetBpm(bpm));
    }
    let minus = Rect::from_center_size(
        Pos2::new(bpm_rect.right() + 12.0, mid + 2.0),
        Vec2::new(16.0, 20.0),
    );
    let minus_resp = ui.interact(minus, ui.scope_id().with("bpm_minus"), Sense::click());
    let plus = Rect::from_center_size(
        Pos2::new(bpm_rect.right() + 31.0, mid + 2.0),
        Vec2::new(16.0, 20.0),
    );
    let plus_resp = ui.interact(plus, ui.scope_id().with("bpm_plus"), Sense::click());
    for (r, label, active) in [
        (minus, "-", minus_resp.hovered()),
        (plus, "+", plus_resp.hovered()),
    ] {
        painter.rect_filled(
            r,
            CornerRadius::same(2),
            if active {
                theme::PANEL_RAISED
            } else {
                theme::PANEL_SUNK
            },
        );
        painter.rect_stroke(
            r,
            CornerRadius::same(2),
            Stroke::new(1.0, if active { theme::CYAN } else { theme::BORDER }),
            egui::StrokeKind::Inside,
        );
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            label,
            theme::mono(12.0),
            if active { theme::CYAN } else { theme::TEXT_DIM },
        );
    }
    if minus_resp.clicked() {
        state.bpm = (state.bpm - 1.0).max(40.0);
        let _ = audio.command_tx.push(Command::SetBpm(state.bpm));
    }
    if plus_resp.clicked() {
        state.bpm = (state.bpm + 1.0).min(240.0);
        let _ = audio.command_tx.push(Command::SetBpm(state.bpm));
    }
    x += 136.0;

    divider(ui, x, rect.top() + 10.0, rect.bottom() - 10.0);
    x += 12.0;

    // ---- CPU meter -----------------------------------------------------
    painter.text(
        Pos2::new(x, rect.top() + 8.0),
        Align2::LEFT_TOP,
        "Q-CPU LOAD",
        theme::mono(7.5),
        theme::TEXT_FAINT,
    );
    let cpu_color = if state.cpu > 70.0 {
        theme::RED
    } else if state.cpu > 40.0 {
        theme::AMBER
    } else {
        theme::CYAN
    };
    painter.text(
        Pos2::new(x, mid),
        Align2::LEFT_CENTER,
        format!("{:05.2}%", state.cpu),
        theme::mono(17.0),
        cpu_color,
    );
    let cpu_bar = Rect::from_min_size(Pos2::new(x, rect.bottom() - 17.0), Vec2::new(104.0, 5.0));
    painter.rect_filled(cpu_bar, CornerRadius::same(1), theme::BG_DEEP);
    let fraction = (state.cpu / 100.0).clamp(0.0, 1.0);
    if fraction > 0.0 {
        painter.rect_filled(
            Rect::from_min_size(
                cpu_bar.min,
                Vec2::new(cpu_bar.width() * fraction, cpu_bar.height()),
            ),
            CornerRadius::same(1),
            cpu_color,
        );
    }
    x += 116.0;
    let status_right = rect.right() - 14.0;
    let status_left = (status_right - 300.0).max(x + 8.0);
    let meter_space = status_left - x - 18.0;

    // ---- Master output (fills the space before the status block) --------
    if meter_space > 90.0 {
        let width = meter_space.min(380.0);
        let meter_x = x + (meter_space - width) * 0.5;
        let master_db = state.master_db();
        let meter_color = if master_db > -2.0 {
            theme::RED
        } else if master_db > -10.0 {
            theme::AMBER
        } else {
            theme::GREEN
        };
        painter.text(
            Pos2::new(meter_x, rect.top() + 8.0),
            Align2::LEFT_TOP,
            "MASTER OUT",
            theme::mono(7.5),
            theme::TEXT_FAINT,
        );
        painter.text(
            Pos2::new(meter_x + width, rect.top() + 8.0),
            Align2::RIGHT_TOP,
            format!("{master_db:+.1} dBFS"),
            theme::mono(8.0),
            meter_color,
        );
        for channel in 0..2 {
            let bar = Rect::from_min_size(
                Pos2::new(meter_x, mid + channel as f32 * 7.0 - 3.0),
                Vec2::new(width, 4.0),
            );
            painter.rect_filled(bar, CornerRadius::same(1), theme::BG_DEEP);
            let t = ((gain_to_db(state.master_peaks[channel].max(1.0e-7)) + 60.0) / 60.0)
                .clamp(0.0, 1.0);
            painter.rect_filled(
                Rect::from_min_size(bar.min, Vec2::new(bar.width() * t, bar.height())),
                CornerRadius::same(1),
                meter_color,
            );
            painter.rect_filled(
                Rect::from_min_size(bar.min, Vec2::new(bar.width() * t, bar.height())),
                CornerRadius::same(1),
                theme::accent_alpha(meter_color, 45),
            );
        }
        painter.text(
            Pos2::new(meter_x, rect.bottom() - 8.0),
            Align2::LEFT_BOTTOM,
            "L / R",
            theme::mono(6.0),
            theme::TEXT_FAINT,
        );
    }

    // ---- Status (right aligned) ---------------------------------------
    let online = !audio.is_silent();
    let link_color = if online { theme::GREEN } else { theme::AMBER };
    painter.text(
        Pos2::new(status_left + 12.0, rect.top() + 10.0),
        Align2::LEFT_TOP,
        if online {
            "NEURAL LINK · STABLE"
        } else {
            "NEURAL LINK · SILENT FALLBACK"
        },
        theme::mono(8.5),
        link_color,
    );
    painter.text(
        Pos2::new(status_left + 12.0, rect.top() + 23.0),
        Align2::LEFT_TOP,
        "AI CO-PILOT · ACTIVE",
        theme::mono(8.5),
        theme::MAGENTA,
    );
    painter.text(
        Pos2::new(status_right, rect.bottom() - 8.0),
        Align2::RIGHT_BOTTOM,
        format!(
            "{} · {} kHz · 32-bit float · {} voices",
            audio.backend.to_uppercase(),
            audio.sample_rate / 1000,
            state.voices
        ),
        theme::mono(7.5),
        theme::TEXT_FAINT,
    );
    led(
        ui,
        Pos2::new(status_left + 4.0, rect.top() + 14.0),
        2.0,
        link_color,
        online,
    );
    led(
        ui,
        Pos2::new(status_left + 4.0, rect.top() + 27.0),
        2.0,
        theme::MAGENTA,
        true,
    );

    // Voice activity strip near the right edge.
    let strip = Rect::from_min_max(
        Pos2::new(rect.right() - 8.0, rect.top() + 12.0),
        Pos2::new(rect.right() - 5.0, rect.bottom() - 12.0),
    );
    painter.rect_filled(strip, CornerRadius::same(1), theme::BG_DEEP);
    let lit = (state.voices as f32 / crate::audio::MAX_VOICES as f32).clamp(0.0, 1.0);
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(strip.left(), strip.bottom() - strip.height() * lit),
            strip.right_bottom(),
        ),
        CornerRadius::same(1),
        theme::CYAN,
    );
}

fn divider(ui: &Ui, x: f32, top: f32, bottom: f32) {
    ui.painter().line_segment(
        [Pos2::new(x, top), Pos2::new(x, bottom)],
        Stroke::new(1.0, theme::BORDER_SOFT),
    );
}
