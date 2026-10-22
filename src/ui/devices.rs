//! Bottom device chain: three parameter devices plus the master bus.

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Stroke, Ui, Vec2};

use crate::audio::{gain_to_db, AudioRuntime, Command, FxParam, MAX_VOICES};
use crate::state::UiState;
use crate::theme;
use crate::ui::widgets::{knob, meter};

const BOX_GAP: f32 = 8.0;

pub fn draw(ui: &mut Ui, rect: Rect, state: &mut UiState, audio: &mut AudioRuntime) {
    let content = theme::paint_panel(
        ui.painter(),
        rect,
        "DEVICE CHAIN // MODULAR SIGNAL PATH",
        "SYNTH → TONE → FILTER → DRIVE → MASTER",
        theme::MAGENTA,
    );
    if content.height() < 60.0 {
        return;
    }

    let ratios = [1.25_f32, 1.20, 1.15, 0.92];
    let total_ratio: f32 = ratios.iter().sum();
    let usable = content.width() - BOX_GAP * 3.0;
    let mut x = content.left();

    for (index, ratio) in ratios.iter().enumerate() {
        let width = usable * ratio / total_ratio;
        let box_rect = Rect::from_min_size(
            Pos2::new(x, content.top()),
            Vec2::new(width, content.height()),
        );
        match index {
            0 => draw_neural_gen(ui, box_rect, state, audio),
            1 => draw_resonator(ui, box_rect, state, audio),
            2 => draw_drive(ui, box_rect, state, audio),
            _ => draw_master(ui, box_rect, state, audio),
        }
        x += width + BOX_GAP;
    }

    // Signal-flow chevrons on the gaps.
    x = content.left();
    for (index, ratio) in ratios.iter().enumerate() {
        let width = usable * ratio / total_ratio;
        if index < 3 {
            let cx = x + width + BOX_GAP * 0.5;
            let cy = content.center().y;
            let color = theme::accent_alpha(theme::CYAN, 70);
            ui.painter().line_segment(
                [Pos2::new(cx - 3.0, cy - 3.0), Pos2::new(cx + 1.0, cy)],
                Stroke::new(1.0, color),
            );
            ui.painter().line_segment(
                [Pos2::new(cx - 3.0, cy + 3.0), Pos2::new(cx + 1.0, cy)],
                Stroke::new(1.0, color),
            );
        }
        x += width + BOX_GAP;
    }
}

fn device_shell(ui: &Ui, rect: Rect, title: &str, subtitle: &str, accent: Color32) -> Rect {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), theme::PANEL_RAISED);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );
    let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 20.0));
    painter.rect_filled(header, CornerRadius::same(2), theme::HEADER);
    painter.line_segment(
        [header.left_bottom(), header.right_bottom()],
        Stroke::new(1.0, theme::BORDER),
    );
    painter.circle_filled(
        Pos2::new(header.left() + 9.0, header.center().y),
        2.2,
        accent,
    );
    painter.text(
        Pos2::new(header.left() + 16.0, header.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::display(8.2),
        accent,
    );
    painter.text(
        Pos2::new(header.right() - 7.0, header.center().y),
        Align2::RIGHT_CENTER,
        subtitle,
        theme::mono(6.2),
        theme::TEXT_FAINT,
    );
    painter.text(
        Pos2::new(rect.right() - 7.0, rect.bottom() - 4.0),
        Align2::RIGHT_BOTTOM,
        "REPLICANT DSP",
        theme::mono(5.5),
        theme::TEXT_FAINT,
    );
    Rect::from_min_max(
        Pos2::new(rect.left() + 7.0, header.bottom() + 4.0),
        Pos2::new(rect.right() - 7.0, rect.bottom() - 9.0),
    )
}

