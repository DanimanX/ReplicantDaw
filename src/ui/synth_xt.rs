//! The NEURAL GEN XT editor: a cyberpunk control surface for the hybrid synth.
//!
//! Layout follows the signal path: oscillators, filters, envelopes, LFOs, then
//! the modulation matrix. Every control writes through the same parameter
//! surface the engine uses, so the rack strip and the editor never disagree.

use egui::{Align2, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};

use crate::audio::{AudioRuntime, Command, SynthParam, SynthXt, MOD_SLOTS, PRESETS, TRACK_COUNT};
use crate::state::UiState;
use crate::studio::Studio;
use crate::theme;
use crate::ui::widgets::knob;

const KNOB: f32 = 54.0;

pub fn draw(
    ctx: &egui::Context,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
) {
    if !studio.synth_open {
        return;
    }
    let mut open = true;
    let track = state.selected_track.min(TRACK_COUNT - 1);
    let accent = theme::track_accent(track);
    let mut changes: Vec<(SynthParam, f32)> = Vec::new();
    let mut preset_request: Option<usize> = None;
    let mut reset_request = false;

    let screen = ctx.content_rect();
    let width = (screen.width() - 60.0).clamp(760.0, 980.0);
    let x = (screen.right() - width - 12.0).max(12.0);

    egui::Window::new("NEURAL GEN XT // HYBRID SYNTH")
        .open(&mut open)
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(Stroke::new(1.0, theme::accent_alpha(accent, 150)))
                .corner_radius(CornerRadius::same(3))
                .inner_margin(8),
        )
        .default_pos(Pos2::new(x, 96.0))
        .default_size(Vec2::new(width, 790.0))
        .resizable(true)
        .show(ctx, |ui| {
            ui.set_width(width - 24.0);
            ui.set_max_width(width - 24.0);
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
            let patch = state.synth[track];

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("NEURAL GEN XT")
                        .font(theme::display(13.0))
                        .color(theme::CYAN),
                );
                ui.label(
                    RichText::new("HYBRID · DUAL OSC · DUAL FILTER")
                        .font(theme::mono(6.6))
                        .color(theme::TEXT_FAINT),
                );
                ui.separator();
                egui::ComboBox::from_id_salt("xt_track")
                    .selected_text(format!("{:02} {}", track + 1, state.track_label(track)))
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for index in 0..TRACK_COUNT {
                            if ui
                                .selectable_label(
                                    index == track,
                                    format!("{:02} {}", index + 1, state.track_label(index)),
                                )
                                .clicked()
                            {
                                state.selected_track = index;
                            }
                        }
                    });
                egui::ComboBox::from_id_salt("xt_preset")
                    .selected_text("PRESET")
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for (index, preset) in PRESETS.iter().enumerate() {
                            if ui.selectable_label(false, preset.name).clicked() {
                                preset_request = Some(index);
                            }
                        }
                    });
                if ui
                    .add(
                        egui::Button::new(RichText::new("INIT").font(theme::display(7.5)))
                            .fill(theme::PANEL_SUNK),
                    )
                    .on_hover_text("Back to the plain default patch")
                    .clicked()
                {
                    reset_request = true;
                }
                ui.label(
                    RichText::new(format!("· {} VOICES", patch.polyphony as u32))
                        .font(theme::mono(6.8))
                        .color(theme::TEXT_FAINT),
                );
            });

            scope_strip(ui, state, accent, width - 40.0);
            ui.add_space(2.0);

            let full_w = width - 40.0;
            let gap = 8.0;
            let left_w = full_w * 0.60;
            let right_w = full_w - left_w - gap;
            let filter_w = (left_w - 6.0) * 0.5;
            egui::ScrollArea::vertical()
                .id_salt("xt_body")
                .max_height(680.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(left_w, 0.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        oscillator_panel(ui, &patch, 0, accent, left_w, &mut changes);
                        oscillator_panel(ui, &patch, 1, accent, left_w, &mut changes);
                        response_panel(ui, &patch, accent, left_w);
                        ui.horizontal_top(|ui| {
                            for index in 0..2u8 {
                                ui.allocate_ui_with_layout(
                                    Vec2::new(filter_w, 0.0),
                                    egui::Layout::top_down(egui::Align::LEFT),
                                    |ui| {
                                        filter_panel(
                                            ui, &patch, index, accent, filter_w, &mut changes,
                                        );
                                    },
                                );
                            }
                        });
                    },
                );
                ui.add_space(gap);
                ui.allocate_ui_with_layout(
                    Vec2::new(right_w, 0.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        envelope_panel(ui, &patch, accent, right_w, &mut changes);
                        lfo_panel(ui, &patch, accent, right_w, &mut changes);
                        master_panel(ui, &patch, accent, right_w, &mut changes);
                    },
                );
            });

            ui.add_space(6.0);
            matrix_panel(ui, &patch, accent, full_w, &mut changes);
                });

            ui.add_space(2.0);
            ui.label(
                RichText::new(
                    "Double-click a knob to reset it. Presets load a full patch; every edit goes straight to the audio engine.",
                )
                .font(theme::mono(6.4))
                .color(theme::TEXT_FAINT),
            );
        });

    let mut patch = state.synth[track];
    if let Some(index) = preset_request {
        patch.load_preset(index);
        studio.notify(format!(
            "Preset {} · {}",
            index + 1,
            SynthXt::preset_name(index)
        ));
    }
    if reset_request {
        patch = SynthXt::default();
    }
    if preset_request.is_some() || reset_request {
        state.synth[track] = patch;
        for param in SynthParam::all() {
            let _ = audio.command_tx.push(Command::SetSynthParam {
                track: track as u8,
                param,
                value: patch.get(param),
            });
        }
    } else {
        for (param, value) in changes {
            patch.set(param, value);
            let _ = audio.command_tx.push(Command::SetSynthParam {
                track: track as u8,
                param,
                value: patch.get(param),
            });
        }
        state.synth[track] = patch;
    }
    studio.synth_open = open;
}

