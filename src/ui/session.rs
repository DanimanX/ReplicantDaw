//! Session view (clip matrix) and mixer strips.

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::audio::{
    db_to_gain, gain_to_db, AudioRuntime, Command, SCENE_COUNT, TRACKS, TRACK_COUNT,
};
use crate::state::UiState;
use crate::theme;
use crate::ui::widgets::{fader, knob, meter, toggle_chip};

pub fn draw_matrix(ui: &mut Ui, rect: Rect, state: &mut UiState, audio: &mut AudioRuntime) {
    let content = theme::paint_panel(
        ui.painter(),
        rect,
        "CLIP MATRIX // SESSION",
        "8 TRACKS · 8 SCENES",
        theme::CYAN,
    );

    let header_h = 34.0;
    let launch_w = 22.0;
    let cols = TRACK_COUNT as f32;
    let rows = SCENE_COUNT as f32;
    let col_w = (content.width() - launch_w) / cols;
    let row_h = (content.height() - header_h) / rows;

    // Column headers.
    for (track, spec) in TRACKS.iter().enumerate() {
        let x = content.left() + launch_w + track as f32 * col_w;
        let head = Rect::from_min_size(Pos2::new(x, content.top()), Vec2::new(col_w, header_h));
        let accent = theme::track_accent(track);
        let selected = state.selected_track == track;
        if selected {
            ui.painter().rect_filled(
                Rect::from_min_size(
                    Pos2::new(head.left() + 1.5, head.bottom() - 2.0),
                    Vec2::new(head.width() - 3.0, 2.0),
                ),
                CornerRadius::same(1),
                accent,
            );
        }
        let hit = ui.interact(
            head,
            ui.scope_id().with(("track_head", track)),
            Sense::click(),
        );
        if hit.clicked() {
            state.selected_track = track;
        }
        if hit.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let label = spec.name;
        let head_painter = ui
            .painter()
            .with_clip_rect(head.shrink2(Vec2::new(1.0, 0.0)));
        head_painter.text(
            Pos2::new(head.center().x, head.center().y + 1.0),
            Align2::CENTER_CENTER,
            label,
            theme::display(6.9),
            if selected { accent } else { theme::TEXT_DIM },
        );
        ui.painter().text(
            Pos2::new(head.left() + 4.0, head.top() + 3.0),
            Align2::LEFT_TOP,
            format!("{:02}", track + 1),
            theme::mono(6.5),
            theme::accent_alpha(accent, 150),
        );
    }

    // Scene launch column + rows.
    for scene in 0..SCENE_COUNT {
        let y = content.top() + header_h + scene as f32 * row_h;
        let launch_rect = Rect::from_min_size(
            Pos2::new(content.left(), y + 2.0),
            Vec2::new(launch_w - 3.0, row_h - 4.0),
        );
        let launch = ui.interact(
            launch_rect,
            ui.scope_id().with(("scene_launch", scene)),
            Sense::click(),
        );
        let hovered = launch.hovered();
        if hovered {
            state.hovered_scene = Some(scene);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        ui.painter().rect_filled(
            launch_rect,
            CornerRadius::same(1),
            if hovered {
                theme::PANEL_RAISED
            } else {
                theme::BG
            },
        );
        let center = launch_rect.center();
        let tri = vec![
            Pos2::new(center.x - 4.0, center.y - 5.0),
            Pos2::new(center.x + 4.0, center.y),
            Pos2::new(center.x - 4.0, center.y + 5.0),
        ];
        ui.painter().add(egui::Shape::convex_polygon(
            tri,
            if hovered {
                theme::MAGENTA
            } else {
                theme::TEXT_FAINT
            },
            Stroke::NONE,
        ));
        if launch.clicked() {
            for track in 0..TRACK_COUNT {
                let _ = audio.command_tx.push(Command::LaunchClip {
                    track: track as u8,
                    scene: scene as u8,
                });
                state.launched[track] = Some(scene as u8);
            }
        }
        if hovered {
            launch.on_hover_text(format!(
                "Launch scene {} · {}",
                scene + 1,
                UiState::scene_name(scene)
            ));
        }
    }

    let beats = state.playhead_beats();
    let step_in_bar = (beats.fract() * 4.0 * 4.0) as usize % 16;
    let step_fraction = (beats * 4.0).fract();

    for (track, spec) in TRACKS.iter().enumerate() {
        let accent = theme::track_accent(track);
        let x = content.left() + launch_w + track as f32 * col_w;
        for scene in 0..SCENE_COUNT {
            let y = content.top() + header_h + scene as f32 * row_h;
            let cell =
                Rect::from_min_size(Pos2::new(x, y + 1.5), Vec2::new(col_w - 2.0, row_h - 3.0));
            if cell.width() < 8.0 || cell.height() < 8.0 {
                continue;
            }
            let response = ui.interact(
                cell,
                ui.scope_id().with(("clip", track, scene)),
                Sense::click(),
            );
            let launched = state.launched[track] == Some(scene as u8);
            let active = launched && state.playing;
            let hovered = response.hovered();
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }

            let bg = if launched {
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 26)
            } else if hovered {
                theme::PANEL_RAISED
            } else {
                theme::PANEL_SUNK
            };
            ui.painter().rect_filled(cell, CornerRadius::same(2), bg);
            ui.painter().rect_stroke(
                cell,
                CornerRadius::same(2),
                Stroke::new(
                    if launched { 1.2 } else { 1.0 },
                    if launched {
                        accent
                    } else if hovered {
                        theme::accent_alpha(accent, 110)
                    } else {
                        theme::BORDER_SOFT
                    },
                ),
                egui::StrokeKind::Inside,
            );

            if launched {
                ui.painter().rect_filled(
                    Rect::from_min_size(cell.min, Vec2::new(2.0, cell.height())),
                    CornerRadius::same(1),
                    accent,
                );
            }

            // Miniature waveform.
            let wave_rect = Rect::from_min_max(
                Pos2::new(cell.left() + 5.0, cell.top() + 5.0),
                Pos2::new(cell.right() - 5.0, cell.bottom() - 8.0),
            );
            if wave_rect.width() > 8.0 && wave_rect.height() > 6.0 {
                let points = state.clip_waveform(track, scene, 28);
                let mid_y = wave_rect.center().y;
                let amp = wave_rect.height() * 0.42;
                let path: Vec<Pos2> = points
                    .iter()
                    .enumerate()
                    .map(|(i, value)| {
                        let x = wave_rect.left()
                            + wave_rect.width() * i as f32 / (points.len().max(2) - 1) as f32;
                        Pos2::new(x, mid_y - value * amp)
                    })
                    .collect();
                let color = if launched {
                    theme::accent_alpha(accent, 220)
                } else {
                    theme::accent_alpha(accent, 75)
                };
                ui.painter()
                    .add(egui::Shape::line(path, Stroke::new(1.0, color)));
                ui.painter().line_segment(
                    [
                        Pos2::new(wave_rect.left(), mid_y),
                        Pos2::new(wave_rect.right(), mid_y),
                    ],
                    Stroke::new(1.0, theme::accent_alpha(accent, 30)),
                );
            }

            // Name on the bottom edge for wider slots.
            if cell.width() > 54.0 {
                ui.painter().text(
                    Pos2::new(cell.left() + 5.0, cell.bottom() - 3.0),
                    Align2::LEFT_BOTTOM,
                    UiState::scene_name(scene),
                    theme::mono(6.0),
                    if launched {
                        theme::accent_alpha(accent, 200)
                    } else {
                        theme::TEXT_FAINT
                    },
                );
            }

            // Live step indicator for the playing clip.
            if active {
                let bar_w = cell.width() - 8.0;
                let progress = (step_in_bar as f32 + step_fraction) / 16.0;
                let y = cell.bottom() - 4.0;
                ui.painter().line_segment(
                    [
                        Pos2::new(cell.left() + 4.0, y),
                        Pos2::new(cell.left() + 4.0 + bar_w, y),
                    ],
                    Stroke::new(2.0, theme::accent_alpha(accent, 50)),
                );
                ui.painter().line_segment(
                    [
                        Pos2::new(cell.left() + 4.0, y),
                        Pos2::new(cell.left() + 4.0 + bar_w * progress, y),
                    ],
                    Stroke::new(2.0, accent),
                );
                let pulse = 0.55 + 0.45 * (beats * 8.0).sin().abs();
                ui.painter().circle_filled(
                    Pos2::new(cell.right() - 6.0, cell.top() + 6.0),
                    1.8,
                    theme::accent_alpha(accent, (pulse * 255.0) as u8),
                );
            }

            if response.clicked() {
                if launched {
                    let _ = audio
                        .command_tx
                        .push(Command::StopClip { track: track as u8 });
                    state.launched[track] = None;
                } else {
                    let _ = audio.command_tx.push(Command::LaunchClip {
                        track: track as u8,
                        scene: scene as u8,
                    });
                    state.launched[track] = Some(scene as u8);
                }
            }
            if response.secondary_clicked() {
                let _ = audio
                    .command_tx
                    .push(Command::StopClip { track: track as u8 });
                state.launched[track] = None;
            }
            if response.hovered() {
                response.on_hover_text(format!(
                    "{} · {} · scene {}",
                    spec.name,
                    UiState::scene_name(scene),
                    scene + 1
                ));
            }
        }
    }

    // Row hairlines.
    for scene in 0..=SCENE_COUNT {
        let y = content.top() + header_h + scene as f32 * row_h;
        ui.painter().line_segment(
            [Pos2::new(content.left(), y), Pos2::new(content.right(), y)],
            Stroke::new(1.0, theme::accent_alpha(theme::BORDER, 90)),
        );
    }
}

