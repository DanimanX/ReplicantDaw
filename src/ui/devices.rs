//! Bottom device chain: editable FM synth, modular effects and the master bus.
//!
//! Modules are painted left to right in signal order. Every effect can be
//! bypassed, reordered, closed (removed) and re-added from the `+ FX` menu.

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::audio::{
    gain_to_db, AudioRuntime, Command, EffectKind, EffectKnob, SynthParam, MAX_VOICES,
};
use crate::state::{UiEffect, UiState};
use crate::studio::Studio;
use crate::theme;
use crate::ui::widgets::{knob, meter};

const GAP: f32 = 7.0;
const HEADER_H: f32 = 20.0;

pub fn effect_color(kind: EffectKind) -> Color32 {
    match kind {
        EffectKind::Drive => theme::AMBER,
        EffectKind::Filter => theme::GREEN,
        EffectKind::Tone => theme::CYAN,
        EffectKind::Width => theme::MAGENTA,
        EffectKind::Delay => theme::BLUE,
    }
}

pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    state: &mut UiState,
    studio: &mut Studio,
    audio: &mut AudioRuntime,
) {
    let tag = format!("SYNTH → PLUGIN → {} FX → MASTER", state.effects.len());
    let content = theme::paint_panel(
        ui.painter(),
        rect,
        "DEVICE CHAIN // MODULAR SIGNAL PATH",
        &tag,
        theme::MAGENTA,
    );
    if content.height() < 60.0 {
        return;
    }

    // Width weights: synth and master are wider, the add button is compact.
    let effect_count = state.effects.len();
    let synth_weight = 1.55_f32;
    let plugin_weight = 1.12_f32;
    let effect_weight = 1.0_f32;
    let add_weight = 0.58_f32;
    let master_weight = 0.92_f32;
    let modules = effect_count + 4;
    let weights = synth_weight
        + plugin_weight
        + effect_weight * effect_count as f32
        + add_weight
        + master_weight;
    let usable = content.width() - GAP * (modules.saturating_sub(1)) as f32;
    let unit = (usable / weights).max(46.0);

    let mut x = content.left();
    let mut clicked: Option<ModuleAction> = None;
    let mut param_change: Option<(usize, usize, f32)> = None;
    let mut synth_change: Option<(usize, SynthParam, f32)> = None;

    // ---- Synth module ---------------------------------------------------
    let synth_rect = Rect::from_min_size(
        Pos2::new(x, content.top()),
        Vec2::new(unit * synth_weight, content.height()),
    );
    draw_synth(ui, synth_rect, state, &mut synth_change);
    x += synth_rect.width() + GAP;

    // ---- Per-track plugin insert ----------------------------------------
    let plugin_rect = Rect::from_min_size(
        Pos2::new(x, content.top()),
        Vec2::new(unit * plugin_weight, content.height()),
    );
    draw_plugin(ui, plugin_rect, studio, state, audio);
    x += plugin_rect.width() + GAP;

    // ---- Effect modules -------------------------------------------------
    for index in 0..effect_count {
        let rect = Rect::from_min_size(
            Pos2::new(x, content.top()),
            Vec2::new(unit * effect_weight, content.height()),
        );
        let mut effect_copy = state.effects[index];
        if draw_effect(
            ui,
            rect,
            index,
            &mut effect_copy,
            effect_count,
            &mut clicked,
            &mut param_change,
        ) {
            state.effects[index] = effect_copy;
        }
        x += rect.width() + GAP;
    }

    // ---- Add module menu ------------------------------------------------
    let add_rect = Rect::from_min_size(
        Pos2::new(x, content.top()),
        Vec2::new(unit * add_weight, content.height()),
    );
    draw_add_button(ui, add_rect, state);
    x += add_rect.width() + GAP;

    // ---- Master ---------------------------------------------------------
    let master_rect = Rect::from_min_size(
        Pos2::new(x, content.top()),
        Vec2::new(unit * master_weight, content.height()),
    );
    draw_master(ui, master_rect, state, audio);

    // ---- Apply engine commands after all modules are drawn --------------
    if let Some((index, knob_index, value)) = param_change {
        let _ = audio.command_tx.push(Command::SetEffectParam {
            index: index as u8,
            knob: knob_index as u8,
            value,
        });
    }
    if let Some((track, param, value)) = synth_change {
        state.synth[track].set(param, value);
        let _ = audio.command_tx.push(Command::SetSynthParam {
            track: track as u8,
            param,
            value,
        });
    }
    if let Some(action) = clicked {
        apply_action(action, state, audio);
    }
}