/// A slim oscilloscope fed by the master bus, so the editor breathes.
fn scope_strip(ui: &mut Ui, state: &UiState, accent: egui::Color32, width: f32) {
    let height = 44.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(2), theme::BG_DEEP);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, theme::BORDER_SOFT),
        egui::StrokeKind::Inside,
    );
    let samples: Vec<f32> = state.scope.iter().rev().take(600).rev().copied().collect();
    if samples.len() > 2 {
        let mid = rect.center().y;
        let amp = rect.height() * 0.42;
        let points: Vec<Pos2> = samples
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let x = rect.left() + rect.width() * index as f32 / samples.len() as f32;
                Pos2::new(x, mid - value.clamp(-1.0, 1.0) * amp)
            })
            .collect();
        painter.add(egui::Shape::line(
            points,
            Stroke::new(1.0, theme::accent_alpha(accent, 220)),
        ));
    } else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "SCOPE · SILENT",
            theme::mono(6.4),
            theme::TEXT_FAINT,
        );
    }
    painter.text(
        Pos2::new(rect.left() + 4.0, rect.top() + 3.0),
        Align2::LEFT_TOP,
        "MASTER SCOPE",
        theme::mono(5.8),
        theme::accent_alpha(accent, 150),
    );
}

fn panel(ui: &mut Ui, title: &str, accent: egui::Color32, width: f32, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::PANEL_SUNK)
        .stroke(Stroke::new(1.0, theme::accent_alpha(accent, 60)))
        .corner_radius(CornerRadius::same(2))
        .inner_margin(5)
        .show(ui, |ui| {
            let inner = (width - 12.0).max(120.0);
            ui.set_min_width(inner);
            ui.set_max_width(inner);
            ui.label(
                RichText::new(title)
                    .font(theme::display(8.2))
                    .color(theme::accent_alpha(accent, 235)),
            );
            body(ui);
        });
}

