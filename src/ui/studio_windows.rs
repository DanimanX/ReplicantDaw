//! Menu bar and utility windows: plugin rack, plugin browser, MIDI and projects.

use egui::{
    Align2, Button, Color32, ComboBox, CornerRadius, Frame, Grid, Pos2, Rect, RichText, ScrollArea,
    Sense, Stroke, Ui, Vec2,
};

use crate::audio::{AudioRuntime, Command};
use crate::plugins::{PluginFormat, MAX_PLUGIN_SLOTS};
use crate::state::UiState;
use crate::studio::Studio;
use crate::theme;
use crate::ui::widgets::knob;

/// Thin system bar above the transport.
pub fn draw_menu_bar(ui: &mut Ui, rect: Rect, studio: &mut Studio, state: &mut UiState) {
    let painter = ui.painter().clone();
    painter.rect_filled(rect, CornerRadius::same(2), theme::BG_DEEP);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, theme::BORDER_SOFT),
        egui::StrokeKind::Inside,
    );

    let button = |ui: &mut Ui, label: &str, active: bool| -> bool {
        let text = RichText::new(label)
            .font(theme::display(8.5))
            .color(if active { theme::CYAN } else { theme::TEXT_DIM });
        let button = Button::new(text)
            .fill(if active {
                theme::PANEL_RAISED
            } else {
                theme::BG
            })
            .stroke(Stroke::new(
                1.0,
                if active {
                    theme::CYAN
                } else {
                    theme::BORDER_SOFT
                },
            ))
            .corner_radius(CornerRadius::same(2));
        ui.add(button).clicked()
    };

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(8.0, 3.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);

    if button(&mut child, "FILE", studio.project_open) {
        studio.project_open = !studio.project_open;
        studio.refresh_project_list();
    }
    if button(&mut child, "PLUGINS", studio.browser_open) {
        studio.browser_open = !studio.browser_open;
    }
    if button(&mut child, "RACK", studio.rack_open) {
        studio.rack_open = !studio.rack_open;
    }
    if button(&mut child, "MIDI", studio.midi_open) {
        studio.midi_open = !studio.midi_open;
    }

    child.add_space(6.0);
    child.label(
        RichText::new(studio.project_label())
            .font(theme::mono(8.5))
            .color(theme::TEXT),
    );
    child.label(
        RichText::new(format!(
            "{} plugin(s)",
            studio.slots.iter().flatten().count()
        ))
        .font(theme::mono(7.5))
        .color(theme::TEXT_FAINT),
    );

    // Recording indicator.
    if state.recording {
        child.add_space(6.0);
        child.label(
            RichText::new(format!(
                "● REC  TRK {:02}  {} NOTES",
                state.record_track + 1,
                state.recorded_total
            ))
            .font(theme::mono(8.5))
            .color(theme::RED),
        );
    }

    let status = if studio.status.is_empty() {
        "READY"
    } else {
        studio.status.as_str()
    };
    painter.text(
        Pos2::new(rect.right() - 8.0, rect.center().y),
        Align2::RIGHT_CENTER,
        status.chars().take(90).collect::<String>(),
        theme::mono(7.5),
        theme::TEXT_FAINT,
    );
}

/// Draw all utility windows.
pub fn draw_windows(
    ctx: &egui::Context,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
) {
    draw_rack(ctx, studio, state);
    draw_browser(ctx, studio, state);
    draw_midi(ctx, studio, state, audio);
    draw_project(ctx, studio, state, audio);
}

fn window_frame() -> Frame {
    Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(egui::Margin::same(8))
}

fn section(ui: &mut Ui, title: &str, accent: Color32) {
    ui.label(RichText::new(title).font(theme::display(9.0)).color(accent));
    let rect = ui.available_rect_before_wrap();
    ui.painter().line_segment(
        [
            Pos2::new(rect.left(), rect.top() + 2.0),
            Pos2::new(rect.right().min(rect.left() + 240.0), rect.top() + 2.0),
        ],
        Stroke::new(1.0, theme::accent_alpha(accent, 60)),
    );
    ui.add_space(4.0);
}