enum ModuleAction {
    Remove(usize),
    Bypass(usize, bool),
    Move(usize, usize),
}

fn apply_action(action: ModuleAction, state: &mut UiState, audio: &mut AudioRuntime) {
    match action {
        ModuleAction::Remove(index) => {
            state.remove_effect(index);
            let _ = audio
                .command_tx
                .push(Command::RemoveEffect { index: index as u8 });
        }
        ModuleAction::Bypass(index, bypass) => {
            if let Some(effect) = state.effects.get_mut(index) {
                effect.bypass = bypass;
            }
            let _ = audio.command_tx.push(Command::SetEffectBypass {
                index: index as u8,
                bypass,
            });
        }
        ModuleAction::Move(from, to) => {
            state.move_effect(from, to);
            let _ = audio.command_tx.push(Command::MoveEffect {
                from: from as u8,
                to: to as u8,
            });
        }
    }
}

fn module_shell(ui: &Ui, rect: Rect, title: &str, badge: &str, accent: Color32) -> Rect {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), theme::PANEL_RAISED);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );
    let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER_H));
    painter.rect_filled(header, CornerRadius::same(2), theme::HEADER);
    painter.line_segment(
        [header.left_bottom(), header.right_bottom()],
        Stroke::new(1.0, theme::BORDER),
    );
    painter.rect_filled(
        Rect::from_min_size(header.min, Vec2::new(2.0, header.height())),
        CornerRadius::same(1),
        accent,
    );
    let title_rect = header
        .shrink2(Vec2::new(7.0, 0.0))
        .with_max_x(header.right() - 44.0);
    let clipped = painter.with_clip_rect(title_rect);
    clipped.text(
        Pos2::new(title_rect.left(), header.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::display(7.6),
        accent,
    );
    if !badge.is_empty() {
        painter.text(
            Pos2::new(title_rect.right(), header.center().y),
            Align2::RIGHT_CENTER,
            badge,
            theme::mono(6.4),
            theme::TEXT_FAINT,
        );
    }
    Rect::from_min_max(
        Pos2::new(rect.left() + 6.0, header.bottom() + 4.0),
        Pos2::new(rect.right() - 6.0, rect.bottom() - 5.0),
    )
}

fn icon_button(ui: &mut Ui, rect: Rect, label: &str, accent: Color32) -> bool {
    let response = ui.interact(
        rect,
        ui.scope_id().with(("icon", label, rect.left() as i32)),
        Sense::click(),
    );
    let hovered = response.hovered();
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(1),
        if hovered {
            theme::accent_alpha(accent, 45)
        } else {
            theme::PANEL_SUNK
        },
    );
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(1),
        Stroke::new(1.0, if hovered { accent } else { theme::BORDER_SOFT }),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::display(6.4),
        if hovered {
            Color32::WHITE
        } else {
            theme::TEXT_DIM
        },
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

fn value_to_norm(value: f32, min: f32, max: f32, log: bool) -> f32 {
    if log {
        ((value.max(1.0e-6).ln() - min.max(1.0e-6).ln())
            / (max.max(1.0e-6).ln() - min.max(1.0e-6).ln()))
        .clamp(0.0, 1.0)
    } else {
        ((value - min) / (max - min).max(1.0e-9)).clamp(0.0, 1.0)
    }
}

fn norm_to_value(norm: f32, min: f32, max: f32, log: bool) -> f32 {
    if log {
        let lo = min.max(1.0e-6).ln();
        let hi = max.max(1.0e-6).ln();
        (lo + (hi - lo) * norm.clamp(0.0, 1.0)).exp()
    } else {
        min + (max - min) * norm.clamp(0.0, 1.0)
    }
}