fn oscillator_panel(
    ui: &mut Ui,
    patch: &SynthXt,
    osc: u8,
    accent: egui::Color32,
    width: f32,
    changes: &mut Vec<(SynthParam, f32)>,
) {
    let index = osc as usize;
    let params = patch.osc[index];
    let title = if params.wave == crate::audio::synth_xt::Waveform::Wavetable {
        let table_idx = (params.table.round() as usize).min(crate::audio::synth_xt::WAVETABLE_COUNT - 1);
        format!(
            "OSC {} // SURGE WT · {} · {:.0}%",
            osc + 1,
            crate::audio::synth_xt::WAVETABLE_NAMES[table_idx],
            params.morph * 100.0
        )
    } else {
        format!("OSC {} // {}", osc + 1, params.wave.label())
    };
    panel(
        ui,
        &title,
        accent,
        width,
        |ui| {
            ui.horizontal(|ui| {
                for (choice, wave) in crate::audio::synth_xt::Waveform::ALL.iter().enumerate() {
                    if chip(ui, wave.label(), params.wave == *wave, accent) {
                        let param = SynthParam::OscWave(osc);
                        push(changes, param, choice as f32);
                    }
                }
            });

            if params.wave == crate::audio::synth_xt::Waveform::Wavetable {
                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("TABLE:")
                            .font(theme::mono(9.0))
                            .color(theme::TEXT_FAINT),
                    );
                    let current_table = (params.table.round() as usize).min(crate::audio::synth_xt::WAVETABLE_COUNT - 1);
                    egui::ComboBox::from_id_salt(format!("wt_table_{osc}"))
                        .selected_text(
                            RichText::new(crate::audio::synth_xt::WAVETABLE_NAMES[current_table])
                                .font(theme::display(10.0))
                                .color(theme::CYAN),
                        )
                        .width(180.0)
                        .show_ui(ui, |ui| {
                            for (idx, name) in crate::audio::synth_xt::WAVETABLE_NAMES.iter().enumerate() {
                                if ui
                                    .selectable_label(
                                        idx == current_table,
                                        RichText::new(format!("{:02} {}", idx + 1, name))
                                            .font(theme::display(9.5)),
                                    )
                                    .clicked()
                                {
                                    push(changes, SynthParam::OscTable(osc), idx as f32);
                                }
                            }
                        });
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("MORPH: {:.1}%", params.morph * 100.0))
                            .font(theme::mono(9.0))
                            .color(theme::TEXT_DIM),
                    );
                });

                // Surge XT 3D / Layered Wavetable Visualizer
                ui.add_space(2.0);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(width - 24.0, 48.0), Sense::hover());
                let painter = ui.painter().with_clip_rect(rect);
                painter.rect_filled(rect, CornerRadius::same(2), theme::PANEL_SUNK);
                painter.rect_stroke(rect, CornerRadius::same(2), Stroke::new(1.0, theme::BORDER_SOFT), egui::StrokeKind::Inside);

                let current_table = (params.table.round() as usize).min(crate::audio::synth_xt::WAVETABLE_COUNT - 1);
                // Draw inactive frames in subtle background perspective
                for frame_i in 0..crate::audio::synth_xt::WT_FRAMES {
                    let frame_norm = frame_i as f32 / (crate::audio::synth_xt::WT_FRAMES - 1) as f32;
                    let y_offset = (frame_i as f32 - 2.0) * 2.5;
                    let x_offset = (frame_i as f32 - 2.0) * 3.0;
                    let pts = 48;
                    let mut line_pts = Vec::with_capacity(pts);
                    for p in 0..pts {
                        let phase = p as f32 / (pts - 1) as f32;
                        let val = crate::audio::synth_xt::sample_wavetable(current_table, frame_norm, phase, 0.01);
                        let x = rect.left() + 10.0 + x_offset + (rect.width() - 26.0) * phase;
                        let y = rect.center().y + y_offset - val * 15.0;
                        line_pts.push(Pos2::new(x, y));
                    }
                    painter.add(egui::Shape::line(
                        line_pts,
                        Stroke::new(0.8, theme::accent_alpha(theme::TEXT_FAINT, 45)),
                    ));
                }

                // Draw active morphed waveform in bright glowing cyan
                let preview_pts = 64;
                let preview = crate::audio::synth_xt::get_wavetable_frame_preview(current_table, params.morph, preview_pts);
                let mut morph_line = Vec::with_capacity(preview_pts);
                for (p, &val) in preview.iter().enumerate() {
                    let phase = p as f32 / (preview_pts - 1) as f32;
                    let x = rect.left() + 10.0 + (rect.width() - 20.0) * phase;
                    let y = rect.center().y - val * 19.0;
                    morph_line.push(Pos2::new(x, y));
                }
                // Glow line + core line
                painter.add(egui::Shape::line(
                    morph_line.clone(),
                    Stroke::new(2.8, theme::accent_alpha(theme::CYAN, 70)),
                ));
                painter.add(egui::Shape::line(
                    morph_line,
                    Stroke::new(1.4, theme::CYAN),
                ));

                // Morph position indicator marker
                let marker_x = rect.left() + 10.0 + (rect.width() - 20.0) * params.morph;
                painter.line_segment(
                    [Pos2::new(marker_x, rect.top() + 3.0), Pos2::new(marker_x, rect.bottom() - 3.0)],
                    Stroke::new(1.2, theme::MAGENTA),
                );
            }

            ui.add_space(3.0);
            ui.horizontal(|ui| {
                if params.wave == crate::audio::synth_xt::Waveform::Wavetable {
                    small_knob(ui, patch, SynthParam::OscMorph(osc), theme::MAGENTA, changes, "");
                }
                for param in [
                    SynthParam::OscOctave(osc),
                    SynthParam::OscSemitone(osc),
                    SynthParam::OscFine(osc),
                    SynthParam::OscUnison(osc),
                    SynthParam::OscDetune(osc),
                    SynthParam::OscLevel(osc),
                    SynthParam::OscPan(osc),
                ] {
                    small_knob(ui, patch, param, accent, changes, "");
                }
            });
        },
    );
}