fn draw_rack(ctx: &egui::Context, studio: &mut Studio, state: &mut UiState) {
    let mut open = studio.rack_open;
    let mut close = false;
    egui::Window::new("PLUGIN RACK // MASTER INSERT CHAIN")
        .open(&mut open)
        .frame(window_frame())
        .default_pos(Pos2::new(300.0, 120.0))
        .default_width(760.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.label(
                RichText::new("Plugins process the master bus on a dedicated worker thread.")
                    .font(theme::mono(8.0))
                    .color(theme::TEXT_FAINT),
            );
            ui.add_space(6.0);

            for slot in 0..MAX_PLUGIN_SLOTS {
                let info = studio.slots[slot].clone();
                let pending = studio.slot_pending[slot];
                Frame::new()
                    .fill(theme::PANEL_SUNK)
                    .stroke(Stroke::new(1.0, theme::BORDER_SOFT))
                    .corner_radius(CornerRadius::same(2))
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{:02}", slot + 1))
                                    .font(theme::display(10.0))
                                    .color(theme::CYAN),
                            );
                            match &info {
                                Some(info) => {
                                    ui.label(
                                        RichText::new(&info.name)
                                            .font(theme::display(9.0))
                                            .color(theme::TEXT),
                                    );
                                    ui.label(
                                        RichText::new(info.format.label())
                                            .font(theme::mono(7.5))
                                            .color(theme::accent_alpha(theme::MAGENTA, 220)),
                                    );
                                    ui.label(
                                        RichText::new(format!(
                                            "v{} · latency {} smp",
                                            info.version,
                                            info.latency + state.plugin_latency as u32
                                        ))
                                        .font(theme::mono(7.0))
                                        .color(theme::TEXT_FAINT),
                                    );
                                }
                                None if pending => {
                                    ui.label(
                                        RichText::new("LOADING…")
                                            .font(theme::mono(8.0))
                                            .color(theme::AMBER),
                                    );
                                }
                                None => {
                                    ui.label(
                                        RichText::new("EMPTY")
                                            .font(theme::mono(8.0))
                                            .color(theme::TEXT_FAINT),
                                    );
                                }
                            }

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if info.is_some() {
                                        if ui
                                            .add(Button::new(
                                                RichText::new("X").font(theme::display(8.0)),
                                            ))
                                            .clicked()
                                        {
                                            studio.unload_plugin(slot);
                                        }
                                        let label = if info.as_ref().is_some_and(|i| i.bypass) {
                                            "BYPASSED"
                                        } else {
                                            "BYPASS"
                                        };
                                        if ui
                                            .add(Button::new(
                                                RichText::new(label).font(theme::display(8.0)),
                                            ))
                                            .clicked()
                                        {
                                            let bypass = !info.as_ref().is_some_and(|i| i.bypass);
                                            studio.set_bypass(slot, bypass);
                                        }
                                    }
                                    if ui
                                        .add(Button::new(
                                            RichText::new("LOAD").font(theme::display(8.0)),
                                        ))
                                        .clicked()
                                    {
                                        studio.browser_target = slot;
                                        studio.browser_open = true;
                                        if studio.scan_results.is_empty() && !studio.scan_running {
                                            studio.start_scan();
                                        }
                                    }
                                    ui.label(
                                        RichText::new(format!("SLOT {}", slot + 1))
                                            .font(theme::mono(7.0))
                                            .color(theme::TEXT_FAINT),
                                    );
                                },
                            );
                        });

                        if let Some(info) = &info {
                            if let Some(error) = &info.error {
                                ui.label(
                                    RichText::new(error)
                                        .font(theme::mono(7.5))
                                        .color(theme::RED),
                                );
                            }
                            if !info.params.is_empty() {
                                ui.add_space(4.0);
                                let mut changes = Vec::new();
                                Grid::new(("params", slot))
                                    .num_columns(8)
                                    .spacing(Vec2::new(6.0, 4.0))
                                    .show(ui, |ui| {
                                        for (index, param) in
                                            info.params.iter().take(16).enumerate()
                                        {
                                            let mut value = param.value;
                                            let (rect, _) = ui.allocate_exact_size(
                                                Vec2::new(72.0, 52.0),
                                                Sense::hover(),
                                            );
                                            let accent = if info.bypass {
                                                theme::TEXT_FAINT
                                            } else {
                                                theme::CYAN
                                            };
                                            let text = format!("{:.0}%", value * 100.0);
                                            if knob(
                                                ui,
                                                rect,
                                                ui.scope_id().with(("plugin_param", slot, index)),
                                                &mut value,
                                                0.0..=1.0,
                                                param.value,
                                                &param.name.chars().take(9).collect::<String>(),
                                                &text,
                                                accent,
                                            ) {
                                                changes.push((param.id, value));
                                            }
                                            if (index + 1) % 8 == 0 {
                                                ui.end_row();
                                            }
                                        }
                                    });
                                for (id, value) in changes {
                                    if let Some(info) = &studio.slots[slot] {
                                        if let Some(param) =
                                            info.params.iter().find(|p| p.id == id).cloned()
                                        {
                                            studio.set_param(slot, &param, value);
                                        }
                                    }
                                }
                            }
                        }
                    });
                ui.add_space(4.0);
            }

            if !studio.logs.is_empty() {
                ui.add_space(4.0);
                section(ui, "EVENTS", theme::AMBER);
                for line in studio.logs.iter().rev().take(4) {
                    ui.label(
                        RichText::new(line)
                            .font(theme::mono(7.5))
                            .color(theme::TEXT_DIM),
                    );
                }
            }

            if ui
                .add(Button::new(
                    RichText::new("CLOSE").font(theme::display(8.0)),
                ))
                .clicked()
            {
                close = true;
            }
        });
    if close {
        open = false;
    }
    studio.rack_open = open;
}