fn draw_synth(
    ui: &mut Ui,
    rect: Rect,
    state: &mut UiState,
    change: &mut Option<(usize, SynthParam, f32)>,
) {
    let track = state.selected_track.min(crate::audio::TRACK_COUNT - 1);
    let accent = theme::track_accent(track);
    let inner = module_shell(
        ui,
        rect,
        "NEURAL GEN V5",
        &format!("TRK {:02}", track + 1),
        accent,
    );

    // Clicking the track badge cycles the edited track.
    let selector = Rect::from_min_size(inner.min, Vec2::new(inner.width(), 15.0));
    let hit = ui.interact(
        selector,
        ui.scope_id().with("synth_track_select"),
        Sense::click(),
    );
    if hit.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect_filled(
        selector,
        CornerRadius::same(1),
        if hit.hovered() {
            theme::accent_alpha(accent, 30)
        } else {
            theme::PANEL_SUNK
        },
    );
    ui.painter().text(
        Pos2::new(selector.left() + 4.0, selector.center().y),
        Align2::LEFT_CENTER,
        crate::audio::TRACKS[track].name,
        theme::mono(7.0),
        accent,
    );
    ui.painter().text(
        Pos2::new(selector.right() - 4.0, selector.center().y),
        Align2::RIGHT_CENTER,
        format!("EDIT ▸ {:02}", track + 1),
        theme::mono(6.0),
        theme::TEXT_FAINT,
    );
    if hit.clicked() {
        state.selected_track = (track + 1) % crate::audio::TRACK_COUNT;
    }

    let params = state.synth[track];
    let knob_area = Rect::from_min_max(Pos2::new(inner.left(), selector.bottom() + 3.0), inner.max);
    let columns = ((knob_area.width() / 58.0).floor() as usize).clamp(2, 4);
    let rows = SynthParam::ALL.len().div_ceil(columns);
    let cell_w = knob_area.width() / columns as f32;
    let cell_h = (knob_area.height() / rows as f32).max(34.0);

    for (index, param) in SynthParam::ALL.iter().enumerate() {
        let row = index / columns;
        let column = index % columns;
        let cell = Rect::from_min_size(
            Pos2::new(
                knob_area.left() + column as f32 * cell_w,
                knob_area.top() + row as f32 * cell_h,
            ),
            Vec2::new(cell_w, cell_h),
        );
        let (min, max) = param.range();
        let log = param.logarithmic();
        let mut norm = value_to_norm(params.get(*param), min, max, log);
        let changed = knob(
            ui,
            cell,
            ui.scope_id().with(("synth_knob", track, index)),
            &mut norm,
            0.0..=1.0,
            value_to_norm(
                match param {
                    SynthParam::Ratio => 1.0,
                    SynthParam::Index => 1.15,
                    SynthParam::Attack => 0.006,
                    SynthParam::Decay => 0.22,
                    SynthParam::Sustain => 0.30,
                    SynthParam::Release => 0.22,
                    SynthParam::Level => 0.9,
                },
                min,
                max,
                log,
            ),
            param.label(),
            &param.format(params.get(*param)),
            accent,
        );
        if changed {
            let value = norm_to_value(norm, min, max, log);
            *change = Some((track, *param, value));
        }
    }
}