fn filter_panel(
    ui: &mut Ui,
    patch: &SynthXt,
    filter: u8,
    accent: egui::Color32,
    width: f32,
    changes: &mut Vec<(SynthParam, f32)>,
) {
    let params = patch.filter[filter as usize];
    panel(
        ui,
        &format!("FILTER {} // {}", filter + 1, params.kind.label()),
        accent,
        width,
        |ui| {
            ui.horizontal(|ui| {
                for (choice, kind) in crate::audio::synth_xt::FilterKind::ALL.iter().enumerate() {
                    if chip(ui, kind.label(), params.kind == *kind, accent) {
                        push(changes, SynthParam::FilterKind(filter), choice as f32);
                    }
                }
            });
            ui.horizontal(|ui| {
                for param in [
                    SynthParam::FilterCutoff(filter),
                    SynthParam::FilterResonance(filter),
                    SynthParam::FilterDrive(filter),
                    SynthParam::FilterKeytrack(filter),
                ] {
                    small_knob(ui, patch, param, accent, changes, "");
                }
            });
        },
    );
}

/// Magnitude response of the two filters, from the analog prototype of the
/// state-variable filter the engine runs.
fn response_panel(ui: &mut Ui, patch: &SynthXt, accent: egui::Color32, width: f32) {
    panel(ui, "FILTER RESPONSE (EST.)", accent, width, |ui| {
        let height = 118.0;
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new((width - 22.0).max(120.0), height), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(2), theme::BG_DEEP);
        // Frequency grid: 20 Hz to 20 kHz, log.
        // 0 dB sits a quarter down, so resonance peaks have headroom.
        let zero_y = rect.top() + rect.height() * 0.25;
        painter.line_segment(
            [
                Pos2::new(rect.left(), zero_y),
                Pos2::new(rect.right(), zero_y),
            ],
            Stroke::new(0.6, theme::accent_alpha(theme::BORDER, 200)),
        );
        painter.text(
            Pos2::new(rect.left() + 3.0, zero_y - 2.0),
            Align2::LEFT_BOTTOM,
            "0 dB",
            theme::mono(5.2),
            theme::TEXT_FAINT,
        );
        for (hz, label) in [
            (20.0_f32, "20"),
            (200.0_f32, "200"),
            (2_000.0_f32, "2k"),
            (20_000.0_f32, "20k"),
        ] {
            let x = rect.left()
                + (hz.log10() - 20.0_f32.log10()) / (20_000.0_f32.log10() - 20.0_f32.log10())
                    * rect.width();
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(0.5, theme::BORDER_SOFT),
            );
            painter.text(
                Pos2::new(x + 2.0, rect.bottom() - 2.0),
                Align2::LEFT_BOTTOM,
                label,
                theme::mono(5.4),
                theme::TEXT_FAINT,
            );
        }
        for (index, filter) in patch.filter.iter().enumerate() {
            let base = if index == 0 { accent } else { theme::MAGENTA };
            // A bypassed filter contributes nothing to the sound; say so.
            let color = if filter.enabled {
                base
            } else {
                theme::accent_alpha(base, 55)
            };
            let q = 0.5 + filter.resonance * 6.0;
            let points: Vec<Pos2> = (0..160)
                .map(|step| {
                    let unit = step as f32 / 159.0;
                    let hz = 20.0 * 1000.0_f32.powf(unit);
                    let fc = filter.cutoff.max(20.0);
                    let ratio = hz / fc;
                    let magnitude = match filter.kind {
                        crate::audio::FilterKind::LowPass12 => {
                            1.0 / ((1.0 - ratio * ratio).powi(2) + (ratio / q).powi(2)).sqrt()
                        }
                        crate::audio::FilterKind::LowPass24 => {
                            1.0 / ((1.0 - ratio * ratio).powi(2) + (ratio / q).powi(2))
                        }
                        crate::audio::FilterKind::HighPass12 => {
                            ratio * ratio
                                / ((1.0 - ratio * ratio).powi(2) + (ratio / q).powi(2)).sqrt()
                        }
                        crate::audio::FilterKind::HighPass24 => {
                            (ratio * ratio).powi(2)
                                / ((1.0 - ratio * ratio).powi(2) + (ratio / q).powi(2))
                        }
                        crate::audio::FilterKind::BandPass => {
                            (ratio / q)
                                / ((1.0 - ratio * ratio).powi(2) + (ratio / q).powi(2)).sqrt()
                        }
                        crate::audio::FilterKind::Notch => {
                            (1.0 - ratio * ratio).abs()
                                / ((1.0 - ratio * ratio).powi(2) + (ratio / q).powi(2)).sqrt()
                        }
                    }
                    .clamp(0.0, 4.0);
                    let db = 20.0 * magnitude.max(1e-4).log10();
                    let y = rect.top()
                        + 4.0
                        + (1.0 - (db / 36.0 + 1.0).clamp(0.0, 1.0)) * (rect.height() - 12.0);
                    Pos2::new(rect.left() + unit * rect.width(), y)
                })
                .collect();
            painter.add(egui::Shape::line(
                points,
                Stroke::new(1.2, theme::accent_alpha(color, 200)),
            ));
            let x = rect.left()
                + ((filter.cutoff.max(20.0).log10() - 20.0_f32.log10())
                    / (20_000.0_f32.log10() - 20.0_f32.log10()))
                .clamp(0.0, 1.0)
                    * rect.width();
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(0.8, theme::accent_alpha(color, 90)),
            );
        }
    });
}

