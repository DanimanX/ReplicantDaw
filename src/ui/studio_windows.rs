//! Menu bar and utility windows: plugin rack, plugin browser, MIDI and projects.

use egui::{
    Align2, Button, Color32, ComboBox, CornerRadius, Frame, Grid, Pos2, Rect, RichText, ScrollArea,
    Sense, Stroke, Ui, Vec2,
};

use crate::audio::{db_to_gain, AudioRuntime, Command, TRACKS, TRACK_COUNT};
use crate::plugins::PluginFormat;
use crate::state::UiState;
use crate::studio::{sync_track_clip, Studio};
use crate::theme;
use crate::ui::widgets::knob;

/// Donation destination shared with the other Replicant projects.
pub const PAYPAL_DONATE: &str = "https://paypal.me/DanimanX?locale.x=en_US&country.x=CL";

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

    let mut pending_zoom: Option<u32> = None;
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
    if button(&mut child, "SYNTH", studio.synth_open) {
        studio.synth_open = !studio.synth_open;
    }

    let donate = Button::new(
        RichText::new("DONATE")
            .font(theme::display(8.5))
            .color(theme::AMBER),
    )
    .fill(theme::accent_alpha(theme::AMBER, 18))
    .stroke(Stroke::new(1.0, theme::accent_alpha(theme::AMBER, 130)))
    .corner_radius(CornerRadius::same(2));
    if child
        .add(donate)
        .on_hover_text("Buy me a coffee · PayPal")
        .clicked()
    {
        child.ctx().open_url(egui::OpenUrl::new_tab(PAYPAL_DONATE));
    }

    // Zoom control: the whole UI scales, and the window grows with it so the
    // layout keeps its proportions while the text becomes physically larger.
    child.add_space(8.0);
    child.label(
        RichText::new("ZOOM")
            .font(theme::display(7.5))
            .color(theme::TEXT_FAINT),
    );
    let current = (studio.zoom * 100.0).round() as u32;
    ComboBox::from_id_salt("ui_zoom")
        .selected_text(RichText::new(format!("{current}%")).font(theme::mono(8.5)))
        .width(62.0)
        .show_ui(&mut child, |ui| {
            for step in ZOOM_STEPS {
                if ui
                    .selectable_label(step == current, format!("{step}%"))
                    .clicked()
                {
                    pending_zoom = Some(step);
                }
            }
        });
    if button(&mut child, "-", false) {
        pending_zoom = Some(previous_zoom(current));
    }
    if button(&mut child, "+", false) {
        pending_zoom = Some(next_zoom(current));
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

    if let Some(percent) = pending_zoom {
        set_zoom(ui.ctx(), studio, percent);
    }
}

/// Zoom presets available from the menu.
pub const ZOOM_STEPS: [u32; 8] = [75, 90, 100, 115, 125, 150, 175, 200];

fn next_zoom(current: u32) -> u32 {
    ZOOM_STEPS
        .into_iter()
        .find(|step| *step > current)
        .unwrap_or(*ZOOM_STEPS.last().unwrap())
}

fn previous_zoom(current: u32) -> u32 {
    ZOOM_STEPS
        .into_iter()
        .rev()
        .find(|step| *step < current)
        .unwrap_or(ZOOM_STEPS[0])
}

/// Apply a UI scale and resize the window so the logical layout is preserved.
pub fn set_zoom(ctx: &egui::Context, studio: &mut Studio, percent: u32) {
    let zoom = (percent as f32 / 100.0).clamp(0.75, 2.0);
    if (studio.zoom - zoom).abs() < 0.001 {
        return;
    }
    studio.zoom = zoom;
    ctx.set_zoom_factor(zoom);
    let base = Vec2::new(1660.0, 1000.0);
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(base * zoom));
    studio.notify(format!("UI zoom {percent}%"));
    ctx.request_repaint();
}

/// Keyboard zoom (Ctrl +/-/0) used by the app shortcut handler.
pub fn zoom_step(ctx: &egui::Context, studio: &mut Studio, direction: i32) {
    let current = (studio.zoom * 100.0).round() as u32;
    match direction {
        0 => set_zoom(ctx, studio, 100),
        d if d > 0 => set_zoom(ctx, studio, next_zoom(current)),
        _ => set_zoom(ctx, studio, previous_zoom(current)),
    }
}