fn draw_plugin(
    ui: &mut Ui,
    rect: Rect,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
) {
    let track = state.selected_track.min(crate::audio::TRACK_COUNT - 1);
    let accent = theme::GREEN;
    let inner = module_shell(
        ui,
        rect,
        "PLUGIN INSERT",
        &format!("TRK {:02}", track + 1),
        accent,
    );
    let info = studio.slots[track].clone();
    let pending = studio.slot_pending[track];

    match info {
        None => {
            let hit = ui.interact(
                inner,
                ui.scope_id().with(("plugin_load", track)),
                Sense::click(),
            );
            let hovered = hit.hovered();
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            ui.painter().rect_filled(
                inner,
                CornerRadius::same(2),
                if hovered {
                    theme::accent_alpha(accent, 18)
                } else {
                    theme::PANEL_SUNK
                },
            );
            ui.painter().rect_stroke(
                inner,
                CornerRadius::same(2),
                Stroke::new(1.0, theme::accent_alpha(accent, 70)),
                egui::StrokeKind::Inside,
            );
            let center = inner.center();
            ui.painter().text(
                Pos2::new(center.x, center.y - 12.0),
                Align2::CENTER_CENTER,
                if pending { "LOADING…" } else { "VST3 / CLAP" },
                theme::display(7.6),
                accent,
            );
            ui.painter().text(
                Pos2::new(center.x, center.y + 5.0),
                Align2::CENTER_CENTER,
                if pending { "" } else { "+ LOAD PLUGIN" },
                theme::display(8.4),
                if hovered { Color32::WHITE } else { theme::TEXT },
            );
            ui.painter().text(
                Pos2::new(center.x, center.y + 20.0),
                Align2::CENTER_CENTER,
                "ASSIGNED TO THIS TRACK",
                theme::mono(5.8),
                theme::TEXT_FAINT,
            );
            if hit.clicked() {
                studio.browser_target = track;
                studio.browser_open = true;
                if studio.scan_results.is_empty() && !studio.scan_running {
                    studio.start_scan();
                }
            }
            if hovered {
                hit.on_hover_text("Load a CLAP or VST3 insert for the selected track");
            }
        }
        Some(info) => {
            let name = info.name.clone();
            let vendor = info.vendor.clone();
            let format = info.format.label();
            let bypass = info.bypass;
            ui.painter().text(
                Pos2::new(inner.left() + 2.0, inner.top() + 2.0),
                Align2::LEFT_TOP,
                name.chars().take(22).collect::<String>(),
                theme::display(7.4),
                if bypass { theme::TEXT_DIM } else { accent },
            );
            ui.painter().text(
                Pos2::new(inner.left() + 2.0, inner.top() + 14.0),
                Align2::LEFT_TOP,
                format!(
                    "{} · {}",
                    format,
                    vendor.chars().take(16).collect::<String>()
                ),
                theme::mono(6.0),
                theme::TEXT_FAINT,
            );
            ui.painter().text(
                Pos2::new(inner.left() + 2.0, inner.top() + 25.0),
                Align2::LEFT_TOP,
                format!("{} params · {} smp", info.params.len(), info.latency),
                theme::mono(5.8),
                theme::TEXT_FAINT,
            );

            let button = 13.0;
            let y = inner.bottom() - button;
            let load = Rect::from_min_size(
                Pos2::new(inner.left(), y),
                Vec2::new((inner.width() - 2.0 * (button + 2.0)).max(20.0), button),
            );
            if icon_button(ui, load, "LOAD", accent) {
                studio.browser_target = track;
                studio.browser_open = true;
                if studio.scan_results.is_empty() && !studio.scan_running {
                    studio.start_scan();
                }
            }
            let bypass_rect =
                Rect::from_min_size(Pos2::new(load.right() + 2.0, y), Vec2::splat(button));
            let close =
                Rect::from_min_size(Pos2::new(bypass_rect.right() + 2.0, y), Vec2::splat(button));
            if icon_button(ui, bypass_rect, if bypass { "B" } else { "b" }, accent) {
                studio.set_bypass(track, !bypass);
            }
            if icon_button(ui, close, "X", theme::RED) {
                studio.unload_plugin(track, audio);
            }
            let hit = ui.interact(
                Rect::from_min_max(inner.min, Pos2::new(inner.right(), y - 2.0)),
                ui.scope_id().with(("plugin_params", track)),
                Sense::click(),
            );
            if hit.clicked() {
                studio.selected_slot = track;
                studio.rack_open = true;
            }
            if hit.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                hit.on_hover_text("Open the plugin rack to edit parameters");
            }
        }
    }
}

