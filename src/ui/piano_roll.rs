//! Piano-roll clip editor opened by double-clicking a session clip or an
//! arrangement lane. Notes are stored in the engine's per-track MIDI clip, so
//! edits are audible immediately and saved with the project.

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::audio::{AudioRuntime, Command, LOOP_STEPS, SCENE_COUNT, TRACK_COUNT};
use crate::state::UiState;
use crate::studio::{sync_clip_loop, sync_track_notes, RollDrag, RollDragMode, Studio};
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
    let scene = (studio.piano_roll_scene as usize).min(SCENE_COUNT - 1);
    let accent = theme::track_accent(track);
    egui::Window::new(format!(
        "PIANO ROLL // TRACK {:02} {} // SLOT {:02}",
        track + 1,
        state.track_label(track),
        scene + 1
    ))
    .open(&mut open)
    .frame(
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(3))
            .inner_margin(egui::Margin::same(8)),
    )
    .default_pos(Pos2::new(80.0, 70.0))
    .default_size(Vec2::new(960.0, 640.0))
    .resizable(true)
    .show(ctx, |ui| {
        draw_header(ui, studio, state, audio, track, scene, accent);
        ui.add_space(5.0);
        draw_grid(ui, studio, state, audio, track, scene, accent);
        ui.add_space(5.0);
        draw_footer(ui, studio, state, audio, track, scene, accent);
    });
    if !open {
        studio.piano_roll_open = false;
        studio.roll_drag = None;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_header(
    ui: &mut Ui,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
    track: usize,
    scene: usize,
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
            .selected_text(format!("{:02} {}", track + 1, state.track_label(track)))
            .width(170.0)
            .show_ui(ui, |ui| {
                for index in 0..TRACK_COUNT {
                    if ui
                        .selectable_label(
                            index == track,
                            format!("{:02} {}", index + 1, state.track_label(index)),
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

        let mut chosen_scene = scene;
        egui::ComboBox::from_id_salt("piano_roll_scene")
            .selected_text(format!(
                "SLOT {:02} · {}",
                scene + 1,
                UiState::scene_name(scene)
            ))
            .width(136.0)
            .show_ui(ui, |ui| {
                for index in 0..SCENE_COUNT {
                    if ui
                        .selectable_label(
                            index == scene,
                            format!("{:02} · {}", index + 1, UiState::scene_name(index)),
                        )
                        .clicked()
                    {
                        chosen_scene = index;
                    }
                }
            });
        if chosen_scene != scene {
            studio.piano_roll_scene = chosen_scene as u8;
            studio.roll_selected = None;
            studio.roll_drag = None;
        }
        let muted = !state.recorded_active[track][scene];
        ui.label(
            egui::RichText::new(format!(
                "{count} NOTE{plural}{muted}",
                count = state.recorded_notes(track, scene).len(),
                plural = if state.recorded_notes(track, scene).len() == 1 {
                    ""
                } else {
                    "S"
                },
                muted = if muted { " · MUTED" } else { "" }
            ))
            .font(theme::mono(8.0))
            .color(if muted {
                theme::AMBER
            } else {
                theme::TEXT_FAINT
            }),
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
                .add(
                    egui::Button::new(egui::RichText::new("CLEAR SLOT").font(theme::display(8.0)))
                        .fill(theme::accent_alpha(theme::RED, 28))
                        .stroke(Stroke::new(1.0, theme::accent_alpha(theme::RED, 120))),
                )
                .clicked()
            {
                state.clear_recorded(track, scene);
                studio.roll_selected = None;
                studio.roll_drag = None;
                sync_notes(state, audio, track, scene);
            }
            let mute_label = if muted { "UNMUTE SLOT" } else { "MUTE SLOT" };
            if ui
                .add(
                    egui::Button::new(egui::RichText::new(mute_label).font(theme::display(8.0)))
                        .fill(if muted {
                            theme::accent_alpha(theme::AMBER, 30)
                        } else {
                            theme::PANEL_SUNK
                        }),
                )
                .clicked()
            {
                state.recorded_active[track][scene] = !muted;
                let _ = audio.command_tx.push(Command::SetRecordedActive {
                    track: track as u8,
                    scene: scene as u8,
                    active: !muted,
                });
            }
        });
    });

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(
                "CLICK GRID = ADD · DRAG BODY = MOVE · DRAG EDGE = LENGTH · RIGHT-CLICK = DELETE",
            )
            .font(theme::mono(6.8))
            .color(theme::TEXT_FAINT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                    egui::Button::new(egui::RichText::new("LOOP ALL").font(theme::display(8.0)))
                        .fill(theme::accent_alpha(theme::AMBER, 22))
                        .stroke(Stroke::new(1.0, theme::accent_alpha(theme::AMBER, 120))),
                )
                .on_hover_text("Reset the clip loop to the whole 16 bars")
                .clicked()
            {
                state.set_clip_loop(track, scene, 0, crate::audio::LOOP_STEPS as u16);
                sync_clip_loop(state, audio, track, scene);
            }
            if ui
                .add(egui::Button::new(
                    egui::RichText::new("RESET VIEW").font(theme::display(8.0)),
                ))
                .clicked()
            {
                studio.roll_base_note = 36;
            }
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

#[allow(clippy::too_many_arguments)]
fn draw_grid(
    ui: &mut Ui,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
    track: usize,
    scene: usize,
    accent: Color32,
) {
    let base = studio.roll_base_note.clamp(0, 127 - NOTE_SPAN as i32 + 1) as u8;
    let canvas_w = KEY_W + LOOP_STEPS as f32 * STEP_W + 2.0;
    let canvas_h = RULER_H + NOTE_SPAN as f32 * ROW_H + 2.0;
    let notes = state.recorded_notes(track, scene).to_vec();

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

            // ---- Loop region -------------------------------------------
            let (loop_start, loop_end) = state.clip_loop(track, scene);
            let loop_x0 = grid.left() + loop_start as f32 * STEP_W;
            let loop_x1 = grid.left() + loop_end as f32 * STEP_W;
            let dim = Color32::from_rgba_unmultiplied(0, 0, 0, 110);
            if loop_start > 0 {
                painter.rect_filled(
                    Rect::from_min_max(grid.min, Pos2::new(loop_x0, grid.bottom())),
                    CornerRadius::ZERO,
                    dim,
                );
            }
            if loop_end < LOOP_STEPS as u16 {
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(loop_x1, grid.top()),
                        Pos2::new(grid.right(), grid.bottom()),
                    ),
                    CornerRadius::ZERO,
                    dim,
                );
            }

            // The ruler band doubles as the loop selector.
            let ruler = Rect::from_min_max(
                Pos2::new(grid.left(), canvas.top()),
                Pos2::new(grid.right(), grid.top()),
            );
            let ruler_response = ui.interact(
                ruler,
                ui.scope_id().with(("piano_roll_loop", track, scene)),
                Sense::click_and_drag(),
            );
            if ruler_response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            let step_at = |x: f32| -> u16 {
                (((x - grid.left()) / STEP_W).floor().max(0.0) as i32).min(LOOP_STEPS as i32 - 1)
                    as u16
            };
            if ruler_response.drag_started() {
                if let Some(pos) = ruler_response.interact_pointer_pos() {
                    let anchor = step_at(pos.x);
                    studio.roll_loop_anchor = Some(anchor);
                    state.set_clip_loop(track, scene, anchor, anchor + 1);
                }
            }
            if ruler_response.dragged() {
                if let (Some(anchor), Some(pos)) = (
                    studio.roll_loop_anchor,
                    ruler_response.interact_pointer_pos(),
                ) {
                    let current = step_at(pos.x);
                    if current != anchor {
                        state.set_clip_loop(track, scene, anchor, current);
                        sync_clip_loop(state, audio, track, scene);
                    }
                }
            }
            if ruler_response.drag_stopped() {
                studio.roll_loop_anchor = None;
            }
            if ruler_response.hovered() {
                ruler_response.on_hover_text("Drag to set the clip's loop start and end");
            }

            // Loop band above the grid and its two handles.
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(loop_x0, canvas.top() + 2.0),
                    Pos2::new(loop_x1, grid.top() - 1.0),
                ),
                CornerRadius::same(1),
                theme::accent_alpha(theme::AMBER, 60),
            );
            for x in [loop_x0, loop_x1] {
                painter.line_segment(
                    [Pos2::new(x, grid.top()), Pos2::new(x, grid.bottom())],
                    Stroke::new(1.2, theme::accent_alpha(theme::AMBER, 170)),
                );
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(x - 2.0, canvas.top() + 2.0),
                        Vec2::new(4.0, RULER_H - 3.0),
                    ),
                    CornerRadius::same(1),
                    theme::AMBER,
                );
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
                                scene,
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
                            state.resize_note(
                                track,
                                scene,
                                index,
                                drag.original_length as i32 + steps,
                            );
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
                state.remove_note(track, scene, index);
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
                        if let Some(index) =
                            state.add_note(track, scene, note as u8, 100, step as u16, 4)
                        {
                            studio.roll_selected = Some(index);
                            state.recorded_active[track][scene] = true;
                            let _ = audio.command_tx.push(Command::SetRecordedActive {
                                track: track as u8,
                                scene: scene as u8,
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
                sync_notes(state, audio, track, scene);
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn draw_footer(
    ui: &mut Ui,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
    track: usize,
    scene: usize,
    accent: Color32,
) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("VELOCITY")
                .font(theme::mono(8.0))
                .color(theme::TEXT_FAINT),
        );
        let (loop_start, loop_end) = state.clip_loop(track, scene);
        ui.label(
            egui::RichText::new(format!(
                "LOOP {} – {}",
                bar_beat_text(loop_start),
                bar_beat_text(loop_end)
            ))
            .font(theme::mono(8.0))
            .color(theme::AMBER),
        );
        if let Some(selected) = studio.roll_selected {
            if let Some(note) = state.recorded_notes(track, scene).get(selected).copied() {
                let mut velocity = note.velocity as i32;
                let slider = ui.add(
                    egui::Slider::new(&mut velocity, 1..=127)
                        .show_value(true)
                        .text(""),
                );
                if slider.changed() {
                    state.set_note_velocity(track, scene, selected, velocity as u8);
                    sync_notes(state, audio, track, scene);
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
                    "DRAG THE RULER TO SET THE CLIP LOOP · TRACK {:02}",
                    track + 1
                ))
                .font(theme::mono(7.5))
                .color(theme::accent_alpha(accent, 200)),
            );
        });
    });
}

fn sync_notes(state: &UiState, audio: &mut AudioRuntime, track: usize, scene: usize) {
    sync_track_notes(state, audio, track, scene);
}

/// "bar.beat" label for a 16th-note step.
fn bar_beat_text(step: u16) -> String {
    format!("{}.{}", step / 16 + 1, step % 16 / 4 + 1)
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
