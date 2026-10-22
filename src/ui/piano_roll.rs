//! Piano-roll clip editor opened by double-clicking a session clip or an
//! arrangement lane. Notes are stored in the engine's per-track MIDI clip, so
//! edits are audible immediately and saved with the project.

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::audio::{AudioRuntime, Command, LOOP_STEPS, TRACKS, TRACK_COUNT};
use crate::state::UiState;
use crate::studio::{sync_track_notes, RollDrag, RollDragMode, Studio};
use crate::theme;

const KEY_W: f32 = 42.0;
const RULER_H: f32 = 18.0;
const STEP_W: f32 = 7.0;
const ROW_H: f32 = 12.0;
const NOTE_SPAN: usize = 49; // four octaves + the top C

pub fn draw(
    ctx: &egui::Context,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
) {
    if !studio.piano_roll_open {
        return;
    }
    let mut open = true;
    let track = studio.piano_roll_track.min(TRACK_COUNT - 1);
    let accent = theme::track_accent(track);
    egui::Window::new(format!(
        "PIANO ROLL // TRACK {:02} {}",
        track + 1,
        TRACKS[track].name
    ))
    .open(&mut open)
    .frame(
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(3))
            .inner_margin(egui::Margin::same(8)),
    )
    .default_pos(Pos2::new(140.0, 90.0))
    .default_size(Vec2::new(1080.0, 700.0))
    .resizable(true)
    .show(ctx, |ui| {
        draw_header(ui, studio, state, audio, track, accent);
        ui.add_space(5.0);
        draw_grid(ui, studio, state, audio, track, accent);
        ui.add_space(5.0);
        draw_footer(ui, studio, state, audio, track, accent);
    });
    if !open {
        studio.piano_roll_open = false;
        studio.roll_drag = None;
    }
}

fn draw_header(
    ui: &mut Ui,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
    track: usize,
    accent: Color32,
) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("TRACK")
                .font(theme::mono(8.0))
                .color(theme::TEXT_FAINT),
        );
        let mut chosen = track;
        egui::ComboBox::from_id_salt("piano_roll_track")
            .selected_text(format!("{:02} {}", track + 1, TRACKS[track].name))
            .width(190.0)
            .show_ui(ui, |ui| {
                for (index, spec) in TRACKS.iter().enumerate().take(TRACK_COUNT) {
                    if ui
                        .selectable_label(
                            index == track,
                            format!("{:02} {}", index + 1, spec.name),
                        )
                        .clicked()
                    {
                        chosen = index;
                    }
                }
            });
        if chosen != track {
            studio.piano_roll_track = chosen;
            studio.roll_selected = None;
            studio.roll_drag = None;
            state.selected_track = chosen;
        }

        ui.label(
            egui::RichText::new(format!(
                "SLOT {:02} · {}",
                studio.piano_roll_scene + 1,
                UiState::scene_name(studio.piano_roll_scene as usize)
            ))
            .font(theme::mono(8.0))
            .color(theme::TEXT_DIM),
        );
        ui.label(
            egui::RichText::new(format!(
                "{} NOTE(S) · 16 BARS · LOOP {} STEPS",
                state.recorded_notes(track).len(),
                LOOP_STEPS
            ))
            .font(theme::mono(8.0))
            .color(theme::TEXT_FAINT),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(
                    egui::RichText::new("CLOSE").font(theme::display(8.5)),
                ))
                .clicked()
            {
                studio.piano_roll_open = false;
                studio.roll_drag = None;
            }
            if ui
                .add(egui::Button::new(
                    egui::RichText::new("OCT +").font(theme::display(8.0)),
                ))
                .clicked()
            {
                studio.roll_base_note = (studio.roll_base_note + 12).min(108);
            }
            if ui
                .add(egui::Button::new(
                    egui::RichText::new("OCT -").font(theme::display(8.0)),
                ))
                .clicked()
            {
                studio.roll_base_note = (studio.roll_base_note - 12).max(0);
            }
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("CLEAR CLIP").font(theme::display(8.0)),
                    )
                    .fill(theme::accent_alpha(theme::RED, 28))
                    .stroke(Stroke::new(1.0, theme::accent_alpha(theme::RED, 120))),
                )
                .clicked()
            {
                state.clear_recorded(track);
                studio.roll_selected = None;
                studio.roll_drag = None;
                sync_notes(state, audio, track);
            }
            if ui
                .add(egui::Button::new(
                    egui::RichText::new("RESET VIEW").font(theme::display(8.0)),
                ))
                .clicked()
            {
                studio.roll_base_note = 36;
            }
            ui.label(
                egui::RichText::new("CLICK GRID = ADD · DRAG BODY = MOVE · DRAG EDGE = LENGTH · RIGHT-CLICK = DELETE")
                    .font(theme::mono(6.8))
                    .color(theme::TEXT_FAINT),
            );
        });
    });
    ui.painter().line_segment(
        [
            Pos2::new(ui.min_rect().left(), ui.min_rect().top()),
            Pos2::new(ui.min_rect().right(), ui.min_rect().top()),
        ],
        Stroke::new(1.0, theme::accent_alpha(accent, 30)),
    );
}