fn draw_neural_gen(ui: &mut Ui, rect: Rect, state: &mut UiState, audio: &mut AudioRuntime) {
    let content = device_shell(
        ui,
        rect,
        "NEURAL GEN V5",
        "GEN-AI // SYNTH CORE",
        theme::CYAN,
    );
    let knob_h = (content.height() * 0.70).clamp(46.0, 96.0);
    let knob_w = (content.width() - 8.0) * 0.5;
    let mut changed_gain = false;
    let mut changed_tone = false;

    let gain_text = format!("{:.2}", state.fx.gain);
    changed_gain |= knob(
        ui,
        Rect::from_min_size(content.min, Vec2::new(knob_w, knob_h)),
        ui.scope_id().with("fx_gain"),
        &mut state.fx.gain,
        0.0..=1.4,
        0.9,
        "GAIN",
        &gain_text,
        theme::CYAN,
    );

    // Logarithmic tone control.
    let tone_min = 300.0_f32;
    let tone_max = 18_000.0_f32;
    let mut tone_norm =
        ((state.fx.tone.ln() - tone_min.ln()) / (tone_max / tone_min).ln()).clamp(0.0, 1.0);
    let tone_text = format_hz(state.fx.tone);
    let tone_widget_changed = knob(
        ui,
        Rect::from_min_size(
            Pos2::new(content.left() + knob_w + 8.0, content.top()),
            Vec2::new(knob_w, knob_h),
        ),
        ui.scope_id().with("fx_tone"),
        &mut tone_norm,
        0.0..=1.0,
        0.97,
        "CUT",
        &tone_text,
        theme::CYAN,
    );
    if tone_widget_changed {
        state.fx.tone = tone_min * (tone_max / tone_min).powf(tone_norm);
        changed_tone = true;
    }
    let _ = changed_tone;

    if changed_gain {
        send_fx(audio, FxParam::Gain, state.fx.gain);
    }
    if tone_widget_changed {
        send_fx(audio, FxParam::Tone, state.fx.tone);
    }

    // Mini waveform of the pre-master signal (visual only, phase animated).
    let wave_rect = Rect::from_min_max(
        Pos2::new(content.left(), content.top() + knob_h + 2.0),
        Pos2::new(content.right(), content.bottom() - 2.0),
    );
    if wave_rect.height() > 10.0 {
        ui.painter()
            .rect_filled(wave_rect, CornerRadius::same(1), theme::BG_DEEP);
        let time = ui.input(|i| i.time) as f32;
        let points: Vec<Pos2> = (0..=64)
            .map(|i| {
                let t = i as f32 / 64.0;
                let y = (t * std::f32::consts::TAU * 2.0 + time * 3.0).sin()
                    * (0.35 + 0.35 * state.fx.drive)
                    + (t * std::f32::consts::TAU * 5.0 + time * 1.7).sin() * 0.12;
                Pos2::new(
                    wave_rect.left() + t * wave_rect.width(),
                    wave_rect.center().y + y * wave_rect.height() * 0.42,
                )
            })
            .collect();
        ui.painter().add(egui::Shape::line(
            points,
            Stroke::new(1.0, theme::accent_alpha(theme::CYAN, 170)),
        ));
        ui.painter().text(
            Pos2::new(wave_rect.left() + 4.0, wave_rect.top() + 2.0),
            Align2::LEFT_TOP,
            format!("{:.0} ms", 300.0),
            theme::mono(5.5),
            theme::TEXT_FAINT,
        );
    }
}

fn draw_resonator(ui: &mut Ui, rect: Rect, state: &mut UiState, audio: &mut AudioRuntime) {
    let content = device_shell(
        ui,
        rect,
        "QUANTUM RESONATOR",
        "SVF // 12 dB/OCT",
        theme::GREEN,
    );
    let knob_h = (content.height() * 0.70).clamp(46.0, 96.0);
    let knob_w = (content.width() - 8.0) * 0.5;

    let cut_min = 80.0_f32;
    let cut_max = 12_000.0_f32;
    let mut cut_norm =
        ((state.fx.cutoff.ln() - cut_min.ln()) / (cut_max / cut_min).ln()).clamp(0.0, 1.0);
    let cutoff_text = format_hz(state.fx.cutoff);
    let changed_cut = knob(
        ui,
        Rect::from_min_size(content.min, Vec2::new(knob_w, knob_h)),
        ui.scope_id().with("fx_cutoff"),
        &mut cut_norm,
        0.0..=1.0,
        0.83,
        "CUTOFF",
        &cutoff_text,
        theme::GREEN,
    );
    if changed_cut {
        state.fx.cutoff = cut_min * (cut_max / cut_min).powf(cut_norm);
        send_fx(audio, FxParam::Cutoff, state.fx.cutoff);
    }

    let resonance_text = format!("{:.0}%", state.fx.resonance * 100.0);
    let changed_res = knob(
        ui,
        Rect::from_min_size(
            Pos2::new(content.left() + knob_w + 8.0, content.top()),
            Vec2::new(knob_w, knob_h),
        ),
        ui.scope_id().with("fx_resonance"),
        &mut state.fx.resonance,
        0.0..=1.0,
        0.18,
        "RESONANCE",
        &resonance_text,
        theme::GREEN,
    );
    if changed_res {
        send_fx(audio, FxParam::Resonance, state.fx.resonance);
    }

    // Node lattice visualisation.
    let graph = Rect::from_min_max(
        Pos2::new(content.left(), content.top() + knob_h + 2.0),
        Pos2::new(content.right(), content.bottom() - 2.0),
    );
    if graph.height() > 10.0 {
        ui.painter()
            .rect_filled(graph, CornerRadius::same(1), theme::BG_DEEP);
        let time = ui.input(|i| i.time) as f32;
        let nodes: Vec<Pos2> = (0..9)
            .map(|i| {
                let fx = (i as f32 * 0.618).fract();
                let fy = (i as f32 * 0.381).fract();
                Pos2::new(
                    graph.left() + 5.0 + fx * (graph.width() - 10.0),
                    graph.top() + 4.0 + fy * (graph.height() - 8.0),
                )
            })
            .collect();
        for (i, node) in nodes.iter().enumerate() {
            for (j, other) in nodes.iter().enumerate() {
                if j > i && (i + j) % 3 == 0 {
                    let pulse = ((time * 1.5 + i as f32 * 0.4).sin() * 0.5 + 0.5) * 0.5 + 0.15;
                    ui.painter().line_segment(
                        [*node, *other],
                        Stroke::new(
                            1.0,
                            Color32::from_rgba_unmultiplied(
                                theme::GREEN.r(),
                                theme::GREEN.g(),
                                theme::GREEN.b(),
                                (pulse * 90.0) as u8,
                            ),
                        ),
                    );
                }
            }
        }
        for (i, node) in nodes.iter().enumerate() {
            let pulse = ((time * 2.0 + i as f32).sin() * 0.5 + 0.5) * 0.6 + 0.4;
            ui.painter().circle_filled(
                *node,
                1.6 + pulse * state.fx.resonance * 1.8,
                theme::accent_alpha(theme::GREEN, 200),
            );
        }
    }
}

