//! Arrangement lanes and the 3D spectral waterfall.

use egui::{Align2, Color32, CornerRadius, Mesh, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};

use crate::audio::{AudioRuntime, LOOP_STEPS, TRACKS, TRACK_COUNT};
use crate::dsp::spectrum::{BINS, HISTORY};
use crate::samples::ClipDrag;
use crate::state::UiState;
use crate::studio::Studio;
use crate::theme;
use crate::ui::widgets::{mini_roll, MiniRollView};

pub const TIMELINE_BARS: usize = 16;

pub fn draw_lanes(
    ui: &mut Ui,
    rect: Rect,
    state: &mut UiState,
    studio: &mut Studio,
    audio: &mut AudioRuntime,
) {
    let content = theme::paint_panel(
        ui.painter(),
        rect,
        "ARRANGEMENT // TRACKS",
        "TIMELINE · 16 BARS",
        theme::CYAN,
    );
    if content.height() < 40.0 {
        return;
    }

    let gutter = 88.0;
    let ruler_h = 16.0;
    let lane_area = Rect::from_min_max(
        Pos2::new(content.left() + gutter, content.top() + ruler_h),
        content.max,
    );
    let lane_h = lane_area.height() / TRACK_COUNT as f32;
    let beats_total = TIMELINE_BARS as f32 * 4.0;
    let beat_w = lane_area.width() / beats_total;

    // Ruler and beat grid.
    ui.painter().rect_filled(
        Rect::from_min_size(
            Pos2::new(content.left(), content.top()),
            Vec2::new(content.width(), ruler_h),
        ),
        CornerRadius::ZERO,
        theme::BG_DEEP,
    );
    for bar in 0..TIMELINE_BARS {
        let x = lane_area.left() + bar as f32 * 4.0 * beat_w;
        ui.painter().text(
            Pos2::new(x + 3.0, content.top() + ruler_h * 0.5),
            Align2::LEFT_CENTER,
            format!("{}", bar + 1),
            theme::mono(7.0),
            theme::TEXT_FAINT,
        );
        ui.painter().line_segment(
            [
                Pos2::new(x, content.top() + ruler_h),
                Pos2::new(x, content.bottom()),
            ],
            Stroke::new(1.0, theme::accent_alpha(theme::BORDER, 190)),
        );
        for beat in 1..4 {
            let bx = x + beat as f32 * beat_w;
            ui.painter().line_segment(
                [
                    Pos2::new(bx, lane_area.top()),
                    Pos2::new(bx, lane_area.bottom()),
                ],
                Stroke::new(1.0, theme::BORDER_SOFT),
            );
        }
    }

    let playing_beats = state.playhead_beats();
    let playhead_x =
        lane_area.left() + (playing_beats / beats_total).clamp(0.0, 1.0) * lane_area.width();

    for (track, _spec) in TRACKS.iter().enumerate() {
        let accent = theme::track_accent(track);
        let lane = Rect::from_min_size(
            Pos2::new(lane_area.left(), lane_area.top() + track as f32 * lane_h),
            Vec2::new(lane_area.width(), lane_h),
        );
        let gutter_rect = Rect::from_min_size(
            Pos2::new(content.left(), lane.top()),
            Vec2::new(gutter, lane_h),
        );
        let gutter_hit = ui.interact(
            gutter_rect,
            ui.scope_id().with(("arr_gutter", track)),
            Sense::click(),
        );
        if gutter_hit.double_clicked() {
            state.selected_track = track;
            studio.begin_rename(track, &state.track_name[track]);
        }
        if gutter_hit.hovered() {
            gutter_hit.on_hover_text("Double-click to rename this track");
        }
        let launched = state.launched[track];
        let scene = launched.unwrap_or(0) as usize;
        // The lane itself is the fallback double-click target; clip blocks
        // added later sit on top and take priority.
        let lane_hit = ui.interact(
            lane,
            ui.scope_id().with(("arr_lane", track)),
            Sense::click(),
        );

        // Dropping places the clip on a beat boundary under the pointer.
        let pointer = lane_hit
            .hover_pos()
            .or_else(|| ui.ctx().pointer_interact_pos());
        let drop_step = pointer
            .map(|pos| {
                let raw = ((pos.x - lane.left()) / lane.width().max(1.0)).clamp(0.0, 1.0)
                    * LOOP_STEPS as f32;
                ((raw / 4.0).round() * 4.0).clamp(0.0, (LOOP_STEPS - 1) as f32) as u16
            })
            .unwrap_or(0);

        if state.selected_track == track {
            ui.painter()
                .rect_filled(lane, CornerRadius::ZERO, theme::accent_alpha(accent, 8));
            ui.painter().rect_filled(
                gutter_rect,
                CornerRadius::ZERO,
                theme::accent_alpha(accent, 12),
            );
        }
        ui.painter().line_segment(
            [lane.left_bottom(), lane.right_bottom()],
            Stroke::new(1.0, theme::accent_alpha(theme::BORDER, 100)),
        );
        ui.painter().rect_filled(
            Rect::from_min_size(gutter_rect.min, Vec2::new(2.0, gutter_rect.height())),
            CornerRadius::ZERO,
            accent,
        );
        ui.painter().text(
            Pos2::new(gutter_rect.left() + 8.0, gutter_rect.center().y - 5.0),
            Align2::LEFT_CENTER,
            state.track_label(track),
            theme::display(7.0),
            if state.selected_track == track {
                accent
            } else {
                theme::TEXT_DIM
            },
        );
        ui.painter().text(
            Pos2::new(gutter_rect.right() - 6.0, gutter_rect.center().y + 5.0),
            Align2::RIGHT_CENTER,
            match launched {
                Some(_) => format!("#{:02} {}", scene + 1, UiState::scene_name(scene)),
                None => "STOPPED".to_string(),
            },
            theme::mono(5.8),
            theme::accent_alpha(accent, 130),
        );

        // The launched slot's MIDI clip takes over the lane when present.
        let notes = state.recorded_notes(track, scene).to_vec();
        let slot_enabled = state.recorded_active[track][scene];
        if !notes.is_empty() {
            let enabled = slot_enabled;
            let steps_total = crate::audio::LOOP_STEPS as f32;
            let (loop_start, loop_end) = state.clip_loop(track, scene);
            // The block is the loop region, not the whole timeline: a short
            // loop used to be drawn as a 16-bar clip.
            let x_at = |step: u16| lane.left() + (step as f32 / steps_total) * lane.width();
            let clip = Rect::from_min_max(
                Pos2::new(x_at(loop_start) + 1.5, lane.top() + 1.5),
                Pos2::new(x_at(loop_end) - 1.5, lane.bottom() - 1.5),
            );
            ui.painter().rect_filled(
                clip,
                CornerRadius::same(2),
                if enabled {
                    Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 18)
                } else {
                    Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 6)
                },
            );
            ui.painter().rect_stroke(
                clip,
                CornerRadius::same(2),
                Stroke::new(
                    1.0,
                    if enabled {
                        theme::accent_alpha(accent, 150)
                    } else {
                        theme::BORDER
                    },
                ),
                egui::StrokeKind::Inside,
            );
            ui.painter().rect_filled(
                Rect::from_min_size(clip.min, Vec2::new(2.0, clip.height())),
                CornerRadius::same(1),
                if enabled { accent } else { theme::BORDER },
            );
            let clip_hit = ui.interact(
                clip,
                ui.scope_id().with(("arr_midi", track)),
                Sense::click(),
            );
            if clip_hit.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if clip_hit.double_clicked() {
                state.selected_track = track;
                studio.open_piano_roll(track, launched.unwrap_or(0));
            }
            if clip_hit.hovered() {
                clip_hit.on_hover_text(format!(
                    "{} · MIDI clip · double-click to edit in the piano roll",
                    state.track_label(track)
                ));
            }

            // Header: the clip name on the left, its loop on the right, each
            // in its own clipped box so a narrow block cannot overlap them.
            let header = Rect::from_min_max(
                Pos2::new(clip.left() + 6.0, clip.top() + 1.0),
                Pos2::new(clip.right() - 5.0, clip.top() + 10.0),
            );
            // Give the loop readout a fixed slice and let the name shrink into
            // the rest, with shorter wording when the block is narrow.
            let loop_w = (header.width() * 0.42).clamp(30.0, 96.0);
            let name_rect = header.with_max_x(header.right() - loop_w - 3.0);
            let loop_rect = header.with_min_x(header.right() - loop_w);
            let name_text = if name_rect.width() > 96.0 {
                format!("MIDI CLIP · SLOT {:02}", scene + 1)
            } else if name_rect.width() > 40.0 {
                format!("SLOT {:02}", scene + 1)
            } else {
                format!("{:02}", scene + 1)
            };
            let name_color = if enabled {
                theme::accent_alpha(accent, 230)
            } else {
                theme::TEXT_DIM
            };
            let name_painter = ui.painter().with_clip_rect(name_rect);
            let name_font = theme::fitted_font(
                &name_painter,
                &name_text,
                theme::display(6.0),
                name_rect.width(),
            );
            name_painter.text(
                Pos2::new(name_rect.left(), name_rect.center().y),
                Align2::LEFT_CENTER,
                &name_text,
                name_font,
                name_color,
            );

            let end_inclusive = loop_end.saturating_sub(1);
            let loop_label = format!(
                "{}.{}–{}.{}",
                loop_start / 16 + 1,
                loop_start % 16 / 4 + 1,
                end_inclusive / 16 + 1,
                end_inclusive % 16 / 4 + 1
            );
            let loop_text = if !enabled {
                "MUTED".to_string()
            } else if loop_rect.width() > 62.0 {
                format!("{} N · {loop_label}", notes.len())
            } else {
                loop_label
            };
            let loop_painter = ui.painter().with_clip_rect(loop_rect);
            let loop_font = theme::fitted_font(
                &loop_painter,
                &loop_text,
                theme::mono(5.4),
                loop_rect.width(),
            );
            if loop_rect.width() > 18.0 {
                loop_painter.text(
                    Pos2::new(loop_rect.right(), loop_rect.center().y),
                    Align2::RIGHT_CENTER,
                    loop_text,
                    loop_font,
                    if enabled {
                        theme::TEXT_DIM
                    } else {
                        theme::AMBER
                    },
                );
            }

            let roll = Rect::from_min_max(
                Pos2::new(clip.left() + 4.0, clip.top() + 10.0),
                Pos2::new(clip.right() - 4.0, clip.bottom() - 2.0),
            );
            if roll.height() > 4.0 {
                mini_roll(
                    ui.painter(),
                    roll,
                    &notes,
                    (loop_start, loop_end),
                    crate::audio::LOOP_STEPS as u16,
                    MiniRollView::Timeline,
                    accent,
                    enabled,
                );
                if state.playing && enabled {
                    // The engine's clip phase already wraps inside the loop,
                    // so map that step straight onto the clip block.
                    let phase = state
                        .clip_phase(track, scene)
                        .unwrap_or(loop_start)
                        .clamp(loop_start, loop_end.saturating_sub(1));
                    let px = roll.left() + phase as f32 / steps_total * roll.width();
                    ui.painter().line_segment(
                        [Pos2::new(px, roll.top()), Pos2::new(px, roll.bottom())],
                        Stroke::new(1.0, theme::CYAN),
                    );
                }
            }
        }

        // Audio samples live in their slot's loop region, like MIDI clips.
        let sample = state.sample_clip(track, scene).cloned();
        if let Some(sample) = &sample {
            let steps_total = crate::audio::LOOP_STEPS as f32;
            let (loop_start, loop_end) = state.clip_loop(track, scene);
            let x0 = lane.left() + loop_start as f32 / steps_total * lane.width();
            let x1 = lane.left() + loop_end as f32 / steps_total * lane.width();
            let block = Rect::from_min_max(
                Pos2::new(x0 + 1.5, lane.top() + 1.5),
                Pos2::new(x1 - 1.5, lane.bottom() - 1.5),
            );
            ui.painter().rect_filled(
                block,
                CornerRadius::same(2),
                theme::accent_alpha(theme::MAGENTA, 16),
            );
            ui.painter().rect_stroke(
                block,
                CornerRadius::same(2),
                Stroke::new(1.0, theme::accent_alpha(theme::MAGENTA, 150)),
                egui::StrokeKind::Inside,
            );
            let wave = Rect::from_min_max(
                Pos2::new(block.left() + 4.0, block.top() + 9.0),
                Pos2::new(block.right() - 4.0, block.bottom() - 2.0),
            );
            if wave.width() > 6.0 && wave.height() > 4.0 {
                draw_peaks(ui.painter(), wave, &sample.peaks, theme::MAGENTA);
            }
            let sample_hit = ui.interact(
                block,
                ui.scope_id().with(("arr_sample", track)),
                Sense::click(),
            );
            let edit_sample = sample_hit.double_clicked();
            if sample_hit.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                sample_hit.on_hover_text(format!(
                    "{} · sample {} · {:.2} s · double-click to edit",
                    state.track_label(track),
                    sample.name,
                    sample.duration_secs()
                ));
            }
            if edit_sample {
                state.selected_track = track;
                studio.open_sample_editor_clip(sample);
            }
            ui.painter().text(
                Pos2::new(block.left() + 6.0, block.top() + 1.0),
                Align2::LEFT_TOP,
                format!("SAMPLE · {}", sample.extension()),
                theme::display(5.8),
                theme::MAGENTA,
            );
        }

        // Arrangement audio clips, placed at their timeline position.
        let arrangement: Vec<_> = state
            .arrangement
            .iter()
            .filter(|clip| clip.track == track)
            .cloned()
            .collect();
        let mut remove_arrangement = None;
        let mut edit_arrangement = None;
        for clip in &arrangement {
            let steps_total = LOOP_STEPS as f32;
            let x0 = lane.left() + clip.start_step as f32 / steps_total * lane.width();
            let x1 = lane.left()
                + (clip.start_step + clip.length_steps).min(LOOP_STEPS as u16) as f32 / steps_total
                    * lane.width();
            let block = Rect::from_min_max(
                Pos2::new(x0 + 1.0, lane.top() + 1.5),
                Pos2::new(x1.max(x0 + 12.0) - 1.0, lane.bottom() - 1.5),
            );
            let block_hit = ui.interact(
                block,
                ui.scope_id().with(("arr_clip", clip.id)),
                Sense::click_and_drag(),
            );
            if block_hit.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            block_hit.dnd_set_drag_payload(ClipDrag::Arrangement {
                id: clip.id,
                clip: std::sync::Arc::clone(&clip.clip),
            });

            let muted = state.mute[track] || (state.any_solo() && !state.solo[track]);
            let fill = if muted {
                theme::accent_alpha(theme::BORDER, 40)
            } else {
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 24)
            };
            ui.painter().rect_filled(block, CornerRadius::same(2), fill);
            ui.painter().rect_stroke(
                block,
                CornerRadius::same(2),
                Stroke::new(
                    if block_hit.hovered() { 1.6 } else { 1.0 },
                    if muted {
                        theme::BORDER
                    } else {
                        theme::accent_alpha(accent, 180)
                    },
                ),
                egui::StrokeKind::Inside,
            );
            ui.painter().rect_filled(
                Rect::from_min_size(block.min, Vec2::new(2.0, block.height())),
                CornerRadius::same(1),
                if muted { theme::BORDER } else { accent },
            );

            let header = Rect::from_min_max(
                Pos2::new(block.left() + 5.0, block.top() + 1.0),
                Pos2::new(block.right() - 4.0, block.top() + 11.0),
            );
            let label = format!("AUDIO · {}", clip.clip.name);
            let label_painter = ui.painter().with_clip_rect(header);
            let label_font =
                theme::fitted_font(&label_painter, &label, theme::display(6.0), header.width());
            label_painter.text(
                Pos2::new(header.left(), header.center().y),
                Align2::LEFT_CENTER,
                &label,
                label_font,
                if muted {
                    theme::TEXT_DIM
                } else {
                    theme::accent_alpha(accent, 230)
                },
            );

            let wave = Rect::from_min_max(
                Pos2::new(block.left() + 4.0, block.top() + 11.0),
                Pos2::new(block.right() - 4.0, block.bottom() - 2.0),
            );
            if wave.width() > 4.0 && wave.height() > 3.0 {
                draw_peaks(
                    ui.painter(),
                    wave,
                    &clip.clip.peaks,
                    if muted {
                        theme::BORDER
                    } else {
                        theme::accent_alpha(accent, 170)
                    },
                );
            }

            if block_hit.double_clicked() {
                edit_arrangement = Some(clip.clip.clone());
            }
            block_hit.context_menu(|ui| {
                if ui.button("OPEN AUDIO EDITOR").clicked() {
                    edit_arrangement = Some(clip.clip.clone());
                    ui.close();
                }
                if ui.button("REMOVE FROM ARRANGEMENT").clicked() {
                    remove_arrangement = Some(clip.id);
                    ui.close();
                }
            });
            block_hit
                .on_hover_text("Arrangement audio · drag to a session cell · double-click to edit");
        }
        if let Some(clip) = edit_arrangement {
            studio.open_sample_editor_clip(&clip);
        }
        if let Some(id) = remove_arrangement {
            studio.remove_arrangement_clip(state, audio, id);
        }

        // Ghost of the clip being dragged, and the highlight for OS file drops.
        let drag_payload = lane_hit.dnd_hover_payload::<ClipDrag>();
        let files_hovered = ui.ctx().input(|input| !input.raw.hovered_files.is_empty());
        if drag_payload.is_some() || (files_hovered && lane_hit.contains_pointer()) {
            let length = drag_payload
                .as_ref()
                .map(|drag| state.steps_for_duration(drag.clip().duration_secs(), drop_step))
                .unwrap_or(16);
            let x0 = lane.left() + drop_step as f32 / LOOP_STEPS as f32 * lane.width();
            let x1 = lane.left()
                + (drop_step.saturating_add(length)).min(LOOP_STEPS as u16) as f32
                    / LOOP_STEPS as f32
                    * lane.width();
            let ghost = Rect::from_min_max(
                Pos2::new(x0 + 1.0, lane.top() + 2.0),
                Pos2::new(x1.max(x0 + 18.0) - 1.0, lane.bottom() - 2.0),
            );
            ui.painter().rect_filled(
                ghost,
                CornerRadius::same(2),
                theme::accent_alpha(theme::MAGENTA, 20),
            );
            ui.painter().rect_stroke(
                ghost,
                CornerRadius::same(2),
                Stroke::new(1.2, theme::accent_alpha(theme::MAGENTA, 180)),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                ghost.center(),
                Align2::CENTER_CENTER,
                "DROP AUDIO",
                theme::mono(6.5),
                theme::MAGENTA,
            );
        }

        // Audio clip blocks, one per four bars (generative slots only).
        let block_bars = 4usize;
        let blocks = if launched.is_some() && notes.is_empty() && sample.is_none() {
            TIMELINE_BARS / block_bars
        } else {
            0
        };
        for block in 0..blocks {
            let x0 = lane.left() + (block * block_bars) as f32 * 4.0 * beat_w;
            let x1 = x0 + block_bars as f32 * 4.0 * beat_w;
            let block_rect = Rect::from_min_max(
                Pos2::new(x0 + 2.0, lane.top() + 3.0),
                Pos2::new(x1 - 2.0, lane.bottom() - 3.0),
            );
            let block_hit = ui.interact(
                block_rect,
                ui.scope_id().with(("arr_audio", track, block)),
                Sense::click(),
            );
            if block_hit.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if block_hit.double_clicked() {
                state.selected_track = track;
                studio.open_audio_editor(track);
            }
            if block_hit.hovered() {
                block_hit.on_hover_text(format!(
                    "{} · audio clip · double-click to edit the clip envelope",
                    state.track_label(track)
                ));
            }
            let muted = state.mute[track] || (state.any_solo() && !state.solo[track]);
            let alpha = if muted { 45 } else { 150 };
            ui.painter().rect_filled(
                block_rect,
                CornerRadius::same(2),
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 12),
            );
            ui.painter().rect_stroke(
                block_rect,
                CornerRadius::same(2),
                Stroke::new(1.0, theme::accent_alpha(accent, 55)),
                egui::StrokeKind::Inside,
            );

            if block_rect.width() > 12.0 && block_rect.height() > 8.0 {
                let points = state.clip_waveform(track, scene, 72);
                let mid = block_rect.center().y;
                let amp = block_rect.height() * 0.40;
                let path: Vec<Pos2> = points
                    .iter()
                    .enumerate()
                    .map(|(i, value)| {
                        let x = block_rect.left()
                            + block_rect.width() * i as f32 / (points.len() - 1) as f32;
                        Pos2::new(x, mid - value * amp)
                    })
                    .collect();
                ui.painter().add(Shape::line(
                    path,
                    Stroke::new(1.0, theme::accent_alpha(accent, alpha)),
                ));
            }
        }

        // Mute / solo indicators on the gutter.
        if state.mute[track] {
            ui.painter().text(
                Pos2::new(gutter_rect.right() - 6.0, gutter_rect.top() + 4.0),
                Align2::RIGHT_TOP,
                "M",
                theme::mono(7.0),
                theme::AMBER,
            );
        } else if state.solo[track] {
            ui.painter().text(
                Pos2::new(gutter_rect.right() - 6.0, gutter_rect.top() + 4.0),
                Align2::RIGHT_TOP,
                "S",
                theme::mono(7.0),
                theme::GREEN,
            );
        }

        if lane_hit.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if lane_hit.double_clicked() {
            state.selected_track = track;
            if sample.is_some() {
                studio.open_sample_clip(track, scene);
            } else if notes.is_empty() {
                studio.open_audio_editor(track);
            } else {
                studio.open_piano_roll(track, launched.unwrap_or(0));
            }
        }
        if lane_hit.hovered() {
            lane_hit.clone().on_hover_text(format!(
                "{} · drop audio here · double-click to edit the {} clip",
                state.track_label(track),
                if notes.is_empty() { "audio" } else { "MIDI" }
            ));
        }

        // A clip dragged from the session matrix (or another lane) lands here.
        if let Some(payload) = lane_hit.dnd_release_payload::<ClipDrag>() {
            studio.drop_clip_on_arrangement(state, audio, (*payload).clone(), track, drop_step);
        }

        // Audio files dropped from the desktop become arrangement clips.
        let dropped: Vec<std::path::PathBuf> = ui.ctx().input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() && lane_hit.contains_pointer() {
            ui.ctx().input_mut(|input| input.raw.dropped_files.clear());
            for (index, path) in dropped.iter().enumerate() {
                let start =
                    (drop_step as u32 + index as u32 * 16).min((LOOP_STEPS - 1) as u32) as u16;
                studio.import_arrangement_file(state, audio, path, track, start);
            }
        }
    }

    // Playhead.
    ui.painter().line_segment(
        [
            Pos2::new(playhead_x, content.top() + ruler_h),
            Pos2::new(playhead_x, content.bottom()),
        ],
        Stroke::new(4.0, theme::accent_alpha(theme::CYAN, 25)),
    );
    ui.painter().line_segment(
        [
            Pos2::new(playhead_x, content.top() + ruler_h),
            Pos2::new(playhead_x, content.bottom()),
        ],
        Stroke::new(1.2, theme::CYAN),
    );
    let tri = vec![
        Pos2::new(playhead_x - 4.0, content.top()),
        Pos2::new(playhead_x + 4.0, content.top()),
        Pos2::new(playhead_x, content.top() + 6.0),
    ];
    ui.painter()
        .add(Shape::convex_polygon(tri, theme::CYAN, Stroke::NONE));
}