fn envelope_panel(
    ui: &mut Ui,
    patch: &SynthXt,
    accent: egui::Color32,
    width: f32,
    changes: &mut Vec<(SynthParam, f32)>,
) {
    panel(ui, "ENVELOPES", accent, width, |ui| {
        for env in 0..crate::audio::synth_xt::ENV_COUNT as u8 {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(env_row_label(env))
                        .font(theme::mono(6.2))
                        .color(theme::accent_alpha(accent, 190)),
                );
                for param in [
                    SynthParam::EnvAttack(env),
                    SynthParam::EnvDecay(env),
                    SynthParam::EnvSustain(env),
                    SynthParam::EnvRelease(env),
                ] {
                    small_knob(ui, patch, param, accent, changes, "");
                }
            });
        }
        ui.horizontal(|ui| {
            small_knob(ui, patch, SynthParam::EnvAmount, accent, changes, "");
            ui.label(
                RichText::new("ENV 2 → CUTOFF")
                    .font(theme::mono(6.0))
                    .color(theme::TEXT_FAINT),
            );
        });
    });
}

fn lfo_panel(
    ui: &mut Ui,
    patch: &SynthXt,
    accent: egui::Color32,
    width: f32,
    changes: &mut Vec<(SynthParam, f32)>,
) {
    panel(ui, "LFO", accent, width, |ui| {
        for lfo in 0..crate::audio::synth_xt::LFO_COUNT as u8 {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("LFO {}", lfo + 1))
                        .font(theme::mono(6.2))
                        .color(theme::accent_alpha(accent, 190)),
                );
                for (choice, shape) in crate::audio::synth_xt::LfoShape::ALL.iter().enumerate() {
                    if chip(
                        ui,
                        shape.label(),
                        patch.lfo[lfo as usize].shape == *shape,
                        accent,
                    ) {
                        push(changes, SynthParam::LfoShape(lfo), choice as f32);
                    }
                }
            });
            ui.horizontal(|ui| {
                small_knob(ui, patch, SynthParam::LfoRate(lfo), accent, changes, "");
                small_knob(ui, patch, SynthParam::LfoDepth(lfo), accent, changes, "");
            });
        }
    });
}