fn draw_drive(ui: &mut Ui, rect: Rect, state: &mut UiState, audio: &mut AudioRuntime) {
    let content = device_shell(
        ui,
        rect,
        "SYNAPSE DRIVE",
        "SATURATION // WIDTH",
        theme::AMBER,
    );
    let knob_h = (content.height() * 0.70).clamp(46.0, 96.0);
    let knob_w = (content.width() - 8.0) * 0.5;

    let drive_text = format!("{:.0}%", state.fx.drive * 100.0);
    let changed_drive = knob(
        ui,
        Rect::from_min_size(content.min, Vec2::new(knob_w, knob_h)),
        ui.scope_id().with("fx_drive"),
        &mut state.fx.drive,
        0.0..=1.0,
        0.12,
        "DRIVE",
        &drive_text,
        theme::AMBER,
    );
    if changed_drive {
        send_fx(audio, FxParam::Drive, state.fx.drive);
    }
    let width_text = format!("{:.0}%", state.fx.width * 100.0);
    let changed_width = knob(
        ui,
        Rect::from_min_size(
            Pos2::new(content.left() + knob_w + 8.0, content.top()),
            Vec2::new(knob_w, knob_h),
        ),
        ui.scope_id().with("fx_width"),
        &mut state.fx.width,
        0.0..=1.5,
        1.0,
        "WIDTH",
        &width_text,
        theme::AMBER,
    );
    if changed_width {
        send_fx(audio, FxParam::Width, state.fx.width);
    }

    // Layered wave field.
    let graph = Rect::from_min_max(
        Pos2::new(content.left(), content.top() + knob_h + 2.0),
        Pos2::new(content.right(), content.bottom() - 2.0),
    );
    if graph.height() > 10.0 {
        ui.painter()
            .rect_filled(graph, CornerRadius::same(1), theme::BG_DEEP);
        let time = ui.input(|i| i.time) as f32;
        for layer in 0..3 {
            let phase = layer as f32 * 0.9;
            let color = if layer == 1 {
                theme::MAGENTA
            } else {
                theme::AMBER
            };
            let points: Vec<Pos2> = (0..=60)
                .map(|i| {
                    let t = i as f32 / 60.0;
                    let amp = (0.20 + 0.10 * layer as f32) * (0.5 + state.fx.drive);
                    let y = (t * std::f32::consts::TAU * 1.6
                        + time * (1.2 + layer as f32 * 0.2)
                        + phase)
                        .sin()
                        * amp
                        + (t * std::f32::consts::TAU * 3.1 + phase).sin() * 0.06;
                    Pos2::new(
                        graph.left() + t * graph.width(),
                        graph.center().y
                            + y * graph.height() * 0.9
                            + (layer as f32 - 1.0) * graph.height() * 0.11,
                    )
                })
                .collect();
            ui.painter().add(egui::Shape::line(
                points,
                Stroke::new(1.0, theme::accent_alpha(color, 140)),
            ));
        }
    }
}