fn draw_grid(
    ui: &mut Ui,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
    track: usize,
    accent: Color32,
) {
    let base = studio.roll_base_note.clamp(0, 127 - NOTE_SPAN as i32 + 1) as u8;
    let canvas_w = KEY_W + LOOP_STEPS as f32 * STEP_W + 2.0;
    let canvas_h = RULER_H + NOTE_SPAN as f32 * ROW_H + 2.0;
    let notes = state.recorded_notes(track).to_vec();

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .max_height((ui.available_height() - 40.0).max(260.0))
        .show(ui, |ui| {
            let (canvas, _) = ui.allocate_exact_size(Vec2::new(canvas_w, canvas_h), Sense::hover());
            let grid = Rect::from_min_max(
                Pos2::new(canvas.left() + KEY_W, canvas.top() + RULER_H),
                Pos2::new(canvas.right(), canvas.bottom()),
            );
            let painter = ui.painter().with_clip_rect(canvas);

            painter.rect_filled(canvas, CornerRadius::same(2), theme::BG_DEEP);

            // Piano keys and horizontal rows.
            for row in 0..NOTE_SPAN {
                let note = base as i32 + (NOTE_SPAN - 1 - row) as i32;
                let y = grid.top() + row as f32 * ROW_H;
                let black = is_black(note as u8);
                let row_color = if black {
                    theme::BG_DEEP
                } else {
                    Color32::from_rgb(0x0a, 0x13, 0x1e)
                };
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(grid.left(), y), Vec2::new(grid.width(), ROW_H)),
                    CornerRadius::ZERO,
                    row_color,
                );
                let key = Rect::from_min_size(Pos2::new(canvas.left(), y), Vec2::new(KEY_W, ROW_H));
                painter.rect_filled(
                    key,
                    CornerRadius::ZERO,
                    if black {
                        Color32::from_rgb(0x05, 0x09, 0x10)
                    } else {
                        Color32::from_rgb(0x15, 0x22, 0x2f)
                    },
                );
                painter.line_segment(
                    [key.left_bottom(), key.right_bottom()],
                    Stroke::new(1.0, theme::BORDER_SOFT),
                );
                if note % 12 == 0 || row == NOTE_SPAN - 1 {
                    painter.text(
                        Pos2::new(key.right() - 4.0, key.center().y),
                        Align2::RIGHT_CENTER,
                        note_name(note as u8),
                        theme::mono(6.8),
                        if note % 12 == 0 {
                            accent
                        } else {
                            theme::TEXT_FAINT
                        },
                    );
                }
            }

            // Vertical grid: 16th, beat and bar lines.
            for step in 0..=LOOP_STEPS {
                let x = grid.left() + step as f32 * STEP_W;
                let (width, color) = if step % 16 == 0 {
                    (1.2, theme::accent_alpha(theme::BORDER, 210))
                } else if step % 4 == 0 {
                    (1.0, theme::BORDER_SOFT)
                } else {
                    (0.6, theme::accent_alpha(theme::BORDER_SOFT, 110))
                };
                painter.line_segment(
                    [Pos2::new(x, grid.top()), Pos2::new(x, grid.bottom())],
                    Stroke::new(width, color),
                );
                if step % 16 == 0 {
                    painter.text(
                        Pos2::new(x + 3.0, canvas.top() + RULER_H * 0.5),
                        Align2::LEFT_CENTER,
                        format!("{}", step / 16 + 1),
                        theme::mono(7.0),
                        theme::TEXT_FAINT,
                    );
                }
            }

            // Playhead.
            let step = (state.beats() as f32 * 4.0).rem_euclid(LOOP_STEPS as f32);
            let px = grid.left() + step * STEP_W;
            painter.line_segment(
                [Pos2::new(px, grid.top()), Pos2::new(px, grid.bottom())],
                Stroke::new(1.4, theme::accent_alpha(theme::CYAN, 220)),
            );

            // Interaction surface below the notes.
            let grid_response = ui.interact(
                grid,
                ui.scope_id().with(("piano_roll_grid", track)),
                Sense::click(),
            );
            let mut note_hovered = false;
            let mut delete_note = None;
            let mut changed = false;

            for (index, note) in notes.iter().enumerate() {
                let y = grid.top()
                    + (NOTE_SPAN as i32 - 1 - (note.note as i32 - base as i32)) as f32 * ROW_H;
                let x = grid.left() + note.start_step as f32 * STEP_W;
                let w = (note.length_steps as f32 * STEP_W).max(3.0);
                if y < grid.top() - ROW_H
                    || y > grid.bottom()
                    || x > grid.right()
                    || note.note < base
                    || note.note as i32 >= base as i32 + NOTE_SPAN as i32
                {
                    continue;
                }
                let note_rect =
                    Rect::from_min_size(Pos2::new(x, y + 1.0), Vec2::new(w, ROW_H - 2.0))
                        .intersect(grid);
                if note_rect.width() < 2.0 {
                    continue;
                }
                let selected = studio.roll_selected == Some(index);
                let velocity = note.velocity as f32 / 127.0;
                let fill = if selected {
                    theme::accent_alpha(accent, (150.0 + 105.0 * velocity) as u8)
                } else {
                    theme::accent_alpha(accent, (55.0 + 110.0 * velocity) as u8)
                };
                painter.rect_filled(note_rect, CornerRadius::same(2), fill);
                painter.rect_stroke(
                    note_rect,
                    CornerRadius::same(2),
                    Stroke::new(
                        if selected { 1.6 } else { 1.0 },
                        if selected {
                            Color32::WHITE
                        } else {
                            theme::accent_alpha(accent, 230)
                        },
                    ),
                    egui::StrokeKind::Inside,
                );

                let handle_w = if note_rect.width() > 10.0 { 5.0 } else { 2.0 };
                let body_rect = Rect::from_min_max(
                    note_rect.min,
                    Pos2::new(note_rect.right() - handle_w, note_rect.bottom()),
                );
                let handle_rect = Rect::from_min_max(
                    Pos2::new(note_rect.right() - handle_w, note_rect.top()),
                    note_rect.max,
                );

                let body = ui.interact(
                    body_rect,
                    ui.scope_id().with(("roll_note", track, index)),
                    Sense::click_and_drag(),
                );
                if body.hovered() {
                    note_hovered = true;
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                if body.clicked() {
                    studio.roll_selected = Some(index);
                }
                if body.secondary_clicked() {
                    delete_note = Some(index);
                }
                if body.drag_started() {
                    studio.roll_selected = Some(index);
                    studio.roll_drag = Some(RollDrag {
                        note_index: index,
                        original_start: note.start_step,
                        original_note: note.note,
                        original_length: note.length_steps,
                        mode: RollDragMode::Move,
                    });
                }
                if body.dragged() {
                    if let Some(drag) = studio.roll_drag {
                        if drag.note_index == index && drag.mode == RollDragMode::Move {
                            let delta = body.drag_delta();
                            let steps = (delta.x / STEP_W).round() as i32;
                            let pitch = (-delta.y / ROW_H).round() as i32;
                            state.move_note(
                                track,
                                index,
                                drag.original_start as i32 + steps,
                                drag.original_note as i32 + pitch,
                            );
                        }
                    }
                }
                if body.drag_stopped() {
                    studio.roll_drag = None;
                    changed = true;
                }

                let handle = ui.interact(
                    handle_rect,
                    ui.scope_id().with(("roll_resize", track, index)),
                    Sense::click_and_drag(),
                );
                if handle.hovered() {
                    note_hovered = true;
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                if handle.drag_started() {
                    studio.roll_selected = Some(index);
                    studio.roll_drag = Some(RollDrag {
                        note_index: index,
                        original_start: note.start_step,
                        original_note: note.note,
                        original_length: note.length_steps,
                        mode: RollDragMode::Resize,
                    });
                }
                if handle.dragged() {
                    if let Some(drag) = studio.roll_drag {
                        if drag.note_index == index && drag.mode == RollDragMode::Resize {
                            let steps = (handle.drag_delta().x / STEP_W).round() as i32;
                            state.resize_note(track, index, drag.original_length as i32 + steps);
                            changed = true;
                        }
                    }
                }
                if handle.drag_stopped() {
                    studio.roll_drag = None;
                    changed = true;
                }
            }

            if let Some(index) = delete_note {
                state.remove_note(track, index);
                studio.roll_selected = None;
                studio.roll_drag = None;
                changed = true;
            }

            // Click on empty grid adds a one-beat note.
            if !note_hovered && studio.roll_drag.is_none() && grid_response.clicked() {
                if let Some(pos) = grid_response.interact_pointer_pos() {
                    let step = ((pos.x - grid.left()) / STEP_W).floor() as i32;
                    let row = ((pos.y - grid.top()) / ROW_H).floor() as i32;
                    let note = base as i32 + (NOTE_SPAN as i32 - 1 - row);
                    if (0..LOOP_STEPS as i32).contains(&step) && (0..128).contains(&note) {
                        if let Some(index) = state.add_note(track, note as u8, 100, step as u16, 4)
                        {
                            studio.roll_selected = Some(index);
                            state.recorded_active[track] = true;
                            let _ = audio.command_tx.push(Command::SetRecordedActive {
                                track: track as u8,
                                active: true,
                            });
                            changed = true;
                        } else {
                            studio.notify("Clip is full (512 notes)");
                        }
                    }
                }
            }

            if changed {
                sync_notes(state, audio, track);
            }
        });
}