fn draw_effect(
    ui: &mut Ui,
    rect: Rect,
    index: usize,
    effect: &mut UiEffect,
    count: usize,
    clicked: &mut Option<ModuleAction>,
    param_change: &mut Option<(usize, usize, f32)>,
) -> bool {
    let accent = effect_color(effect.kind);
    let inner = module_shell(
        ui,
        rect,
        effect.kind.name(),
        effect.kind.short(),
        if effect.bypass {
            theme::TEXT_FAINT
        } else {
            accent
        },
    );

    // Header controls: close, bypass, reorder.
    let button = 13.0;
    let right = rect.right() - 4.0;
    let y = rect.top() + (HEADER_H - button) * 0.5;
    let close = Rect::from_min_size(Pos2::new(right - button, y), Vec2::splat(button));
    let bypass = Rect::from_min_size(
        Pos2::new(close.left() - button - 2.0, y),
        Vec2::splat(button),
    );
    if icon_button(ui, close, "X", theme::RED) {
        *clicked = Some(ModuleAction::Remove(index));
    }
    if icon_button(ui, bypass, if effect.bypass { "B" } else { "b" }, accent) {
        *clicked = Some(ModuleAction::Bypass(index, !effect.bypass));
    }

    if inner.height() > 40.0 {
        let nav = 11.0;
        let nav_y = inner.bottom() - nav;
        let left_btn = Rect::from_min_size(Pos2::new(inner.left(), nav_y), Vec2::new(nav, nav));
        let right_btn = Rect::from_min_size(
            Pos2::new(inner.left() + nav + 2.0, nav_y),
            Vec2::new(nav, nav),
        );
        if index > 0 && icon_button(ui, left_btn, "‹", accent) {
            *clicked = Some(ModuleAction::Move(index, index - 1));
        }
        if index + 1 < count && icon_button(ui, right_btn, "›", accent) {
            *clicked = Some(ModuleAction::Move(index, index + 1));
        }
        ui.painter().text(
            Pos2::new(inner.right(), nav_y + nav * 0.5),
            Align2::RIGHT_CENTER,
            format!("{:02}", index + 1),
            theme::mono(6.2),
            theme::TEXT_FAINT,
        );
    }

    let knob_area = Rect::from_min_max(inner.min, Pos2::new(inner.right(), inner.bottom() - 13.0));
    let specs = effect.kind.knobs();
    let columns = if knob_area.width() >= 120.0 { 3 } else { 2 };
    let rows = specs.len().div_ceil(columns);
    let cell_w = knob_area.width() / columns as f32;
    let cell_h = (knob_area.height() / rows as f32).max(34.0);

    let mut changed = false;
    for (knob_index, spec) in specs.iter().enumerate() {
        let row = knob_index / columns;
        let column = knob_index % columns;
        let cell = Rect::from_min_size(
            Pos2::new(
                knob_area.left() + column as f32 * cell_w,
                knob_area.top() + row as f32 * cell_h,
            ),
            Vec2::new(cell_w, cell_h),
        );
        let mut norm = value_to_norm(
            effect.params[knob_index],
            spec.min,
            spec.max,
            spec.logarithmic,
        );
        let knob_accent = if effect.bypass {
            theme::TEXT_FAINT
        } else {
            accent
        };
        let changed_knob = knob(
            ui,
            cell,
            ui.scope_id().with(("fx_knob", index, knob_index)),
            &mut norm,
            0.0..=1.0,
            value_to_norm(spec.default, spec.min, spec.max, spec.logarithmic),
            spec.name,
            &format_effect_value(spec, effect.params[knob_index]),
            knob_accent,
        );
        if changed_knob {
            let value = norm_to_value(norm, spec.min, spec.max, spec.logarithmic);
            effect.params[knob_index] = value;
            *param_change = Some((index, knob_index, value));
            changed = true;
        }
    }
    changed
}

fn format_effect_value(spec: &EffectKnob, value: f32) -> String {
    match spec.suffix {
        "%" => format!("{:.0}%", value * 100.0),
        "Hz" => {
            if value >= 1000.0 {
                format!("{:.1}k", value / 1000.0)
            } else {
                format!("{value:.0}")
            }
        }
        "s" => format!("{value:.2}s"),
        _ => format!("{value:.2}x"),
    }
}