fn draw_browser(ctx: &egui::Context, studio: &mut Studio, _state: &mut UiState) {
    let mut open = studio.browser_open;
    egui::Window::new("PLUGIN BROWSER")
        .open(&mut open)
        .frame(window_frame())
        .default_pos(Pos2::new(260.0, 150.0))
        .default_width(620.0)
        .default_height(420.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("TARGET SLOT {:02}", studio.browser_target + 1))
                        .font(theme::display(9.0))
                        .color(theme::CYAN),
                );
                if ui
                    .add(Button::new(
                        RichText::new("RESCAN").font(theme::display(8.0)),
                    ))
                    .clicked()
                {
                    studio.start_scan();
                }
                if studio.scan_running {
                    ui.spinner();
                    ui.label(
                        RichText::new("SCANNING")
                            .font(theme::mono(8.0))
                            .color(theme::AMBER),
                    );
                }
                ui.label(
                    RichText::new(format!("{} found", studio.scan_results.len()))
                        .font(theme::mono(8.0))
                        .color(theme::TEXT_FAINT),
                );
            });
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::singleline(&mut studio.browser_filter)
                    .hint_text("filter plugins…")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(4.0);

            let filter = studio.browser_filter.to_lowercase();
            ScrollArea::vertical()
                .max_height(300.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if studio.scan_results.is_empty() && !studio.scan_running {
                        ui.label(
                            RichText::new(
                                "No plugins scanned yet. Press RESCAN, or set REPLICANT_DAW_PLUGIN_PATH.",
                            )
                            .font(theme::mono(8.0))
                            .color(theme::TEXT_DIM),
                        );
                    }
                    let mut to_load = None;
                    for descriptor in &studio.scan_results {
                        if !filter.is_empty()
                            && !descriptor.name.to_lowercase().contains(&filter)
                            && !descriptor.format.label().to_lowercase().contains(&filter)
                        {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            let badge = descriptor.format.label();
                            let color = match descriptor.format {
                                PluginFormat::Clap => theme::CYAN,
                                PluginFormat::Vst3 => theme::MAGENTA,
                                PluginFormat::Vst2 => theme::AMBER,
                            };
                            ui.label(
                                RichText::new(format!("{badge:<4}"))
                                    .font(theme::mono(8.0))
                                    .color(color),
                            );
                            ui.label(
                                RichText::new(&descriptor.name)
                                    .font(theme::display(8.5))
                                    .color(theme::TEXT),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if descriptor.hostable() {
                                        if ui
                                            .add(Button::new(
                                                RichText::new("LOAD").font(theme::display(8.0)),
                                            ))
                                            .clicked()
                                        {
                                            to_load = Some(descriptor.clone());
                                        }
                                    } else {
                                        ui.label(
                                            RichText::new("not hostable")
                                                .font(theme::mono(7.0))
                                                .color(theme::TEXT_FAINT),
                                        );
                                    }
                                    ui.label(
                                        RichText::new(
                                            descriptor
                                                .path
                                                .parent()
                                                .map(|p| p.to_string_lossy().to_string())
                                                .unwrap_or_default(),
                                        )
                                        .font(theme::mono(7.0))
                                        .color(theme::TEXT_FAINT),
                                    );
                                },
                            );
                        });
                    }
                    if let Some(descriptor) = to_load {
                        let slot = studio.browser_target;
                        studio.load_plugin(slot, &descriptor);
                        studio.browser_open = false;
                    }
                });
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "CLAP and VST3 are hosted in-process. VST2 requires the proprietary SDK and is listed only.",
                )
                .font(theme::mono(7.0))
                .color(theme::TEXT_FAINT),
            );
        });
    studio.browser_open = open;
}