pub fn draw_fft(ui: &mut Ui, rect: Rect, state: &mut UiState) {
    let tag = format!(
        "1024 PTS · HANN · {:.1} kHz",
        state.sample_rate as f32 / 1000.0
    );
    let content = theme::paint_panel(
        ui.painter(),
        rect,
        "SPECTRAL WATERFALL // FFT",
        &tag,
        theme::MAGENTA,
    );
    if content.width() < 40.0 || content.height() < 40.0 {
        return;
    }

    // Sunken screen.
    ui.painter()
        .rect_filled(content, CornerRadius::same(2), theme::BG_DEEP);
    ui.painter().rect_stroke(
        content,
        CornerRadius::same(2),
        Stroke::new(1.0, theme::BORDER_SOFT),
        egui::StrokeKind::Inside,
    );

    let spread = content.width() * 0.55;
    let depth_x = content.width() * 0.30;
    let depth_y = content.height() * 0.28;
    let amp_h = content.height() * 0.68;
    let origin_x = content.left() + content.width() * 0.44;
    let base_y = content.top() + content.height() * 0.88;

    let project = |fx: f32, depth: f32, amp: f32| -> Pos2 {
        let persp = 1.0 - 0.22 * depth;
        Pos2::new(
            origin_x + (fx - 0.5) * spread * persp + depth * depth_x,
            base_y - depth * depth_y - amp * amp_h * persp,
        )
    };

    // Ground grid.
    let ground = Color32::from_rgba_unmultiplied(0, 229, 255, 18);
    for i in 0..=8 {
        let fx = i as f32 / 8.0;
        ui.painter().line_segment(
            [project(fx, 0.0, 0.0), project(fx, 1.0, 0.0)],
            Stroke::new(1.0, ground),
        );
    }
    for i in 0..=6 {
        let depth = i as f32 / 6.0;
        ui.painter().line_segment(
            [project(0.0, depth, 0.0), project(1.0, depth, 0.0)],
            Stroke::new(1.0, ground),
        );
    }

    // Frequency labels along the front edge.
    let labels = [
        (0.0, "30"),
        (0.28, "200"),
        (0.55, "1k"),
        (0.78, "4k"),
        (1.0, "16k"),
    ];
    for (fx, label) in labels {
        let pos = project(fx, 0.0, 0.0);
        ui.painter().text(
            Pos2::new(pos.x, pos.y + 4.0),
            Align2::CENTER_TOP,
            label,
            theme::mono(6.5),
            theme::TEXT_FAINT,
        );
    }
    ui.painter().text(
        Pos2::new(content.left() + 6.0, content.bottom() - 5.0),
        Align2::LEFT_BOTTOM,
        "FREQ Hz",
        theme::mono(6.5),
        theme::TEXT_FAINT,
    );
    ui.painter().text(
        Pos2::new(project(1.0, 1.0, 0.0).x, content.top() + 7.0),
        Align2::RIGHT_TOP,
        "TIME →",
        theme::mono(6.5),
        theme::TEXT_FAINT,
    );

    let history = state.analyzer.history();
    let frames = history.len().min(HISTORY);
    if frames < 2 {
        return;
    }

    // Render a coarser grid for performance and legibility.
    let freq_bins = 44usize;
    let time_cols = frames.min(44);
    let start_frame = frames - time_cols;

    let mut vertices = Vec::with_capacity((freq_bins + 1) * (time_cols + 1));
    for col in 0..=time_cols {
        let history_index = start_frame + col.min(time_cols - 1);
        let data = &history[history_index];
        let depth = 1.0 - col as f32 / time_cols as f32;
        for bin in 0..=freq_bins {
            let frac = bin as f32 / freq_bins as f32;
            let data_index = ((frac * (BINS - 1) as f32) as usize).min(BINS - 1);
            let amp = data[data_index].powf(0.92).clamp(0.0, 1.0);
            let pos = project(frac, depth, amp);
            vertices.push((pos, depth, amp));
        }
    }

    let mut mesh = Mesh::default();
    mesh.vertices.reserve(vertices.len());
    mesh.indices.reserve(time_cols * freq_bins * 6);
    for (pos, depth, amp) in &vertices {
        let shade = 0.55 + 0.45 * (1.0 - depth);
        mesh.colored_vertex(*pos, heat(*amp, shade));
    }
    let stride = freq_bins + 1;
    for col in 0..time_cols {
        for bin in 0..freq_bins {
            let i0 = (col * stride + bin) as u32;
            let i1 = ((col + 1) * stride + bin) as u32;
            let i2 = (col * stride + bin + 1) as u32;
            let i3 = ((col + 1) * stride + bin + 1) as u32;
            mesh.add_triangle(i0, i1, i2);
            mesh.add_triangle(i1, i3, i2);
        }
    }
    ui.painter().add(Shape::mesh(mesh));

    // Sparse wireframe over the surface.
    let wire = Color32::from_rgba_unmultiplied(220, 250, 255, 30);
    for col in (0..=time_cols).step_by(5) {
        let offset = col * stride;
        let path: Vec<Pos2> = (0..=freq_bins)
            .step_by(2)
            .map(|bin| vertices[offset + bin].0)
            .collect();
        ui.painter().add(Shape::line(path, Stroke::new(1.0, wire)));
    }
    for bin in (0..=freq_bins).step_by(5) {
        let path: Vec<Pos2> = (0..=time_cols)
            .map(|col| vertices[col * stride + bin].0)
            .collect();
        ui.painter().add(Shape::line(path, Stroke::new(1.0, wire)));
    }

    // Bright ridge for the newest spectrum.
    let offset = time_cols * stride;
    let ridge: Vec<Pos2> = (0..=freq_bins)
        .map(|bin| vertices[offset + bin].0)
        .collect();
    ui.painter().add(Shape::line(
        ridge.clone(),
        Stroke::new(3.0, Color32::from_rgba_unmultiplied(0, 229, 255, 35)),
    ));
    ui.painter()
        .add(Shape::line(ridge, Stroke::new(1.0, theme::CYAN)));

    // Corner readouts.
    let peak_bin = state
        .analyzer
        .display()
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let peak_hz = state.analyzer.peak_hz(peak_bin);
    ui.painter().text(
        Pos2::new(content.right() - 6.0, content.bottom() - 5.0),
        Align2::RIGHT_BOTTOM,
        format!("PEAK {:.0} Hz", peak_hz),
        theme::mono(6.5),
        theme::MAGENTA,
    );

    // Faint scanline overlay for the CRT feel.
    let mut y = content.top() + 3.0;
    let scan = Color32::from_rgba_unmultiplied(0, 0, 0, 35);
    while y < content.bottom() {
        ui.painter().line_segment(
            [Pos2::new(content.left(), y), Pos2::new(content.right(), y)],
            Stroke::new(1.0, scan),
        );
        y += 3.0;
    }
}

