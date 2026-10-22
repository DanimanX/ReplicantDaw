//! Clip-level editor for a sample slot: loop region, slot trim and launch.
//!
//! This is what a double-click on a sample cell opens. It treats the slot as
//! an audio clip that can loop, rather than as a file to load again; the
//! source sampler stays one button away.

use egui::{Align2, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::audio::{AudioRuntime, Command, LOOP_STEPS, TRACK_COUNT};
use crate::samples::SampleSlot;
use crate::state::UiState;
use crate::studio::Studio;
use crate::theme;

const STEPS: f32 = LOOP_STEPS as f32;

pub fn draw(
    ctx: &egui::Context,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
) {
    if !studio.sample_clip_open {
        return;
    }
    let (track, scene) = studio.sample_clip_target;
    let track = track.min(TRACK_COUNT - 1);
    let scene = scene.min(crate::audio::SCENE_COUNT - 1);
    let Some(slot) = state.sample_slot(track, scene).cloned() else {
        // The slot was cleared while the editor was open.
        studio.sample_clip_open = false;
        return;
    };

    let mut open = true;
    let screen = ctx.content_rect();
    let width = (screen.width() * 0.46).clamp(560.0, 820.0);
    let x = (screen.right() - width - 8.0).max(8.0);
    let accent = theme::MAGENTA;

    let mut loop_enabled = slot.loop_enabled;
    let mut gain_db = slot.gain_db;
    let mut launch = false;
    let mut stop = false;
    let mut edit_source = false;
    let mut clear = false;

    egui::Window::new(format!(
        "SAMPLE CLIP // TRACK {:02} · SLOT {:02}",
        track + 1,
        scene + 1
    ))
    .open(&mut open)
    .frame(
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::accent_alpha(accent, 150)))
            .corner_radius(CornerRadius::same(3))
            .inner_margin(8),
    )
    .default_pos(Pos2::new(x, 128.0))
    .default_size(Vec2::new(width, 520.0))
    .resizable(true)
    .show(ctx, |ui| {
        ui.set_width(width - 24.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(truncate(&slot.clip.name, 28))
                    .font(theme::display(10.0))
                    .color(accent),
            );
            ui.label(
                RichText::new(format!(
                    "{} · {:.2} s · {} Hz",
                    slot.clip.extension(),
                    slot.clip.duration_secs(),
                    slot.clip.buffer.sample_rate
                ))
                .font(theme::mono(7.0))
                .color(theme::TEXT_DIM),
            );
        });
        ui.add_space(4.0);

        draw_region(ui, state, track, scene, &slot);

        ui.add_space(5.0);
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new("LOOP").font(theme::display(8.5)))
                        .fill(if loop_enabled {
                            theme::accent_alpha(theme::CYAN, 45)
                        } else {
                            theme::PANEL_SUNK
                        })
                        .stroke(Stroke::new(
                            1.0,
                            if loop_enabled {
                                theme::CYAN
                            } else {
                                theme::BORDER
                            },
                        )),
                )
                .on_hover_text("Repeat the sample to fill the clip loop")
                .clicked()
            {
                loop_enabled = !loop_enabled;
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("LAUNCH").font(theme::display(8.5)))
                        .fill(theme::accent_alpha(theme::GREEN, 35)),
                )
                .clicked()
            {
                launch = true;
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("STOP").font(theme::display(8.5)))
                        .fill(theme::PANEL_SUNK),
                )
                .clicked()
            {
                stop = true;
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("EDIT SOURCE").font(theme::display(8.0)))
                        .fill(theme::PANEL_SUNK),
                )
                .on_hover_text("Open the sample in the source editor to trim, fade or reverse it")
                .clicked()
            {
                edit_source = true;
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("CLEAR").font(theme::display(8.0)))
                        .fill(theme::PANEL_SUNK),
                )
                .clicked()
            {
                clear = true;
            }
        });

        ui.add_space(4.0);
        ui.spacing_mut().slider_width = (width - 260.0).clamp(160.0, 420.0);
        ui.add(
            egui::Slider::new(&mut gain_db, -24.0..=12.0)
                .text("SLOT GAIN dB")
                .fixed_decimals(1),
        );
        ui.label(
            RichText::new(
                "The loop region sets the clip length: when it wraps, the sample restarts. LOOP repeats the sample inside that region.",
            )
            .font(theme::mono(6.6))
            .color(theme::TEXT_FAINT),
        );
    });

    if loop_enabled != slot.loop_enabled {
        studio.set_sample_loop(state, audio, track, scene, loop_enabled);
    }
    if (gain_db - slot.gain_db).abs() > 1e-4 {
        studio.set_sample_gain(state, audio, track, scene, gain_db);
    }
    if launch {
        let _ = audio.command_tx.push(Command::LaunchClip {
            track: track as u8,
            scene: scene as u8,
        });
        state.launched[track] = Some(scene as u8);
    }
    if stop {
        let _ = audio
            .command_tx
            .push(Command::StopClip { track: track as u8 });
        state.launched[track] = None;
    }
    if edit_source {
        studio.open_sample_editor_clip(&slot.clip);
    }
    if clear {
        studio.clear_sample(state, audio, track, scene);
        studio.sample_clip_open = false;
    }
    if !open {
        studio.sample_clip_open = false;
    }
}