fn draw_midi(
    ctx: &egui::Context,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
) {
    let mut open = studio.midi_open;
    egui::Window::new("MIDI // INPUT & RECORDING")
        .open(&mut open)
        .frame(window_frame())
        .default_pos(Pos2::new(880.0, 300.0))
        .default_width(460.0)
        .resizable(true)
        .show(ctx, |ui| {
            section(ui, "INPUT PORTS", theme::CYAN);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} port(s)", studio.midi.ports.len()))
                        .font(theme::mono(8.0))
                        .color(theme::TEXT_DIM),
                );
                if ui
                    .add(Button::new(
                        RichText::new("RESCAN").font(theme::display(8.0)),
                    ))
                    .clicked()
                {
                    studio.midi.rescan();
                }
                if studio.midi.is_connected()
                    && ui
                        .add(Button::new(
                            RichText::new("DISCONNECT").font(theme::display(8.0)),
                        ))
                        .clicked()
                {
                    studio.midi.close();
                }
            });
            for (index, port) in studio.midi.ports.clone().iter().enumerate() {
                ui.horizontal(|ui| {
                    let connected = studio.midi.connected == Some(index);
                    ui.label(
                        RichText::new(if connected { "●" } else { "○" })
                            .font(theme::mono(9.0))
                            .color(if connected {
                                theme::GREEN
                            } else {
                                theme::TEXT_FAINT
                            }),
                    );
                    ui.label(
                        RichText::new(port)
                            .font(theme::mono(8.0))
                            .color(if connected {
                                theme::TEXT
                            } else {
                                theme::TEXT_DIM
                            }),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !connected
                            && ui
                                .add(Button::new(
                                    RichText::new("CONNECT").font(theme::display(8.0)),
                                ))
                                .clicked()
                        {
                            match studio.midi.open(index) {
                                Ok(()) => studio.notify(format!("MIDI connected: {port}")),
                                Err(error) => studio.notify(error),
                            }
                        }
                    });
                });
            }
            if studio.midi.ports.is_empty() {
                ui.label(
                    RichText::new("No MIDI inputs detected. Use the on-screen keys below.")
                        .font(theme::mono(8.0))
                        .color(theme::TEXT_FAINT),
                );
            }

            ui.add_space(6.0);
            section(ui, "ON-SCREEN KEYBOARD", theme::MAGENTA);
            draw_keyboard(ui, studio, state);

            ui.add_space(6.0);
            section(ui, "RECORDING", theme::AMBER);
            ui.horizontal(|ui| {
                let armed = state.recording;
                let label = if armed { "STOP REC" } else { "ARM + REC" };
                let text = RichText::new(label)
                    .font(theme::display(8.5))
                    .color(if armed { theme::RED } else { theme::TEXT });
                if ui
                    .add(
                        Button::new(text)
                            .fill(if armed {
                                theme::accent_alpha(theme::RED, 40)
                            } else {
                                theme::PANEL_SUNK
                            })
                            .stroke(Stroke::new(
                                1.0,
                                if armed { theme::RED } else { theme::BORDER },
                            )),
                    )
                    .clicked()
                {
                    if armed {
                        let _ = audio.command_tx.push(Command::RecordStop);
                        state.recording = false;
                        studio.notify("Recording stopped");
                    } else {
                        let track = state.selected_track as u8;
                        state.recording = true;
                        state.record_track = track;
                        state.clear_recorded(track as usize);
                        let _ = audio.command_tx.push(Command::RecordStart { track });
                        studio.notify(format!("Recording on track {:02}", track + 1));
                    }
                }
                ui.label(
                    RichText::new(format!("{} notes captured", state.recorded_total))
                        .font(theme::mono(8.0))
                        .color(theme::TEXT_DIM),
                );
            });

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("TARGET TRACK")
                        .font(theme::mono(8.0))
                        .color(theme::TEXT_FAINT),
                );
                ComboBox::from_id_salt("midi_track")
                    .selected_text(format!("{:02}", state.selected_track + 1))
                    .show_ui(ui, |ui| {
                        for track in 0..crate::audio::TRACK_COUNT {
                            if ui
                                .selectable_label(
                                    state.selected_track == track,
                                    format!(
                                        "{:02} {}",
                                        track + 1,
                                        crate::audio::TRACKS[track].name
                                    ),
                                )
                                .clicked()
                            {
                                state.selected_track = track;
                                state.input_track = track as u8;
                                let _ = audio
                                    .command_tx
                                    .push(Command::SetInputTrack { track: track as u8 });
                            }
                        }
                    });
                if ui
                    .add(Button::new(
                        RichText::new("CLEAR CLIP").font(theme::display(8.0)),
                    ))
                    .clicked()
                {
                    let track = state.selected_track;
                    state.clear_recorded(track);
                    let _ = audio
                        .command_tx
                        .push(Command::ClearRecorded { track: track as u8 });
                }
                let mut midi_to_plugins = state.plugin_midi;
                if ui
                    .checkbox(
                        &mut midi_to_plugins,
                        RichText::new("MIDI → PLUGINS").font(theme::mono(8.0)),
                    )
                    .changed()
                {
                    state.plugin_midi = midi_to_plugins;
                    let _ = audio.command_tx.push(Command::SetPluginMidi {
                        enabled: midi_to_plugins,
                    });
                }
            });
        });
    studio.midi_open = open;
}