/// Draw all utility windows.
pub fn draw_windows(
    ctx: &egui::Context,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
) {
    draw_rack(ctx, studio, state, audio);
    draw_browser(ctx, studio, state, audio);
    draw_plugin_editor(ctx, studio, state, audio);
    draw_rename(ctx, studio, state);
    draw_midi(ctx, studio, state, audio);
    draw_project(ctx, studio, state, audio);
    draw_audio_editor(ctx, studio, state, audio);
    crate::ui::samples::draw_editor(ctx, studio, audio);
    crate::ui::sample_clip::draw(ctx, studio, state, audio);
    crate::ui::synth_xt::draw(ctx, studio, state, audio);
    crate::ui::piano_roll::draw(ctx, studio, state, audio);
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

fn draw_rack(
    ctx: &egui::Context,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
) {
    if !studio.rack_open {
        return;
    }
    let mut open = true;
    let mut close = false;
    egui::Window::new("PLUGIN RACK // PER-TRACK INSERTS")
        .open(&mut open)
        .frame(window_frame())
        .default_pos(Pos2::new(300.0, 120.0))
        .default_width(820.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.label(
                RichText::new(
                    "Each track owns one CLAP/VST3 insert. The insert runs before the master effects in the device chain.",
                )
                .font(theme::mono(8.0))
                .color(theme::TEXT_FAINT),
            );
            ui.add_space(6.0);

            ScrollArea::vertical()
                .id_salt("rack_tracks")
                .max_height(340.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (track, _spec) in TRACKS.iter().enumerate().take(TRACK_COUNT) {
                        let info = studio.slots[track].clone();
                        let pending = studio.slot_pending[track];
                        let accent = theme::track_accent(track);
                        Frame::new()
                            .fill(theme::PANEL_SUNK)
                            .stroke(Stroke::new(
                                1.0,
                                if studio.selected_slot == track {
                                    theme::accent_alpha(accent, 150)
                                } else {
                                    theme::BORDER_SOFT
                                },
                            ))
                            .corner_radius(CornerRadius::same(2))
                            .inner_margin(egui::Margin::same(6))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("{:02}", track + 1))
                                            .font(theme::display(10.0))
                                            .color(accent),
                                    );
                                    ui.label(
                                        RichText::new(state.track_label(track))
                                            .font(theme::display(8.2))
                                            .color(theme::TEXT),
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
                                                    .color(theme::accent_alpha(
                                                        theme::MAGENTA,
                                                        220,
                                                    )),
                                            );
                                            ui.label(
                                                RichText::new(format!(
                                                    "v{} · latency {} smp",
                                                    info.version, info.latency
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
                                                    .add(Button::new(RichText::new("X").font(
                                                        theme::display(8.0),
                                                    )))
                                                    .clicked()
                                                {
                                                    studio.unload_plugin(track, audio);
                                                }
                                                let label =
                                                    if info.as_ref().is_some_and(|i| i.bypass) {
                                                        "BYPASSED"
                                                    } else {
                                                        "BYPASS"
                                                    };
                                                if ui
                                                    .add(Button::new(
                                                        RichText::new(label)
                                                            .font(theme::display(8.0)),
                                                    ))
                                                    .clicked()
                                                {
                                                    let bypass = !info
                                                        .as_ref()
                                                        .is_some_and(|i| i.bypass);
                                                    studio.set_bypass(track, bypass);
                                                }
                                            }
                                            if ui
                                                .add(Button::new(
                                                    RichText::new("LOAD").font(theme::display(8.0)),
                                                ))
                                                .clicked()
                                            {
                                                studio.browser_target = track;
                                                studio.browser_open = true;
                                                if studio.scan_results.is_empty()
                                                    && !studio.scan_running
                                                {
                                                    studio.start_scan();
                                                }
                                            }
                                            if info.is_some()
                                                && ui
                                                    .add(Button::new(
                                                        RichText::new("EDIT").font(
                                                            theme::display(8.0),
                                                        ),
                                                    ))
                                                    .clicked()
                                            {
                                                studio.open_plugin_editor(track, false);
                                            }
                                            if info.as_ref().is_some_and(|i| i.has_editor)
                                                && ui
                                                    .add(Button::new(
                                                        RichText::new("GUI").font(
                                                            theme::display(8.0),
                                                        )
                                                        .color(theme::AMBER),
                                                    ))
                                                    .on_hover_text("Open the plugin's own window")
                                                    .clicked()
                                            {
                                                if info.as_ref().is_some_and(|i| i.gui_open) {
                                                    studio.close_plugin_gui(track);
                                                } else {
                                                    studio.open_plugin_gui(track);
                                                }
                                            }
                                            ui.label(
                                                RichText::new(format!(
                                                    "TRACK {:02}",
                                                    track + 1
                                                ))
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
                                }
                            });
                        ui.add_space(4.0);
                    }
                });

            let selected = studio.selected_slot.min(TRACK_COUNT - 1);
            if let Some(info) = studio.slots[selected].clone() {
                if !info.params.is_empty() {
                    ui.add_space(4.0);
                    section(
                        ui,
                        &format!(
                            "PARAMETERS · TRACK {:02} · {}",
                            selected + 1,
                            info.name
                        ),
                        theme::CYAN,
                    );
                    ScrollArea::vertical()
                        .id_salt("rack_params")
                        .max_height(230.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let mut changes = Vec::new();
                            Grid::new(("params", selected))
                                .num_columns(8)
                                .spacing(Vec2::new(6.0, 4.0))
                                .show(ui, |ui| {
                                    for (index, param) in
                                        info.params.iter().take(64).enumerate()
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
                                            ui.scope_id().with(("plugin_param", selected, index)),
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
                                if let Some(info) = &studio.slots[selected] {
                                    if let Some(param) =
                                        info.params.iter().find(|p| p.id == id).cloned()
                                    {
                                        studio.set_param(selected, &param, value);
                                    }
                                }
                            }
                        });
                }
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

fn draw_browser(
    ctx: &egui::Context,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
) {
    let mut open = studio.browser_open;
    // Selecting something must close the picker; `open` is captured before the
    // click is handled, so a flag is needed to carry the decision to the end.
    let mut close = false;
    let title = if studio.instrument_browser {
        "INSTRUMENT BROWSER // ADD PLUGIN / INSTRUMENT"
    } else {
        "PLUGIN BROWSER"
    };
    egui::Window::new(title)
        .open(&mut open)
        .frame(window_frame())
        .default_pos(Pos2::new(260.0, 150.0))
        .default_width(620.0)
        .default_height(420.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "TARGET TRACK {:02} · {}",
                        studio.browser_target + 1,
                        state.track_label(studio.browser_target.min(TRACK_COUNT - 1))
                    ))
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
            let mut choose_synth = false;
            if studio.instrument_browser {
                // The internal engine is always available, above the plugins.
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("INT ")
                            .font(theme::mono(8.0))
                            .color(theme::GREEN),
                    );
                    ui.label(
                        RichText::new("NEURAL GEN XT · internal hybrid synth")
                            .font(theme::display(8.5))
                            .color(theme::TEXT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(Button::new(
                                RichText::new("USE").font(theme::display(8.0)),
                            ))
                            .clicked()
                        {
                            choose_synth = true;
                        }
                    });
                });
                ui.separator();
            }
            ScrollArea::vertical()
                .id_salt("plugin_browser_list")
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
                        studio.load_plugin(slot, &descriptor, state);
                        close = true;
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
            if choose_synth {
                let track = studio.browser_target;
                studio.select_synth(state, track, audio);
                close = true;
                studio.notify(format!(
                    "Track {:02} now plays the internal synth",
                    track + 1
                ));
            }
        });
    if close {
        open = false;
    }
    studio.browser_open = open;
    if !open {
        studio.instrument_browser = false;
    }
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
                        let scene = state.launched[track as usize].unwrap_or(0);
                        state.recording = true;
                        state.record_track = track;
                        state.clear_recorded(track as usize, scene as usize);
                        let _ = audio.command_tx.push(Command::RecordStart { track, scene });
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
                    .selected_text(format!(
                        "{:02} {}",
                        state.selected_track + 1,
                        state.track_label(state.selected_track)
                    ))
                    .show_ui(ui, |ui| {
                        for track in 0..crate::audio::TRACK_COUNT {
                            if ui
                                .selectable_label(
                                    state.selected_track == track,
                                    format!("{:02} {}", track + 1, state.track_label(track)),
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
                        RichText::new("CLEAR SLOT").font(theme::display(8.0)),
                    ))
                    .clicked()
                {
                    let track = state.selected_track;
                    let scene = state.launched[track].unwrap_or(0);
                    state.clear_recorded(track, scene as usize);
                    let _ = audio.command_tx.push(Command::ClearRecorded {
                        track: track as u8,
                        scene,
                    });
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

fn draw_audio_editor(
    ctx: &egui::Context,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
) {
    if !studio.audio_editor_open {
        return;
    }
    let mut open = true;
    let track = studio.audio_editor_track.min(TRACK_COUNT - 1);
    let accent = theme::track_accent(track);
    let mut chosen_track = track;
    egui::Window::new(format!(
        "AUDIO CLIP // TRACK {:02} {}",
        track + 1,
        state.track_label(track)
    ))
    .open(&mut open)
    .frame(window_frame())
    .default_pos(Pos2::new(520.0, 160.0))
    .default_size(Vec2::new(760.0, 520.0))
    .resizable(true)
    .show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("TRACK")
                    .font(theme::mono(8.0))
                    .color(theme::TEXT_FAINT),
            );
            egui::ComboBox::from_id_salt("audio_editor_track")
                .selected_text(format!("{:02} {}", track + 1, state.track_label(track)))
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
                            chosen_track = index;
                        }
                    }
                });
            let scene = state.launched[track].unwrap_or(0) as usize;
            ui.label(
                RichText::new(format!(
                    "SLOT {:02} · {} · 16 BARS",
                    scene + 1,
                    UiState::scene_name(scene)
                ))
                .font(theme::mono(8.0))
                .color(theme::TEXT_DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(Button::new(
                        RichText::new("CLOSE").font(theme::display(8.5)),
                    ))
                    .clicked()
                {
                    studio.audio_editor_open = false;
                }
                if ui
                    .add(
                        Button::new(
                            RichText::new("EDIT MIDI").font(theme::display(8.0)),
                        )
                        .fill(theme::PANEL_SUNK),
                    )
                    .clicked()
                {
                    studio.open_piano_roll(track, state.launched[track].unwrap_or(0));
                }
            });
        });
        if chosen_track != track {
            studio.audio_editor_track = chosen_track;
            state.selected_track = chosen_track;
            return;
        }

        ui.add_space(6.0);
        let scene = state.launched[track].unwrap_or(0) as usize;
        let (wave_rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 168.0),
            Sense::hover(),
        );
        let painter = ui.painter();
        painter.rect_filled(wave_rect, CornerRadius::same(2), theme::BG_DEEP);
        painter.rect_stroke(
            wave_rect,
            CornerRadius::same(2),
            Stroke::new(1.0, theme::BORDER_SOFT),
            egui::StrokeKind::Inside,
        );

        // Bar grid.
        for bar in 0..=16 {
            let x = wave_rect.left() + wave_rect.width() * bar as f32 / 16.0;
            painter.line_segment(
                [
                    Pos2::new(x, wave_rect.top()),
                    Pos2::new(x, wave_rect.bottom()),
                ],
                Stroke::new(
                    if bar % 4 == 0 { 1.0 } else { 0.5 },
                    if bar % 4 == 0 {
                        theme::accent_alpha(theme::BORDER, 180)
                    } else {
                        theme::BORDER_SOFT
                    },
                ),
            );
        }

        let points = state.clip_waveform(track, scene, 240);
        let gain = db_to_gain(state.clip_gain_db[track]);
        let fade_in = state.clip_fade_in[track];
        let fade_out = state.clip_fade_out[track];
        let mid = wave_rect.center().y;
        let amp = wave_rect.height() * 0.40;
        let path: Vec<Pos2> = points
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let progress = index as f32 / points.len().max(2) as f32;
                let fade = envelope(progress, fade_in, fade_out);
                let x = wave_rect.left()
                    + wave_rect.width() * index as f32 / points.len().max(2) as f32;
                Pos2::new(x, mid - value * amp * gain * fade)
            })
            .collect();
        painter.add(egui::Shape::line(
            path,
            Stroke::new(1.1, theme::accent_alpha(accent, 225)),
        ));

        // Envelope curve.
        let envelope_points: Vec<Pos2> = (0..=160)
            .map(|index| {
                let progress = index as f32 / 160.0;
                let fade = envelope(progress, fade_in, fade_out) * gain;
                Pos2::new(
                    wave_rect.left() + progress * wave_rect.width(),
                    wave_rect.top() + 4.0 + (1.0 - fade.min(1.0)) * (wave_rect.height() - 8.0),
                )
            })
            .collect();
        painter.add(egui::Shape::line(
            envelope_points,
            Stroke::new(1.0, theme::accent_alpha(theme::AMBER, 150)),
        ));

        let playhead = (state.beats() as f32 / 64.0).clamp(0.0, 1.0);
        let px = wave_rect.left() + playhead * wave_rect.width();
        painter.line_segment(
            [Pos2::new(px, wave_rect.top()), Pos2::new(px, wave_rect.bottom())],
            Stroke::new(1.2, theme::CYAN),
        );
        painter.text(
            Pos2::new(wave_rect.left() + 5.0, wave_rect.top() + 5.0),
            Align2::LEFT_TOP,
            "RENDERED TRACK AUDIO",
            theme::display(6.6),
            theme::accent_alpha(accent, 200),
        );
        painter.text(
            Pos2::new(wave_rect.right() - 5.0, wave_rect.top() + 5.0),
            Align2::RIGHT_TOP,
            format!("CLIP {:+.1} dB", state.clip_gain_db[track]),
            theme::mono(6.4),
            theme::AMBER,
        );

        ui.add_space(8.0);
        ui.label(
            RichText::new("CLIP ENVELOPE")
                .font(theme::display(8.6))
                .color(accent),
        );
        let mut changed = false;
        let knob_size = Vec2::new(150.0, 84.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(knob_size, Sense::hover());
            let text = format!("{:+.1} dB", state.clip_gain_db[track]);
            changed |= knob(
                ui,
                rect,
                ui.scope_id().with(("audio_clip_gain", track)),
                &mut state.clip_gain_db[track],
                -60.0..=12.0,
                0.0,
                "CLIP GAIN",
                &text,
                accent,
            );

            let (rect, _) = ui.allocate_exact_size(knob_size, Sense::hover());
            let text = format!("{:.0}%", state.clip_fade_in[track] * 100.0);
            changed |= knob(
                ui,
                rect,
                ui.scope_id().with(("audio_fade_in", track)),
                &mut state.clip_fade_in[track],
                0.0..=1.0,
                0.0,
                "FADE IN",
                &text,
                theme::GREEN,
            );

            let (rect, _) = ui.allocate_exact_size(knob_size, Sense::hover());
            let text = format!("{:.0}%", state.clip_fade_out[track] * 100.0);
            changed |= knob(
                ui,
                rect,
                ui.scope_id().with(("audio_fade_out", track)),
                &mut state.clip_fade_out[track],
                0.0..=1.0,
                0.0,
                "FADE OUT",
                &text,
                theme::AMBER,
            );

            ui.vertical(|ui| {
                let muted = state.mute[track];
                let label = if muted { "MUTED" } else { "MUTE" };
                if ui
                    .add(
                        Button::new(RichText::new(label).font(theme::display(8.0)))
                            .fill(if muted {
                                theme::accent_alpha(theme::AMBER, 30)
                            } else {
                                theme::PANEL_SUNK
                            }),
                    )
                    .clicked()
                {
                    state.mute[track] = !muted;
                    let _ = audio.command_tx.push(Command::SetTrackMute {
                        track: track as u8,
                        mute: state.mute[track],
                    });
                }
                let soloed = state.solo[track];
                let label = if soloed { "SOLO ON" } else { "SOLO" };
                if ui
                    .add(
                        Button::new(RichText::new(label).font(theme::display(8.0)))
                            .fill(if soloed {
                                theme::accent_alpha(theme::GREEN, 30)
                            } else {
                                theme::PANEL_SUNK
                            }),
                    )
                    .clicked()
                {
                    state.solo[track] = !soloed;
                    let _ = audio.command_tx.push(Command::SetTrackSolo {
                        track: track as u8,
                        solo: state.solo[track],
                    });
                }
            });
        });
        if changed {
            sync_track_clip(state, audio, track);
        }

        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "The clip envelope is applied before the master bus: level, fade-in and fade-out are saved with the project. MIDI notes are edited in the piano roll.",
            )
            .font(theme::mono(7.5))
            .color(theme::TEXT_FAINT),
        );
    });
    studio.audio_editor_open = open;
}

