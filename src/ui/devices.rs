//! Bottom device chain: instrument slot, modular effects and the master bus.
//!
//! The first module is the track's instrument slot. It holds the internal
//! `NEURAL GEN XT` synth and the CLAP/VST3 instrument insert, and lets you
//! choose which one sounds. Modules are painted left to right in signal order;
//! every effect can be bypassed, reordered, closed and re-added from `+ FX`.

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
    let tag = format!("INSTRUMENT → {} FX → MASTER", state.effects.len());
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

    // Width weights: the instrument slot and master are wider, the add
    // button is compact.
    let effect_count = state.effects.len();
    let instrument_weight = 2.55_f32;
    let effect_weight = 1.0_f32;
    let add_weight = 0.58_f32;
    let master_weight = 0.95_f32;
    let modules = effect_count + 3;
    let weights =
        instrument_weight + effect_weight * effect_count as f32 + add_weight + master_weight;
    let usable = content.width() - GAP * (modules.saturating_sub(1)) as f32;
    let unit = (usable / weights).max(46.0);

    let mut x = content.left();
    let mut clicked: Option<ModuleAction> = None;
    let mut param_change: Option<(usize, usize, f32)> = None;
    let mut synth_change: Option<(usize, SynthParam, f32)> = None;

    // ---- Instrument slot (internal synth + plugin insert) ----------------
    let instrument_rect = Rect::from_min_size(
        Pos2::new(x, content.top()),
        Vec2::new(unit * instrument_weight, content.height()),
    );
    draw_instrument(ui, instrument_rect, studio, state, audio, &mut synth_change);
    x += instrument_rect.width() + GAP;

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