fn master_panel(
    ui: &mut Ui,
    patch: &SynthXt,
    accent: egui::Color32,
    width: f32,
    changes: &mut Vec<(SynthParam, f32)>,
) {
    panel(ui, "VOICE", accent, width, |ui| {
        ui.horizontal(|ui| {
            small_knob(ui, patch, SynthParam::Volume, accent, changes, "");
            small_knob(ui, patch, SynthParam::Noise, accent, changes, "");
            small_knob(ui, patch, SynthParam::Glide, accent, changes, "");
            small_knob(ui, patch, SynthParam::Polyphony, accent, changes, "");
            let (rect, response) = ui.allocate_exact_size(Vec2::new(84.0, 20.0), Sense::click());
            let serial = patch.serial_filters;
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(2),
                if serial {
                    theme::accent_alpha(accent, 40)
                } else {
                    theme::PANEL
                },
            );
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(2),
                Stroke::new(1.0, theme::accent_alpha(accent, 140)),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                if serial { "SERIAL" } else { "PARALLEL" },
                theme::display(7.0),
                accent,
            );
            if response.clicked() {
                push(
                    changes,
                    SynthParam::SerialFilters,
                    if serial { 0.0 } else { 1.0 },
                );
            }
            if response.hovered() {
                response.on_hover_text("Filter routing");
            }
        });
    });
}

fn matrix_panel(
    ui: &mut Ui,
    patch: &SynthXt,
    accent: egui::Color32,
    width: f32,
    changes: &mut Vec<(SynthParam, f32)>,
) {
    panel(ui, "MODULATION MATRIX", accent, width, |ui| {
        // A grid keeps both columns on the same baseline.
        let bar_w = ((width - 60.0) * 0.5 - 250.0).clamp(80.0, 170.0);
        egui::Grid::new("xt_matrix")
            .num_columns(2)
            .spacing(Vec2::new(14.0, 3.0))
            .show(ui, |ui| {
                for row in 0..3u8 {
                    for column in 0..2u8 {
                        let slot = column * 3 + row;
                        if slot >= MOD_SLOTS as u8 {
                            continue;
                        }
                        route_row(ui, patch, slot, accent, bar_w, changes);
                    }
                    ui.end_row();
                }
            });
    });
}