/// The 16-bar ruler with the clip region, its handles and the waveform.
fn draw_region(
    ui: &mut egui::Ui,
    state: &mut UiState,
    track: usize,
    scene: usize,
    slot: &SampleSlot,
) {
    let height = 190.0;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if rect.width() < 40.0 {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(2), theme::BG_DEEP);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, theme::BORDER_SOFT),
        egui::StrokeKind::Inside,
    );

    let (loop_start, loop_end) = state.clip_loop(track, scene);
    let x_of = |step: f32| rect.left() + (step / STEPS).clamp(0.0, 1.0) * rect.width();
    let start_x = x_of(loop_start as f32);
    let end_x = x_of(loop_end as f32).max(start_x + 1.0);

    // Bar grid.
    for bar in 0..=16 {
        let x = x_of(bar as f32 * 16.0);
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(
                if bar % 4 == 0 { 1.0 } else { 0.5 },
                if bar % 4 == 0 {
                    theme::accent_alpha(theme::BORDER, 170)
                } else {
                    theme::BORDER_SOFT
                },
            ),
        );
        if bar < 16 {
            painter.text(
                Pos2::new(x + 3.0, rect.top() + 2.0),
                Align2::LEFT_TOP,
                format!("{}", bar + 1),
                theme::mono(5.8),
                theme::TEXT_FAINT,
            );
        }
    }

    // Inside the region: the waveform, repeated when looping so the clip
    // reads as the audio that actually runs.
    let wave_top = rect.top() + 12.0;
    let wave = Rect::from_min_max(
        Pos2::new(start_x, wave_top),
        Pos2::new(end_x, rect.bottom() - 4.0),
    );
    if wave.width() > 4.0 && wave.height() > 6.0 {
        let peaks = &slot.clip.peaks;
        if !peaks.is_empty() {
            let passes = if slot.loop_enabled {
                // The sample repeats to fill the region: how many times.
                let region_secs = region_seconds(state, loop_start, loop_end);
                let sample_secs = slot.clip.duration_secs().max(0.001);
                (region_secs / sample_secs).ceil().clamp(1.0, 64.0)
            } else {
                1.0
            };
            let pass_w = wave.width() / passes;
            for pass in 0..passes as usize {
                let left = wave.left() + pass as f32 * pass_w;
                let right = (left + pass_w).min(wave.right());
                let mid = wave.center().y;
                let amp = wave.height() * 0.46;
                let columns = (right - left).floor().max(1.0) as usize;
                for column in 0..columns {
                    let bucket = (column * peaks.len() / columns).min(peaks.len() - 1);
                    let value = peaks[bucket].clamp(0.0, 1.0);
                    let px = left + column as f32;
                    painter.line_segment(
                        [
                            Pos2::new(px, mid - value * amp),
                            Pos2::new(px, mid + value * amp),
                        ],
                        Stroke::new(1.0, theme::accent_alpha(theme::MAGENTA, 200)),
                    );
                }
                if passes > 1.0 {
                    painter.line_segment(
                        [
                            Pos2::new(right, wave.top()),
                            Pos2::new(right, wave.bottom()),
                        ],
                        Stroke::new(0.6, theme::accent_alpha(theme::MAGENTA, 70)),
                    );
                }
            }
        }
    }

    // Shade everything outside the clip loop.
    for shaded in [
        Rect::from_min_max(rect.min, Pos2::new(start_x, rect.bottom())),
        Rect::from_min_max(Pos2::new(end_x, rect.top()), rect.max),
    ] {
        if shaded.width() > 0.0 {
            painter.rect_filled(
                shaded,
                CornerRadius::ZERO,
                theme::accent_alpha(theme::BG_DEEP, 200),
            );
        }
    }

    // Loop handles; dragging is absolute, mapped from the pointer.
    for (edge, x) in [("start", start_x), ("end", end_x)] {
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.4, theme::AMBER),
        );
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(x - 2.0, rect.top() + 2.0), Vec2::new(4.0, 14.0)),
            CornerRadius::same(1),
            theme::AMBER,
        );
        let hit = Rect::from_min_max(
            Pos2::new(x - 5.0, rect.top()),
            Pos2::new(x + 5.0, rect.bottom()),
        );
        let response = ui.interact(
            hit,
            ui.make_persistent_id(("sample_clip_handle", track, scene, edge)),
            Sense::click_and_drag(),
        );
        let dragged = response.dragged();
        let hovered = response.hovered();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            response.on_hover_text("Drag to move the clip loop");
        }
        if dragged {
            if let Some(pointer) = ui.ctx().pointer_interact_pos() {
                let step = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0) * STEPS;
                let step = step.round() as i32;
                let (start, end) = if edge == "start" {
                    (step.clamp(0, loop_end as i32 - 1), loop_end as i32)
                } else {
                    (
                        loop_start as i32,
                        step.clamp(loop_start as i32 + 1, LOOP_STEPS as i32),
                    )
                };
                state.set_clip_loop(track, scene, start as u16, end as u16);
            }
        }
    }

    // Playhead inside the region.
    if state.playing {
        if let Some(phase) = state.clip_phase(track, scene) {
            let px = x_of(phase.clamp(loop_start, loop_end.saturating_sub(1)) as f32);
            painter.line_segment(
                [Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())],
                Stroke::new(1.4, theme::CYAN),
            );
        }
    }

    painter.text(
        Pos2::new(rect.left() + 4.0, rect.bottom() - 3.0),
        Align2::LEFT_BOTTOM,
        format!(
            "LOOP {} – {} · {} STEP{}",
            bar_beat(loop_start),
            bar_beat(loop_end.saturating_sub(1)),
            loop_end.saturating_sub(loop_start),
            if loop_end.saturating_sub(loop_start) == 1 {
                ""
            } else {
                "S"
            }
        ),
        theme::mono(6.4),
        theme::AMBER,
    );
    painter.text(
        Pos2::new(rect.right() - 4.0, rect.bottom() - 3.0),
        Align2::RIGHT_BOTTOM,
        if state.launched[track] == Some(scene as u8) {
            "PLAYING"
        } else {
            "IDLE"
        },
        theme::mono(6.4),
        if state.launched[track] == Some(scene as u8) {
            theme::GREEN
        } else {
            theme::TEXT_FAINT
        },
    );
}

/// Length of a step region in seconds at the current tempo.
fn region_seconds(state: &UiState, start: u16, end: u16) -> f32 {
    let bpm = state.bpm.max(1.0);
    let secs_per_step = 60.0 / bpm / 4.0;
    (end.saturating_sub(start)).max(1) as f32 * secs_per_step
}

fn bar_beat(step: u16) -> String {
    format!("{}.{}", step / 16 + 1, step % 16 / 4 + 1)
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut short: String = text.chars().take(max.saturating_sub(1)).collect();
        short.push('…');
        short
    }
}