fn module_shell(
    ui: &Ui,
    rect: Rect,
    title: &str,
    badge: &str,
    accent: Color32,
    header_reserve: f32,
) -> Rect {
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
    // The caller owns the header buttons; the title must not run under them.
    let title_rect = header
        .shrink2(Vec2::new(7.0, 0.0))
        .with_max_x(header.right() - header_reserve);
    let clipped = painter.with_clip_rect(title_rect);
    let font = theme::fitted_font(&clipped, title, theme::display(7.6), title_rect.width());
    clipped.text(
        Pos2::new(title_rect.left(), header.center().y),
        Align2::LEFT_CENTER,
        title,
        font,
        accent,
    );
    if !badge.is_empty() && rect.width() >= 150.0 {
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

/// The track's instrument slot.
///
/// Mutually exclusive instrument generator: either the internal `SURGE WT SYNTH`
/// or the loaded `VST3 / CLAP` plugin. Exactly one is active and sounds per track.
fn draw_instrument(
    ui: &mut Ui,
    rect: Rect,
    studio: &mut Studio,
    state: &mut UiState,
    audio: &mut AudioRuntime,
    change: &mut Option<(usize, SynthParam, f32)>,
) {
    use crate::state::InstrumentSource;

    let track = state.selected_track.min(crate::audio::TRACK_COUNT - 1);
    let accent = theme::track_accent(track);
    let info = studio.slots[track].clone();
    let pending = studio.slot_pending[track];
    let mut source = state.instrument[track];
    // A plugin can only be the source when one is actually loaded. While a
    // load is in flight the slot stays selected so the panel shows progress
    // instead of falling back to the synth that is no longer routed.
    if source == InstrumentSource::Plugin && info.is_none() && !pending {
        source = InstrumentSource::Empty;
        state.instrument[track] = source;
    }

    // The slot holds one of three things, and the panel shows exactly that.
    // There is no separate "source" switch: it would let the synth and a
    // loaded plugin claim the same track.
    let title = match (&info, pending, source) {
        (Some(info), _, _) if info.is_instrument => info.name.clone(),
        // An effect plugin is a track insert; the synth is still the source.
        (Some(info), _, _) => format!("{} · FX INSERT", info.name),
        (None, true, _) => "LOADING PLUGIN…".into(),
        (None, false, InstrumentSource::Synth) => "SURGE WT SYNTH".into(),
        (None, false, _) => "INSTRUMENT // EMPTY".into(),
    };

    // GUI + E + b + X sit in the header when a plugin is loaded.
    let header_reserve = if info.is_some() { 84.0 } else { 40.0 };
    let inner = module_shell(
        ui,
        rect,
        &title,
        &format!("TRK {:02}", track + 1),
        accent,
        header_reserve,
    );

    // ---- Empty slot: the call to action --------------------------------
    if info.is_none() && !pending && source != InstrumentSource::Synth {
        let hit = ui.interact(
            inner,
            ui.scope_id().with(("instrument_add", track)),
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
                theme::accent_alpha(accent, 22)
            } else {
                theme::PANEL_SUNK
            },
        );
        ui.painter().rect_stroke(
            inner,
            CornerRadius::same(2),
            Stroke::new(
                1.2,
                if hovered {
                    theme::accent_alpha(accent, 190)
                } else {
                    theme::accent_alpha(accent, 70)
                },
            ),
            egui::StrokeKind::Inside,
        );
        let center = inner.center();
        ui.painter().text(
            Pos2::new(center.x, center.y - 20.0),
            Align2::CENTER_CENTER,
            "+",
            theme::display(26.0),
            if hovered { Color32::WHITE } else { accent },
        );
        let cta = "SELECT SYNTH OR VST/CLAP";
        let cta_painter = ui
            .painter()
            .with_clip_rect(inner.shrink2(Vec2::new(4.0, 0.0)));
        let cta_font = theme::fitted_font(
            &cta_painter,
            cta,
            theme::display(10.0),
            inner.width() - 12.0,
        );
        cta_painter.text(
            Pos2::new(center.x, center.y + 6.0),
            Align2::CENTER_CENTER,
            cta,
            cta_font,
            if hovered { Color32::WHITE } else { theme::TEXT },
        );
        ui.painter().text(
            Pos2::new(center.x, center.y + 26.0),
            Align2::CENTER_CENTER,
            "CLICK TO OPEN INSTRUMENT BROWSER",
            theme::mono(8.0),
            theme::TEXT_FAINT,
        );
        if hit.clicked() {
            studio.open_instrument_browser(track);
        }
        if hovered {
            hit.on_hover_text(
                "Pick a CLAP/VST3 plugin installed on this system, or the internal Surge WT synth",
            );
        }
        return;
    }

    // ---- Header strip: which track this module edits -------------------
    let row_h = 16.0;
    let track_select = Rect::from_min_size(inner.min, Vec2::new(inner.width(), row_h));

    let track_hit = ui.interact(
        track_select,
        ui.scope_id().with(("instrument_track", track)),
        Sense::click(),
    );
    ui.painter().rect_filled(
        track_select,
        CornerRadius::same(1),
        if track_hit.hovered() {
            theme::accent_alpha(accent, 30)
        } else {
            theme::PANEL_SUNK
        },
    );
    let track_painter = ui
        .painter()
        .with_clip_rect(track_select.shrink2(Vec2::new(2.0, 0.0)));
    let label_font = theme::fitted_font(
        &track_painter,
        state.track_label(track),
        theme::mono(8.5),
        track_select.width() - 6.0,
    );
    track_painter.text(
        Pos2::new(track_select.left() + 4.0, track_select.center().y),
        Align2::LEFT_CENTER,
        state.track_label(track),
        label_font,
        accent,
    );
    if track_hit.clicked() {
        state.selected_track = (track + 1) % crate::audio::TRACK_COUNT;
    }
    if track_hit.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        track_hit.on_hover_text("Click to edit the next track");
    }

    // ---- Action Buttons in the Top Module Header -----------------------
    let button = 14.0;
    let y = rect.top() + (HEADER_H - button) * 0.5;
    let right = rect.right() - 4.0;

    if info.is_some() {
        let close = Rect::from_min_size(Pos2::new(right - button, y), Vec2::splat(button));
        let bypass = Rect::from_min_size(
            Pos2::new(close.left() - button - 3.0, y),
            Vec2::splat(button),
        );
        let edit = Rect::from_min_size(
            Pos2::new(bypass.left() - button - 3.0, y),
            Vec2::splat(button),
        );
        let gui = Rect::from_min_size(
            Pos2::new(edit.left() - 26.0 - 3.0, y),
            Vec2::new(26.0, button),
        );

        if icon_button(ui, edit, "E", accent) {
            studio.open_plugin_editor(track, false);
        }
        if icon_button(
            ui,
            bypass,
            if info.as_ref().is_some_and(|slot| slot.bypass) {
                "B"
            } else {
                "b"
            },
            accent,
        ) {
            let bypass_now = !info.as_ref().is_some_and(|slot| slot.bypass);
            studio.set_bypass(track, bypass_now);
        }
        if icon_button(ui, close, "X", theme::RED) {
            studio.unload_plugin(track, audio);
            state.instrument[track] = crate::studio::source_after_unload(state.instrument[track]);
        }
        if info.as_ref().is_some() {
            let is_open = info.as_ref().is_some_and(|slot| slot.gui_open);
            if icon_button(ui, gui, if is_open { "GUI*" } else { "GUI" }, theme::CYAN) {
                if is_open {
                    studio.close_plugin_gui(track);
                } else {
                    studio.open_plugin_gui(track);
                }
            }
        }
    } else if !pending && source == InstrumentSource::Synth {
        let edit = Rect::from_min_size(Pos2::new(right - 34.0, y), Vec2::new(34.0, button));
        if icon_button(ui, edit, "EDIT", accent) {
            studio.synth_open = true;
        }
    }

    let content_top = inner.top() + row_h + 4.0;
    let synth_mode = info.is_none() && !pending && source == InstrumentSource::Synth;
    // The synth panel keeps one line at the bottom for its engine badge.
    let content = Rect::from_min_max(
        Pos2::new(inner.left(), content_top),
        Pos2::new(
            inner.right(),
            inner.bottom() - if synth_mode { 14.0 } else { 0.0 },
        ),
    );

    // ---- A loaded plugin owns the panel ---------------------------------
    if let Some(info) = &info {
        draw_plugin_parameters(ui, content, studio, track, info, accent);
        return;
    }
    if pending {
        // The instance is still starting on the plugin worker thread.
        ui.painter().text(
            content.center(),
            Align2::CENTER_CENTER,
            "LOADING PLUGIN…",
            theme::mono(9.0),
            theme::TEXT_DIM,
        );
        return;
    }

    // ---- If Synth is selected, render Surge WT Synth Controls ONLY ------
    let params = state.synth[track];
    let compact = [
        SynthParam::OscMorph(0),
        SynthParam::FilterCutoff(0),
        SynthParam::FilterResonance(0),
        SynthParam::OscLevel(1),
        SynthParam::OscDetune(0),
        SynthParam::EnvRelease(0),
    ];
    let columns = ((content.width() / 62.0).floor() as usize).clamp(2, 3);
    let rows = compact.len().div_ceil(columns);
    let cell_w = content.width() / columns as f32;
    let cell_h = (content.height() / rows as f32).max(32.0);

    for (index, param) in compact.iter().enumerate() {
        let row = index / columns;
        let column = index % columns;
        let cell = Rect::from_min_size(
            Pos2::new(
                content.left() + column as f32 * cell_w,
                content.top() + row as f32 * cell_h,
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
            value_to_norm(param.default_value(), min, max, log),
            &param.label(),
            &param.format(params.get(*param)),
            accent,
        );
        if changed {
            let value = norm_to_value(norm, min, max, log);
            *change = Some((track, *param, value));
        }
    }

    // Badge that names the internal Surge WT engine and opens the full editor.
    let badge = Rect::from_min_size(
        Pos2::new(inner.left(), content.bottom() + 1.0),
        Vec2::new(content.width(), 13.0),
    );
    let badge_hit = ui.interact(
        badge,
        ui.scope_id().with(("synth_editor_open", track)),
        Sense::click(),
    );
    ui.painter().text(
        Pos2::new(badge.left() + 2.0, badge.center().y),
        Align2::LEFT_CENTER,
        "SURGE WT SYNTH · 1:1 WAVETABLE ENGINE",
        theme::mono(8.0),
        theme::TEXT_DIM,
    );
    ui.painter().text(
        Pos2::new(badge.right() - 2.0, badge.center().y),
        Align2::RIGHT_CENTER,
        if badge_hit.hovered() {
            "OPEN FULL EDITOR >"
        } else {
            "EDITOR >"
        },
        theme::mono(8.0),
        if badge_hit.hovered() {
            accent
        } else {
            theme::TEXT_FAINT
        },
    );
    if badge_hit.clicked() {
        studio.synth_open = true;
    }
    if badge_hit.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        badge_hit.on_hover_text("Open the Surge WT Synth full editor");
    }
}