/// One modulation route: source, destination and a bipolar amount bar.
fn route_row(
    ui: &mut Ui,
    patch: &SynthXt,
    slot: u8,
    accent: egui::Color32,
    bar_w: f32,
    changes: &mut Vec<(SynthParam, f32)>,
) {
    let route = patch.matrix[slot as usize];
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{:02}", slot + 1))
                .font(theme::mono(6.2))
                .color(theme::TEXT_FAINT),
        );
        egui::ComboBox::from_id_salt(("xt_mod_src", slot))
            .selected_text(route.source.label())
            .width(74.0)
            .show_ui(ui, |ui| {
                for (choice, source) in crate::audio::synth_xt::ModSource::ALL.iter().enumerate() {
                    if ui
                        .selectable_label(route.source == *source, source.label())
                        .clicked()
                    {
                        push(changes, SynthParam::ModSource(slot), choice as f32);
                    }
                }
            });
        egui::ComboBox::from_id_salt(("xt_mod_dst", slot))
            .selected_text(route.dest.label())
            .width(104.0)
            .show_ui(ui, |ui| {
                for (choice, dest) in crate::audio::synth_xt::ModDest::ALL.iter().enumerate() {
                    if ui
                        .selectable_label(route.dest == *dest, dest.label())
                        .clicked()
                    {
                        push(changes, SynthParam::ModDest(slot), choice as f32);
                    }
                }
            });
        let mut amount = route.amount;
        let (bar, response) =
            ui.allocate_exact_size(Vec2::new(bar_w, 15.0), Sense::click_and_drag());
        if response.dragged() || response.clicked() {
            if let Some(pointer) = ui.ctx().pointer_interact_pos() {
                let unit = ((pointer.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
                amount = unit * 2.0 - 1.0;
                push(changes, SynthParam::ModAmount(slot), amount);
            }
        }
        paint_amount(ui, bar, amount, accent);
        ui.label(
            RichText::new(format!("{amount:+.2}"))
                .font(theme::mono(6.2))
                .color(accent),
        );
    });
}

fn small_knob(
    ui: &mut Ui,
    patch: &SynthXt,
    param: SynthParam,
    accent: egui::Color32,
    changes: &mut Vec<(SynthParam, f32)>,
    override_label: &str,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(KNOB, KNOB), Sense::hover());
    let (min, max) = param.range();
    let log = param.logarithmic();
    let value = patch.get(param);
    let mut norm = to_norm(value, min, max, log);
    let label = if override_label.is_empty() {
        short_label(param)
    } else {
        override_label.to_string()
    };
    let changed = knob(
        ui,
        rect,
        ui.scope_id().with(("xt_knob", param.id())),
        &mut norm,
        0.0..=1.0,
        to_norm(param.default_value(), min, max, log),
        &label,
        &param.format(value),
        accent,
    );
    let response = ui.interact(
        rect,
        ui.scope_id().with(("xt_knob_hit", param.id())),
        Sense::hover(),
    );
    if response.hovered() {
        response.on_hover_text(param.label());
    }
    if changed {
        push(changes, param, from_norm(norm, min, max, log));
    }
}

/// Bipolar modulation amount: centre is zero, fill grows to either side.
fn paint_amount(ui: &mut Ui, rect: Rect, amount: f32, accent: egui::Color32) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), theme::BG_DEEP);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, theme::BORDER_SOFT),
        egui::StrokeKind::Inside,
    );
    let center = rect.center().x;
    let x = rect.left() + (amount * 0.5 + 0.5) * rect.width();
    let fill = Rect::from_min_max(
        Pos2::new(center.min(x), rect.top() + 3.0),
        Pos2::new(center.max(x), rect.bottom() - 3.0),
    );
    painter.rect_filled(
        fill,
        CornerRadius::same(1),
        theme::accent_alpha(accent, 170),
    );
    painter.line_segment(
        [
            Pos2::new(center, rect.top() + 1.0),
            Pos2::new(center, rect.bottom() - 1.0),
        ],
        Stroke::new(0.8, theme::TEXT_FAINT),
    );
    painter.line_segment(
        [
            Pos2::new(x, rect.top() + 2.0),
            Pos2::new(x, rect.bottom() - 2.0),
        ],
        Stroke::new(1.2, theme::TEXT),
    );
}