fn draw_keyboard(ui: &mut Ui, studio: &mut Studio, state: &mut UiState) {
    // Two octaves, chromatic. White keys are 14, black keys are the usual gaps.
    let white_count = 14;
    let height = 64.0;
    let width = ui.available_width().min(440.0);
    let key_w = width / white_count as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();

    let white_semitone = [0, 2, 4, 5, 7, 9, 11];
    for octave in 0..2 {
        for (i, semitone) in white_semitone.iter().enumerate() {
            let note = octave * 12 + semitone;
            let x = rect.left() + (octave * 7 + i) as f32 * key_w;
            let key_rect = Rect::from_min_size(
                Pos2::new(x + 1.0, rect.top()),
                Vec2::new(key_w - 2.0, height),
            );
            let response = ui.interact(
                key_rect,
                ui.scope_id().with(("white_key", note)),
                Sense::click_and_drag(),
            );
            let pressed = response.is_pointer_button_down_on();
            let slot = note.min(23);
            let active = studio.virtual_keys[slot];
            if pressed && !active {
                studio.virtual_keys[slot] = true;
                studio.send_virtual_note(slot);
                state.input_track = state.selected_track as u8;
            } else if !pressed && active {
                studio.virtual_keys[slot] = false;
                studio.release_virtual_note(slot);
            }
            painter.rect_filled(
                key_rect,
                CornerRadius::same(2),
                if pressed {
                    theme::accent_alpha(theme::CYAN, 70)
                } else {
                    Color32::from_rgb(0x16, 0x22, 0x2e)
                },
            );
            painter.rect_stroke(
                key_rect,
                CornerRadius::same(2),
                Stroke::new(1.0, theme::BORDER),
                egui::StrokeKind::Inside,
            );
            painter.text(
                Pos2::new(key_rect.center().x, key_rect.bottom() - 6.0),
                Align2::CENTER_BOTTOM,
                format!("{}", 60 + note),
                theme::mono(6.5),
                theme::TEXT_FAINT,
            );
        }
    }

    // Black keys drawn on top.
    let black_offsets = [1.0_f32, 3.0, 6.0, 8.0, 10.0];
    for octave in 0..2 {
        for (i, offset) in black_offsets.iter().enumerate() {
            let note = octave * 12 + [1, 3, 6, 8, 10][i];
            let x = rect.left() + (octave * 7) as f32 * key_w + offset * key_w - key_w * 0.32;
            let key_rect = Rect::from_min_size(
                Pos2::new(x, rect.top()),
                Vec2::new(key_w * 0.64, height * 0.60),
            );
            let response = ui.interact(
                key_rect,
                ui.scope_id().with(("black_key", note)),
                Sense::click_and_drag(),
            );
            let pressed = response.is_pointer_button_down_on();
            let active = studio.virtual_keys[note.min(23)];
            if pressed && !active {
                studio.virtual_keys[note.min(23)] = true;
                studio.send_virtual_note(note.min(23));
            } else if !pressed && active {
                studio.virtual_keys[note.min(23)] = false;
                studio.release_virtual_note(note.min(23));
            }
            painter.rect_filled(
                key_rect,
                CornerRadius::same(2),
                if pressed {
                    theme::MAGENTA
                } else {
                    theme::BG_DEEP
                },
            );
            painter.rect_stroke(
                key_rect,
                CornerRadius::same(2),
                Stroke::new(1.0, theme::BORDER),
                egui::StrokeKind::Inside,
            );
        }
    }

    ui.label(
        RichText::new("Click/hold keys to play; arm REC to capture a clip.")
            .font(theme::mono(7.0))
            .color(theme::TEXT_FAINT),
    );
}