fn draw_add_button(ui: &mut Ui, rect: Rect, state: &mut UiState) {
    let accent = theme::CYAN;
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), theme::PANEL_SUNK);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );
    let hit = ui.interact(
        rect,
        ui.scope_id().with("add_effect_module"),
        Sense::click(),
    );
    let hovered = hit.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if hovered {
        theme::CYAN
    } else {
        theme::TEXT_DIM
    };
    let center = rect.center();
    painter.text(
        Pos2::new(center.x, center.y - 8.0),
        Align2::CENTER_CENTER,
        "+",
        theme::display(22.0),
        color,
    );
    painter.text(
        Pos2::new(center.x, center.y + 14.0),
        Align2::CENTER_CENTER,
        "ADD FX",
        theme::display(7.0),
        color,
    );
    if hit.clicked() {
        state.add_effect_open = !state.add_effect_open;
        if state.add_effect_open {
            state.add_menu_pos = Pos2::new(rect.left(), (rect.top() - 230.0).max(70.0));
            state.add_menu_pos.x = state.add_menu_pos.x.min(rect.right() - 180.0);
        }
    }
    let _ = accent;
}

/// Floating menu with the available effect modules.
pub fn draw_add_menu(ctx: &egui::Context, state: &mut UiState, audio: &mut AudioRuntime) {
    if !state.add_effect_open {
        return;
    }
    let mut close = false;
    let mut chosen = None;
    egui::Area::new(egui::Id::unique("add_effect_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(state.add_menu_pos)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(theme::PANEL)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("INSERT EFFECT MODULE")
                            .font(theme::display(9.0))
                            .color(theme::CYAN),
                    );
                    ui.add_space(4.0);
                    if state.effects.len() >= crate::audio::MAX_EFFECTS {
                        ui.label(
                            egui::RichText::new("Chain is full (6 modules). Close one first.")
                                .font(theme::mono(8.0))
                                .color(theme::AMBER),
                        );
                    }
                    for kind in EffectKind::ALL {
                        let enabled = state.effects.len() < crate::audio::MAX_EFFECTS;
                        let label = format!("{:<8} {}", kind.short(), kind.name());
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new(
                                    egui::RichText::new(label)
                                        .font(theme::mono(9.0))
                                        .color(effect_color(kind)),
                                )
                                .fill(theme::PANEL_SUNK),
                            )
                            .clicked()
                        {
                            chosen = Some(kind);
                        }
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(egui::RichText::new("RESET CHAIN").font(theme::display(8.0)))
                            .clicked()
                        {
                            state.pending_chain_reset = true;
                        }
                        if ui
                            .button(egui::RichText::new("CLOSE").font(theme::display(8.0)))
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
        });
    if let Some(kind) = chosen {
        if state.add_effect(kind).is_some() {
            let _ = audio.command_tx.push(Command::AddEffect { kind });
        }
        close = true;
    }
    if state.pending_chain_reset {
        state.pending_chain_reset = false;
        state.reset_chain();
        let _ = audio.command_tx.push(Command::ClearEffects);
        for effect in &state.effects {
            let _ = audio
                .command_tx
                .push(Command::AddEffect { kind: effect.kind });
        }
        let _ = audio.command_tx.push(Command::SetMasterGain {
            gain: state.master_gain,
        });
        close = true;
    }
    if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.add_effect_open = false;
    }
}