fn push(changes: &mut Vec<(SynthParam, f32)>, param: SynthParam, value: f32) {
    changes.push((param, value));
}

fn chip(ui: &mut Ui, label: &str, selected: bool, accent: egui::Color32) -> bool {
    let response = ui.add(
        egui::Button::new(
            RichText::new(label)
                .font(theme::mono(6.2))
                .color(if selected { accent } else { theme::TEXT_DIM }),
        )
        .fill(if selected {
            theme::accent_alpha(accent, 40)
        } else {
            theme::PANEL
        })
        .stroke(Stroke::new(
            1.0,
            if selected {
                theme::accent_alpha(accent, 170)
            } else {
                theme::BORDER_SOFT
            },
        ))
        .min_size(Vec2::new(0.0, 16.0)),
    );
    response.clicked()
}

/// Caption that fits under a 54 px knob. The full name is the tooltip.
fn short_label(param: SynthParam) -> String {
    match param {
        SynthParam::OscWave(_) => "WAVE".into(),
        SynthParam::OscTable(_) => "TABLE".into(),
        SynthParam::OscMorph(_) => "MORPH".into(),
        SynthParam::OscOctave(_) => "OCT".into(),
        SynthParam::OscSemitone(_) => "SEMI".into(),
        SynthParam::OscFine(_) => "FINE".into(),
        SynthParam::OscLevel(_) => "LVL".into(),
        SynthParam::OscPan(_) => "PAN".into(),
        SynthParam::OscUnison(_) => "UNI".into(),
        SynthParam::OscDetune(_) => "DET".into(),
        SynthParam::OscPhase(_) => "PHASE".into(),
        SynthParam::Noise => "NOISE".into(),
        SynthParam::FilterEnabled(_) => "ON".into(),
        SynthParam::FilterKind(_) => "TYPE".into(),
        SynthParam::FilterCutoff(_) => "CUT".into(),
        SynthParam::FilterResonance(_) => "RES".into(),
        SynthParam::FilterDrive(_) => "DRV".into(),
        SynthParam::FilterKeytrack(_) => "KEY".into(),
        SynthParam::SerialFilters => "ROUTE".into(),
        SynthParam::EnvAttack(_) => "A".into(),
        SynthParam::EnvDecay(_) => "D".into(),
        SynthParam::EnvSustain(_) => "S".into(),
        SynthParam::EnvRelease(_) => "R".into(),
        SynthParam::EnvAmount => "ENV>CUT".into(),
        SynthParam::LfoShape(_) => "SHAPE".into(),
        SynthParam::LfoRate(_) => "RATE".into(),
        SynthParam::LfoDepth(_) => "DEPTH".into(),
        SynthParam::ModSource(_) => "SRC".into(),
        SynthParam::ModDest(_) => "DST".into(),
        SynthParam::ModAmount(_) => "AMT".into(),
        SynthParam::Volume => "VOL".into(),
        SynthParam::Glide => "GLIDE".into(),
        SynthParam::Polyphony => "POLY".into(),
    }
}

/// Short name for the envelope rows ("AMP A/D/S/R").
fn env_row_label(env: u8) -> &'static str {
    match env {
        0 => "AMP",
        1 => "FILTER",
        _ => "MOD",
    }
}

fn to_norm(value: f32, min: f32, max: f32, log: bool) -> f32 {
    if max <= min {
        return 0.0;
    }
    let value = value.clamp(min, max);
    if log && min > 0.0 {
        (value / min).ln() / (max / min).ln()
    } else {
        (value - min) / (max - min)
    }
}

fn from_norm(norm: f32, min: f32, max: f32, log: bool) -> f32 {
    let norm = norm.clamp(0.0, 1.0);
    if log && min > 0.0 {
        min * (max / min).powf(norm)
    } else {
        min + (max - min) * norm
    }
}