/// Large plugin editor, Ableton-style: the parameter surface can take over
/// the whole window with `FULLSCREEN` (or F11), and Esc restores it.
fn draw_plugin_editor(
    ctx: &egui::Context,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
) {
    if !studio.plugin_editor_open {
        return;
    }
    let track = studio.plugin_editor_track.min(TRACK_COUNT - 1);
    let accent = theme::accent_alpha(theme::CYAN, 255);
    let info = studio.slots[track].clone();
    let pending = studio.slot_pending[track];
    let mut open = true;

    let screen = ctx.content_rect();
    let fullscreen = studio.plugin_editor_fullscreen;
    let mut window = egui::Window::new(match &info {
        Some(info) => format!(
            "PLUGIN EDITOR // TRACK {:02} {} // {}",
            track + 1,
            state.track_label(track),
            info.name
        ),
        None => format!(
            "PLUGIN EDITOR // TRACK {:02} {}",
            track + 1,
            state.track_label(track)
        ),
    })
    .open(&mut open)
    .frame(window_frame())
    .resizable(!fullscreen)
    .collapsible(!fullscreen);

    if fullscreen {
        window = window
            .fixed_pos(Pos2::new(screen.left() + 6.0, screen.top() + 6.0))
            .fixed_size(Vec2::new(
                (screen.width() - 12.0).max(320.0),
                (screen.height() - 12.0).max(240.0),
            ));
    } else {
        window = window
            .default_pos(Pos2::new(220.0, 100.0))
            .default_size(Vec2::new(820.0, 520.0));
    }

    window.show(ctx, |ui| {
        ui.horizontal(|ui| {
            if ui
                .add(
                    Button::new(
                        RichText::new(if fullscreen { "RESTORE" } else { "FULLSCREEN" })
                        .font(theme::display(8.5))
                        .color(theme::CYAN),
                    )
                    .fill(theme::PANEL_SUNK),
                )
                .clicked()
            {
                studio.plugin_editor_fullscreen = !fullscreen;
            }
            ui.label(
                RichText::new(format!(
                    "TRACK {:02} {}",
                    track + 1,
                    state.track_label(track)
                ))
                .font(theme::display(8.5))
                .color(theme::TEXT_DIM),
            );
            if let Some(info) = &info {
                ui.label(
                    RichText::new(format!(
                        "{} · {} · {} params · latency {} smp",
                        info.format.label(),
                        info.vendor,
                        info.params.len(),
                        info.latency
                    ))
                    .font(theme::mono(7.5))
                    .color(theme::TEXT_FAINT),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(Button::new(
                        RichText::new("CLOSE").font(theme::display(8.5)),
                    ))
                    .clicked()
                {
                    studio.plugin_editor_open = false;
                    studio.plugin_editor_fullscreen = false;
                }
                if let Some(info) = &info {
                    if ui
                        .add(Button::new(
                            RichText::new("X").font(theme::display(8.5)),
                        ))
                        .clicked()
                    {
                        studio.unload_plugin(track, audio);
                        studio.plugin_editor_open = false;
                    }
                    let label = if info.bypass { "BYPASSED" } else { "BYPASS" };
                    if ui
                        .add(Button::new(
                            RichText::new(label).font(theme::display(8.0)),
                        ))
                        .clicked()
                    {
                        studio.set_bypass(track, !info.bypass);
                    }
                }
                if ui
                    .add(Button::new(
                        RichText::new("LOAD…").font(theme::display(8.0)),
                    ))
                    .clicked()
                {
                    studio.browser_target = track;
                    studio.browser_open = true;
                    if studio.scan_results.is_empty() && !studio.scan_running {
                        studio.start_scan();
                    }
                }
            });
        });

        ui.add_space(6.0);
        let Some(info) = info.clone() else {
            ui.add_space(20.0);
            let message = if pending {
                "LOADING PLUGIN…".to_string()
            } else {
                format!(
                    "No plugin on track {:02}. Press LOAD… and pick a CLAP/VST3 instrument or effect.",
                    track + 1
                )
            };
            ui.label(
                RichText::new(message)
                    .font(theme::mono(9.0))
                    .color(theme::TEXT_DIM),
            );
            return;
        };

        if let Some(error) = &info.error {
            ui.label(RichText::new(error).font(theme::mono(8.0)).color(theme::RED));
        }

        ui.add(
            egui::TextEdit::singleline(&mut studio.plugin_editor_filter)
                .hint_text("filter parameters…")
                .desired_width(260.0),
        );
        ui.add_space(4.0);

        let filter = studio.plugin_editor_filter.to_lowercase();
        let params: Vec<_> = info
            .params
            .iter()
            .filter(|param| filter.is_empty() || param.name.to_lowercase().contains(&filter))
            .cloned()
            .collect();
        if params.is_empty() {
            ui.label(
                RichText::new("This plugin exposes no parameters, or none match the filter.")
                    .font(theme::mono(8.0))
                    .color(theme::TEXT_FAINT),
            );
            return;
        }

        let knob_size = Vec2::new(
            if fullscreen { 112.0 } else { 100.0 },
            if fullscreen { 88.0 } else { 74.0 },
        );
        let spacing = 8.0;
        let columns = (((ui.available_width() + spacing) / (knob_size.x + spacing)).floor()
            as usize)
            .clamp(1, 10);
        let mut changes = Vec::new();
        ScrollArea::vertical()
            .id_salt("plugin_editor_params")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Grid::new(("plugin_editor_grid", track))
                    .num_columns(columns)
                    .spacing(Vec2::new(8.0, 6.0))
                    .show(ui, |ui| {
                        for (index, param) in params.iter().enumerate() {
                            let mut value = param.value;
                            let (rect, _) = ui.allocate_exact_size(knob_size, Sense::hover());
                            let text = format!("{:.0}%", value * 100.0);
                            if knob(
                                ui,
                                rect,
                                ui.scope_id().with(("plugin_editor_param", track, index)),
                                &mut value,
                                0.0..=1.0,
                                param.value,
                                &param.name.chars().take(14).collect::<String>(),
                                &text,
                                if info.bypass { theme::TEXT_FAINT } else { accent },
                            ) {
                                changes.push((param.id, value));
                            }
                            if (index + 1) % columns == 0 {
                                ui.end_row();
                            }
                        }
                    });
            });
        for (id, value) in changes {
            if let Some(info) = &studio.slots[track] {
                if let Some(param) = info.params.iter().find(|p| p.id == id).cloned() {
                    studio.set_param(track, &param, value);
                }
            }
        }
    });

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && studio.plugin_editor_fullscreen {
        studio.plugin_editor_fullscreen = false;
        ctx.request_repaint();
    }
    if !open {
        studio.plugin_editor_open = false;
        studio.plugin_editor_fullscreen = false;
    }
}