fn draw_master(ui: &mut Ui, rect: Rect, state: &mut UiState, audio: &mut AudioRuntime) {
    let inner = module_shell(ui, rect, "MASTER BUS", "STEREO", theme::MAGENTA);
    let meter_w = 7.0;
    let meter_h = (inner.height() - 26.0).clamp(20.0, 90.0);
    for channel in 0..2 {
        let meter_rect = Rect::from_min_size(
            Pos2::new(inner.left() + channel as f32 * (meter_w + 2.0), inner.top()),
            Vec2::new(meter_w, meter_h),
        );
        meter(ui, meter_rect, state.master_peaks[channel]);
        ui.painter().text(
            Pos2::new(meter_rect.center().x, meter_rect.bottom() + 1.0),
            Align2::CENTER_TOP,
            if channel == 0 { "L" } else { "R" },
            theme::mono(5.6),
            theme::TEXT_FAINT,
        );
    }

    let text_x = inner.left() + 22.0;
    let master_db = state.master_db();
    let color = if master_db > -1.0 {
        theme::RED
    } else if master_db > -8.0 {
        theme::AMBER
    } else {
        theme::GREEN
    };
    ui.painter().text(
        Pos2::new(text_x, inner.top() + 1.0),
        Align2::LEFT_TOP,
        "OUT",
        theme::mono(5.8),
        theme::TEXT_FAINT,
    );
    ui.painter().text(
        Pos2::new(text_x, inner.top() + 10.0),
        Align2::LEFT_TOP,
        if master_db > -60.0 {
            format!("{master_db:+.1}")
        } else {
            "-inf".to_string()
        },
        theme::mono(15.0),
        color,
    );
    ui.painter().text(
        Pos2::new(text_x, inner.top() + 27.0),
        Align2::LEFT_TOP,
        "dBFS",
        theme::mono(5.6),
        theme::TEXT_FAINT,
    );

    // Master gain knob.
    let knob_rect = Rect::from_min_size(
        Pos2::new(inner.left() + inner.width() * 0.52, inner.top()),
        Vec2::new(inner.width() * 0.44, 58.0),
    );
    let mut gain = state.master_gain;
    let gain_text = format!("{:.2}x", state.master_gain);
    if knob(
        ui,
        knob_rect,
        ui.scope_id().with("master_gain"),
        &mut gain,
        0.0..=1.6,
        0.9,
        "GAIN",
        &gain_text,
        theme::MAGENTA,
    ) {
        state.master_gain = gain;
        let _ = audio.command_tx.push(Command::SetMasterGain { gain });
    }

    ui.painter().text(
        Pos2::new(inner.right(), inner.top() + 1.0),
        Align2::RIGHT_TOP,
        format!("{} kHz", state.sample_rate / 1000),
        theme::mono(6.8),
        theme::TEXT,
    );
    ui.painter().text(
        Pos2::new(inner.right(), inner.top() + 11.0),
        Align2::RIGHT_TOP,
        format!("{} voices", MAX_VOICES.min(state.voices as usize)),
        theme::mono(5.8),
        theme::TEXT_FAINT,
    );

    let bar = Rect::from_min_size(
        Pos2::new(inner.left(), inner.bottom() - 8.0),
        Vec2::new(inner.width(), 4.0),
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
        Pos2::new(inner.left(), bar.top() - 1.0),
        Align2::LEFT_BOTTOM,
        format!("DSP {:.1}%", state.cpu),
        theme::mono(5.4),
        theme::TEXT_FAINT,
    );

    let lit = state.master_peaks[0] < 0.995 && state.master_peaks[1] < 0.995;
    ui.painter().circle_filled(
        Pos2::new(inner.right() - 3.0, bar.top() - 4.0),
        2.0,
        if lit { theme::GREEN } else { theme::RED },
    );
    let _ = gain_to_db(state.master_peaks[0]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_mapping_round_trips() {
        for value in [0.02_f32, 0.1, 0.28, 1.0] {
            let norm = value_to_norm(value, 0.02, 1.0, true);
            let back = norm_to_value(norm, 0.02, 1.0, true);
            assert!((value - back).abs() < 1e-5, "{value} -> {norm} -> {back}");
        }
    }

    #[test]
    fn linear_mapping_is_bounded() {
        assert!((value_to_norm(-5.0, 0.0, 10.0, false)).abs() < 1e-6);
        assert!((value_to_norm(50.0, 0.0, 10.0, false) - 1.0).abs() < 1e-6);
        assert!((norm_to_value(0.5, 0.0, 10.0, false) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn effect_colors_are_distinct() {
        let colors: Vec<_> = EffectKind::ALL
            .iter()
            .map(|kind| effect_color(*kind))
            .collect();
        for (index, color) in colors.iter().enumerate() {
            for other in colors.iter().skip(index + 1) {
                assert_ne!(color, other);
            }
        }
    }
}