fn heat(value: f32, shade: f32) -> Color32 {
    let v = value.clamp(0.0, 1.0);
    let stops: [(f32, [f32; 3]); 6] = [
        (0.0, [4.0, 10.0, 24.0]),
        (0.18, [0.0, 70.0, 130.0]),
        (0.38, [0.0, 190.0, 235.0]),
        (0.58, [0.0, 229.0, 255.0]),
        (0.76, [224.0, 36.0, 195.0]),
        (1.0, [255.0, 230.0, 255.0]),
    ];
    let mut color = stops.last().unwrap().1;
    for window in stops.windows(2) {
        let (a_v, a_c) = window[0];
        let (b_v, b_c) = window[1];
        if v >= a_v && v <= b_v {
            let t = ((v - a_v) / (b_v - a_v)).clamp(0.0, 1.0);
            color = [
                a_c[0] + (b_c[0] - a_c[0]) * t,
                a_c[1] + (b_c[1] - a_c[1]) * t,
                a_c[2] + (b_c[2] - a_c[2]) * t,
            ];
            break;
        }
    }
    Color32::from_rgb(
        (color[0] * shade) as u8,
        (color[1] * shade) as u8,
        (color[2] * shade) as u8,
    )
}

/// Draw a cached sample envelope with one vertical span per pixel.
fn draw_peaks(painter: &egui::Painter, rect: Rect, peaks: &[f32], color: Color32) {
    if peaks.is_empty() || rect.width() < 2.0 {
        return;
    }
    let mid = rect.center().y;
    let amp = rect.height() * 0.46;
    let width = rect.width().floor().max(1.0) as usize;
    for x in 0..width {
        let bucket = (x * peaks.len() / width).min(peaks.len() - 1);
        let value = peaks[bucket].clamp(0.0, 1.0);
        let px = rect.left() + x as f32;
        painter.line_segment(
            [
                Pos2::new(px, mid - value * amp),
                Pos2::new(px, mid + value * amp),
            ],
            Stroke::new(1.0, theme::accent_alpha(color, 200)),
        );
    }
}