/// Inline track rename prompt used by the session, arrangement and mixer.
fn draw_rename(ctx: &egui::Context, studio: &mut Studio, state: &mut UiState) {
    let Some(track) = studio.rename_track else {
        return;
    };
    let track = track.min(TRACK_COUNT - 1);
    let mut open = true;
    let mut commit = false;
    egui::Window::new(format!("RENAME TRACK {:02}", track + 1))
        .open(&mut open)
        .frame(window_frame())
        .resizable(false)
        .collapsible(false)
        .default_pos(Pos2::new(480.0, 320.0))
        .default_width(320.0)
        .show(ctx, |ui| {
            ui.label(
                RichText::new(
                    "Track names appear in the session, arrangement, mixer and device chain.",
                )
                .font(theme::mono(7.5))
                .color(theme::TEXT_FAINT),
            );
            ui.add_space(4.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut studio.rename_buffer)
                    .hint_text("track name")
                    .desired_width(290.0)
                    .char_limit(24),
            );
            if studio.rename_focus {
                response.request_focus();
                studio.rename_focus = false;
            }
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                commit = true;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                studio.rename_track = None;
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add(Button::new(RichText::new("SAVE").font(theme::display(8.5))))
                    .clicked()
                {
                    commit = true;
                }
                if ui
                    .add(Button::new(
                        RichText::new("CANCEL").font(theme::display(8.5)),
                    ))
                    .clicked()
                {
                    studio.rename_track = None;
                }
            });
        });
    if commit {
        let name = studio.rename_buffer.clone();
        state.rename_track(track, &name);
        studio.rename_track = None;
        studio.notify(format!(
            "Track {:02} renamed to {}",
            track + 1,
            state.track_label(track)
        ));
    } else if !open {
        studio.rename_track = None;
    }
}

fn envelope(progress: f32, fade_in: f32, fade_out: f32) -> f32 {
    let fade_in = if fade_in > 0.0 {
        (progress / fade_in).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let fade_out = if fade_out > 0.0 {
        ((1.0 - progress) / fade_out).clamp(0.0, 1.0)
    } else {
        1.0
    };
    fade_in.min(fade_out)
}