pub fn draw_mixer(ui: &mut Ui, rect: Rect, state: &mut UiState, audio: &mut AudioRuntime) {
    let content = theme::paint_panel(
        ui.painter(),
        rect,
        "MIXER // CHANNEL STRIPS",
        "dBFS",
        theme::MAGENTA,
    );

    let col_w = content.width() / TRACK_COUNT as f32;
    for (track, spec) in TRACKS.iter().enumerate() {
        let accent = theme::track_accent(track);
        let strip = Rect::from_min_size(
            Pos2::new(content.left() + track as f32 * col_w, content.top()),
            Vec2::new(col_w, content.height()),
        );
        let inner = strip.shrink2(Vec2::new(2.0, 0.0));
        let selected = state.selected_track == track;

        if selected {
            ui.painter().rect_filled(
                Rect::from_min_size(inner.min, Vec2::new(inner.width(), 1.0)),
                CornerRadius::ZERO,
                accent,
            );
        }

        // Name / select.
        let name_rect = Rect::from_min_size(inner.min, Vec2::new(inner.width(), 14.0));
        let name_hit = ui.interact(
            name_rect,
            ui.scope_id().with(("strip_name", track)),
            Sense::click(),
        );
        if name_hit.clicked() {
            state.selected_track = track;
        }
        if name_hit.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        ui.painter().text(
            Pos2::new(name_rect.center().x, name_rect.center().y),
            Align2::CENTER_CENTER,
            spec.name,
            theme::display(6.8),
            if selected { accent } else { theme::TEXT_DIM },
        );

        // Pan knob.
        let pan_rect = Rect::from_min_size(
            Pos2::new(
                inner.left() + inner.width() * 0.5 - 15.0,
                name_rect.bottom() + 1.0,
            ),
            Vec2::new(30.0, 46.0),
        );
        let pan_label = pan_text(state.track_pan[track]);
        let changed = knob(
            ui,
            pan_rect,
            ui.scope_id().with(("pan", track)),
            &mut state.track_pan[track],
            -1.0..=1.0,
            0.0,
            "PAN",
            &pan_label,
            accent,
        );
        if changed {
            let _ = audio.command_tx.push(Command::SetTrackPan {
                track: track as u8,
                pan: state.track_pan[track],
            });
        }

        let bottom_h = 15.0;
        let fader_top = pan_rect.bottom() + 2.0;
        let fader_bottom = inner.bottom() - bottom_h - 11.0;
        let fader_h = (fader_bottom - fader_top).max(20.0);

        // dB scale ticks (left side).
        for (db, label) in [
            (6.0, "+6"),
            (0.0, "0"),
            (-12.0, "-12"),
            (-24.0, "-24"),
            (-36.0, "-36"),
            (-48.0, "-48"),
        ] {
            let t = (db + 60.0) / 66.0;
            let y = fader_bottom - t * fader_h;
            ui.painter().text(
                Pos2::new(inner.left() + 1.0, y),
                Align2::LEFT_CENTER,
                label,
                theme::mono(5.6),
                theme::TEXT_FAINT,
            );
        }

        let fader_rect = Rect::from_min_size(
            Pos2::new(inner.left() + inner.width() * 0.44, fader_top),
            Vec2::new(18.0, fader_h),
        );
        let meter_rect = Rect::from_min_size(
            Pos2::new(fader_rect.right() + 2.0, fader_top + 7.0),
            Vec2::new(6.5, fader_h - 14.0),
        );

        let changed = fader(
            ui,
            fader_rect,
            ui.scope_id().with(("fader", track)),
            &mut state.track_gain_db[track],
            accent,
        );
        if changed {
            let _ = audio.command_tx.push(Command::SetTrackGain {
                track: track as u8,
                gain: db_to_gain(state.track_gain_db[track]),
            });
        }
        meter(ui, meter_rect, state.peaks[track]);

        // Peak readout.
        let peak_db = gain_to_db(state.peaks[track].max(1.0e-7));
        let peak_color = if peak_db > -3.0 {
            theme::RED
        } else if peak_db > -12.0 {
            theme::AMBER
        } else {
            theme::GREEN
        };
        ui.painter().text(
            Pos2::new(meter_rect.center().x, fader_bottom + 1.0),
            Align2::CENTER_TOP,
            format!("{peak_db:.0}"),
            theme::mono(6.2),
            peak_color,
        );

        // Gain readout (below peak on the left).
        ui.painter().text(
            Pos2::new(inner.left() + 1.0, fader_bottom + 1.0),
            Align2::LEFT_TOP,
            format!("{:+.1}", state.track_gain_db[track]),
            theme::mono(6.0),
            theme::TEXT_FAINT,
        );

        // Mute / solo.
        let button_y = inner.bottom() - bottom_h;
        let button_w = (inner.width() - 3.0) / 2.0;
        let mute_rect = Rect::from_min_size(
            Pos2::new(inner.left(), button_y),
            Vec2::new(button_w, bottom_h - 1.0),
        );
        let solo_rect = Rect::from_min_size(
            Pos2::new(mute_rect.right() + 3.0, button_y),
            Vec2::new(button_w, bottom_h - 1.0),
        );
        if toggle_chip(
            ui,
            mute_rect,
            ui.scope_id().with(("mute", track)),
            "M",
            state.mute[track],
            theme::AMBER,
        )
        .clicked()
        {
            state.mute[track] = !state.mute[track];
            let _ = audio.command_tx.push(Command::SetTrackMute {
                track: track as u8,
                mute: state.mute[track],
            });
        }
        if toggle_chip(
            ui,
            solo_rect,
            ui.scope_id().with(("solo", track)),
            "S",
            state.solo[track],
            theme::GREEN,
        )
        .clicked()
        {
            state.solo[track] = !state.solo[track];
            let _ = audio.command_tx.push(Command::SetTrackSolo {
                track: track as u8,
                solo: state.solo[track],
            });
        }
    }

    for track in 1..TRACK_COUNT {
        let x = content.left() + track as f32 * col_w;
        ui.painter().line_segment(
            [Pos2::new(x, content.top()), Pos2::new(x, content.bottom())],
            Stroke::new(1.0, theme::BORDER_SOFT),
        );
    }
}

fn pan_text(pan: f32) -> String {
    if pan.abs() < 0.02 {
        "C".to_string()
    } else if pan < 0.0 {
        format!("L{:.0}", pan.abs() * 100.0)
    } else {
        format!("R{:.0}", pan * 100.0)
    }
}