/// Parameter grid + edit controls for the hosted plugin instrument.
fn draw_plugin_parameters(
    ui: &mut Ui,
    content: Rect,
    studio: &mut Studio,
    track: usize,
    info: &crate::plugins::PluginSlotInfo,
    accent: Color32,
) {
    let bypass = info.bypass;
    let header = Rect::from_min_size(content.min, Vec2::new(content.width(), 13.0));
    ui.painter().text(
        Pos2::new(header.left() + 2.0, header.top() + 2.0),
        Align2::LEFT_TOP,
        format!("{} · {}", info.format.label(), info.vendor),
        theme::mono(6.0),
        theme::TEXT_FAINT,
    );
    let open = Rect::from_min_size(
        Pos2::new(header.right() - 62.0, header.top()),
        Vec2::new(60.0, 12.0),
    );
    let open_hit = ui.interact(
        open,
        ui.scope_id().with(("plugin_open_editor", track)),
        Sense::click(),
    );
    ui.painter().rect_filled(
        open,
        CornerRadius::same(1),
        if open_hit.hovered() {
            theme::accent_alpha(accent, 45)
        } else {
            theme::PANEL_SUNK
        },
    );
    ui.painter().rect_stroke(
        open,
        CornerRadius::same(1),
        Stroke::new(1.0, theme::accent_alpha(accent, 120)),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        open.center(),
        Align2::CENTER_CENTER,
        "OPEN EDITOR",
        theme::display(6.0),
        accent,
    );
    if open_hit.clicked() {
        studio.open_plugin_editor(track, false);
    }
    if open_hit.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        open_hit.on_hover_text("Open the plugin editor full screen");
    }

    let knob_area = Rect::from_min_max(
        Pos2::new(content.left(), header.bottom() + 2.0),
        Pos2::new(content.right(), content.bottom()),
    );
    if info.params.is_empty() {
        ui.painter().text(
            knob_area.center(),
            Align2::CENTER_CENTER,
            "NO PARAMETERS · OPEN THE EDITOR",
            theme::mono(6.4),
            theme::TEXT_FAINT,
        );
        return;
    }

    let columns = ((knob_area.width() / 62.0).floor() as usize).clamp(2, 4);
    let visible = (columns * 2).min(info.params.len());
    let rows = visible.div_ceil(columns);
    let cell_w = knob_area.width() / columns as f32;
    let cell_h = (knob_area.height() / rows as f32).max(32.0);
    let mut changes = Vec::new();
    for (index, param) in info.params.iter().take(visible).enumerate() {
        let row = index / columns;
        let column = index % columns;
        let cell = Rect::from_min_size(
            Pos2::new(
                knob_area.left() + column as f32 * cell_w,
                knob_area.top() + row as f32 * cell_h,
            ),
            Vec2::new(cell_w, cell_h),
        );
        let mut value = param.value;
        let text = format!("{:.0}%", value * 100.0);
        if knob(
            ui,
            cell,
            ui.scope_id().with(("instrument_param", track, index)),
            &mut value,
            0.0..=1.0,
            param.value,
            &param.name.chars().take(9).collect::<String>(),
            &text,
            if bypass { theme::TEXT_FAINT } else { accent },
        ) {
            changes.push((param.id, value));
        }
    }
    for (id, value) in changes {
        if let Some(info) = &studio.slots[track] {
            if let Some(param) = info.params.iter().find(|p| p.id == id).cloned() {
                studio.set_param(track, &param, value);
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
        46.0,
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
    let inner = module_shell(ui, rect, "MASTER BUS", "STEREO", theme::MAGENTA, 40.0);
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