fn draw_master(ui: &mut Ui, rect: Rect, state: &mut UiState, audio: &AudioRuntime) {
    let content = device_shell(ui, rect, "MASTER BUS", "STEREO // LIMITER", theme::MAGENTA);
    let meter_w = 8.0;
    let meter_h = (content.height() - 28.0).clamp(20.0, 92.0);
    let top = content.top() + 4.0;
    for channel in 0..2 {
        let meter_rect = Rect::from_min_size(
            Pos2::new(content.left() + channel as f32 * (meter_w + 3.0), top),
            Vec2::new(meter_w, meter_h),
        );
        meter(ui, meter_rect, state.master_peaks[channel]);
        ui.painter().text(
            Pos2::new(meter_rect.center().x, meter_rect.bottom() + 2.0),
            Align2::CENTER_TOP,
            if channel == 0 { "L" } else { "R" },
            theme::mono(6.0),
            theme::TEXT_FAINT,
        );
    }

    let text_x = content.left() + 26.0;
    let master_db = state.master_db();
    let color = if master_db > -1.0 {
        theme::RED
    } else if master_db > -8.0 {
        theme::AMBER
    } else {
        theme::GREEN
    };
    ui.painter().text(
        Pos2::new(text_x, top + 2.0),
        Align2::LEFT_TOP,
        "OUT",
        theme::mono(6.0),
        theme::TEXT_FAINT,
    );
    ui.painter().text(
        Pos2::new(text_x, top + 12.0),
        Align2::LEFT_TOP,
        if master_db > -60.0 {
            format!("{master_db:+.1}")
        } else {
            "-inf".to_string()
        },
        theme::mono(17.0),
        color,
    );
    ui.painter().text(
        Pos2::new(text_x, top + 32.0),
        Align2::LEFT_TOP,
        "dBFS",
        theme::mono(6.0),
        theme::TEXT_FAINT,
    );
    ui.painter().text(
        Pos2::new(content.right(), top + 2.0),
        Align2::RIGHT_TOP,
        format!("{} kHz", state.sample_rate / 1000),
        theme::mono(7.5),
        theme::TEXT,
    );
    ui.painter().text(
        Pos2::new(content.right(), top + 14.0),
        Align2::RIGHT_TOP,
        "32-bit float",
        theme::mono(6.0),
        theme::TEXT_FAINT,
    );
    ui.painter().text(
        Pos2::new(content.right(), top + 26.0),
        Align2::RIGHT_TOP,
        format!("{} / {} voices", state.voices, MAX_VOICES),
        theme::mono(6.0),
        theme::TEXT_FAINT,
    );

    // Mini CPU load bar.
    let bar = Rect::from_min_size(
        Pos2::new(content.left(), content.bottom() - 12.0),
        Vec2::new(content.width(), 4.0),
    );
    ui.painter()
        .rect_filled(bar, CornerRadius::same(1), theme::BG_DEEP);
    let load = (state.cpu / 100.0).clamp(0.0, 1.0);
    ui.painter().rect_filled(
        Rect::from_min_size(bar.min, Vec2::new(bar.width() * load, bar.height())),
        CornerRadius::same(1),
        theme::CYAN,
    );
    ui.painter().text(
        Pos2::new(content.left(), bar.top() - 2.0),
        Align2::LEFT_BOTTOM,
        "DSP LOAD",
        theme::mono(5.5),
        theme::TEXT_FAINT,
    );
    ui.painter().text(
        Pos2::new(content.right(), bar.top() - 2.0),
        Align2::RIGHT_BOTTOM,
        format!("{:.1}%", state.cpu),
        theme::mono(5.5),
        theme::CYAN,
    );

    // Limiter status LED.
    let lit = state.master_peaks[0] < 0.995 && state.master_peaks[1] < 0.995;
    let led_center = Pos2::new(content.right() - 4.0, content.bottom() - 22.0);
    ui.painter()
        .circle_filled(led_center, 2.2, if lit { theme::GREEN } else { theme::RED });
    ui.painter().text(
        Pos2::new(led_center.x - 5.0, led_center.y),
        Align2::RIGHT_CENTER,
        "LIMIT",
        theme::mono(5.5),
        theme::TEXT_FAINT,
    );

    let _ = gain_to_db(state.master_peaks[0]);
    let _ = audio.sample_rate;
}

fn send_fx(audio: &mut AudioRuntime, param: FxParam, value: f32) {
    let _ = audio.command_tx.push(Command::SetFx { param, value });
}

fn format_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.1}k", hz / 1000.0)
    } else {
        format!("{hz:.0}")
    }
}