fn draw_project(
    ctx: &egui::Context,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
) {
    let mut open = studio.project_open;
    egui::Window::new("PROJECT")
        .open(&mut open)
        .frame(window_frame())
        .default_pos(Pos2::new(200.0, 130.0))
        .default_width(520.0)
        .resizable(true)
        .show(ctx, |ui| {
            section(ui, "CURRENT", theme::CYAN);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("NAME")
                        .font(theme::mono(8.0))
                        .color(theme::TEXT_FAINT),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut studio.project_name)
                        .desired_width(220.0)
                        .hint_text("Untitled"),
                );
            });
            ui.label(
                RichText::new(match &studio.project_path {
                    Some(path) => path.display().to_string(),
                    None => "not saved yet".to_string(),
                })
                .font(theme::mono(7.5))
                .color(theme::TEXT_FAINT),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add(Button::new(RichText::new("SAVE").font(theme::display(8.5))))
                    .clicked()
                {
                    match studio.save_project(state) {
                        Ok(path) => studio.notify(format!("Saved {}", path.display())),
                        Err(error) => studio.notify(error),
                    }
                }
                if ui
                    .add(Button::new(
                        RichText::new("SAVE AS").font(theme::display(8.5)),
                    ))
                    .clicked()
                {
                    let path = crate::project::Project::path_for_name(&studio.project_name);
                    match studio.save_project_as(state, path) {
                        Ok(path) => studio.notify(format!("Saved {}", path.display())),
                        Err(error) => studio.notify(error),
                    }
                }
                if ui
                    .add(Button::new(
                        RichText::new("REFRESH").font(theme::display(8.5)),
                    ))
                    .clicked()
                {
                    studio.refresh_project_list();
                }
            });

            ui.add_space(8.0);
            section(ui, "SAVED PROJECTS", theme::MAGENTA);
            ui.label(
                RichText::new(
                    crate::project::Project::projects_dir()
                        .display()
                        .to_string(),
                )
                .font(theme::mono(7.0))
                .color(theme::TEXT_FAINT),
            );
            ScrollArea::vertical()
                .max_height(220.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if studio.projects.is_empty() {
                        ui.label(
                            RichText::new("No saved projects yet.")
                                .font(theme::mono(8.0))
                                .color(theme::TEXT_DIM),
                        );
                    }
                    let mut to_load = None;
                    for path in studio.projects.clone() {
                        ui.horizontal(|ui| {
                            let name = path
                                .file_stem()
                                .map(|stem| stem.to_string_lossy().to_string())
                                .unwrap_or_default();
                            ui.label(
                                RichText::new(name)
                                    .font(theme::display(8.5))
                                    .color(theme::TEXT),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add(Button::new(
                                            RichText::new("LOAD").font(theme::display(8.0)),
                                        ))
                                        .clicked()
                                    {
                                        to_load = Some(path.clone());
                                    }
                                },
                            );
                        });
                    }
                    if let Some(path) = to_load {
                        match studio.load_project(&path, state, audio) {
                            Ok(()) => studio.project_open = false,
                            Err(error) => studio.notify(error),
                        }
                    }
                });

            ui.add_space(6.0);
            section(ui, "ENGINE", theme::GREEN);
            ui.label(
                RichText::new(format!(
                    "{} Hz · {} · {} plugin slot(s) · latency {} smp",
                    state.sample_rate,
                    if audio.is_silent() {
                        "SILENT FALLBACK"
                    } else {
                        "AUDIO ONLINE"
                    },
                    studio.slots.iter().flatten().count(),
                    state.plugin_latency
                ))
                .font(theme::mono(8.0))
                .color(theme::TEXT_DIM),
            );
        });
    studio.project_open = open;
}