fn draw_footer(
    ui: &mut Ui,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
    track: usize,
    accent: Color32,
) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("VELOCITY")
                .font(theme::mono(8.0))
                .color(theme::TEXT_FAINT),
        );
        if let Some(selected) = studio.roll_selected {
            if let Some(note) = state.recorded_notes(track).get(selected).copied() {
                let mut velocity = note.velocity as i32;
                let slider = ui.add(
                    egui::Slider::new(&mut velocity, 1..=127)
                        .show_value(true)
                        .text(""),
                );
                if slider.changed() {
                    state.set_note_velocity(track, selected, velocity as u8);
                    sync_notes(state, audio, track);
                }
                ui.label(
                    egui::RichText::new(format!(
                        "NOTE {} · START {:03} · LEN {:02}",
                        note_name(note.note),
                        note.start_step,
                        note.length_steps
                    ))
                    .font(theme::mono(7.5))
                    .color(theme::TEXT_DIM),
                );
            } else {
                studio.roll_selected = None;
            }
        } else {
            ui.label(
                egui::RichText::new("Select a note to edit its velocity.")
                    .font(theme::mono(7.5))
                    .color(theme::TEXT_FAINT),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "CLICK GRID TO PROGRAM MIDI ON TRACK {:02}",
                    track + 1
                ))
                .font(theme::mono(7.5))
                .color(theme::accent_alpha(accent, 200)),
            );
        });
    });
}

fn sync_notes(state: &UiState, audio: &mut AudioRuntime, track: usize) {
    sync_track_notes(state, audio, track);
}

fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[(note % 12) as usize], note as i32 / 12 - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names_include_the_octave() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(69), "A4");
        assert_eq!(note_name(36), "C2");
    }

    #[test]
    fn black_keys_match_the_piano_layout() {
        assert!(is_black(61));
        assert!(is_black(66));
        assert!(!is_black(60));
        assert!(!is_black(64));
    }
}
