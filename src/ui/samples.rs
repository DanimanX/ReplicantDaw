//! Sample browser drawer (right of the clip matrix) and sample editor.
//!
//! The drawer is deliberately an overlay: opening it never shrinks the clip
//! matrix, so samples can be dragged from it onto a cell while the grid keeps
//! its full size.

use std::path::PathBuf;

use egui::{Align2, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::audio::AudioRuntime;
use crate::samples::{DecodedSample, SampleEntry};
use crate::studio::{Studio, TrimEdge};
use crate::theme;

const TAB_W: f32 = 92.0;
const TAB_H: f32 = 18.0;
const PANEL_W: f32 = 320.0;
const ROW_H: f32 = 20.0;

/// Small tab attached to the matrix's top-right corner.
pub fn draw_browser(ctx: &egui::Context, matrix: Rect, studio: &mut Studio) {
    let tab_pos = Pos2::new(
        (matrix.right() - TAB_W - 4.0).max(matrix.left() + 4.0),
        matrix.top() + 4.0,
    );
    let open = studio.sample_browser_open;

    egui::Area::new(egui::Id::unique("sample_browser_tab"))
        .order(egui::Order::Middle)
        .fixed_pos(tab_pos)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(Vec2::new(TAB_W, TAB_H), Sense::click());
            let hovered = response.hovered();
            let fill = if open {
                theme::accent_alpha(theme::CYAN, 55)
            } else if hovered {
                theme::PANEL_RAISED
            } else {
                theme::PANEL_SUNK
            };
            ui.painter().rect_filled(rect, CornerRadius::same(2), fill);
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(2),
                Stroke::new(
                    1.0,
                    if open || hovered {
                        theme::CYAN
                    } else {
                        theme::accent_alpha(theme::CYAN, 120)
                    },
                ),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                if open { "SAMPLES  <" } else { "SAMPLES  >" },
                theme::display(7.0),
                if open { theme::CYAN } else { theme::TEXT_DIM },
            );
            if response.clicked() {
                studio.sample_browser_open = !open;
                if !open {
                    studio.reload_sample_dir();
                }
            }
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                response.on_hover_text(
                    "Browse WAV / FLAC / MP3 samples and drag them into the clip matrix",
                );
            }
        });

    if !open {
        return;
    }

    let screen = ctx.content_rect();
    let panel_pos = Pos2::new(matrix.right() + 6.0, matrix.top() + 4.0);
    // Stay beside the clip matrix instead of covering the mixer below it.
    let panel_h = (matrix.height() - 8.0)
        .min(screen.bottom() - panel_pos.y - 8.0)
        .clamp(240.0, 560.0);
    let panel_w = (screen.right() - panel_pos.x - 8.0).clamp(230.0, PANEL_W);

    egui::Area::new(egui::Id::unique("sample_browser_panel"))
        .order(egui::Order::Middle)
        .fixed_pos(panel_pos)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(Stroke::new(1.0, theme::accent_alpha(theme::CYAN, 140)))
                .corner_radius(CornerRadius::same(3))
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.set_width(panel_w - 16.0);
                    ui.set_height(panel_h - 16.0);
                    ui.spacing_mut().item_spacing = Vec2::new(3.0, 3.0);

                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("SAMPLE BROWSER")
                                .font(theme::display(9.0))
                                .color(theme::CYAN),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(RichText::new("X").font(theme::display(8.0)))
                                        .fill(theme::PANEL_SUNK),
                                )
                                .clicked()
                            {
                                studio.sample_browser_open = false;
                            }
                        });
                    });
                    ui.label(
                        RichText::new("WAV · FLAC · MP3")
                            .font(theme::mono(6.4))
                            .color(theme::TEXT_FAINT),
                    );

                    ui.horizontal(|ui| {
                        if compact_button(ui, "UP").clicked() {
                            if let Some(parent) = studio.sample_dir.parent() {
                                let parent = parent.to_path_buf();
                                studio.set_sample_dir(parent);
                            }
                        }
                        if compact_button(ui, "HOME").clicked() {
                            studio.set_sample_dir(crate::samples::home_dir());
                        }
                        if compact_button(ui, "SAMPLES").clicked() {
                            let home = crate::samples::home_dir();
                            let dir = if home.join("Samples").is_dir() {
                                home.join("Samples")
                            } else {
                                home
                            };
                            studio.set_sample_dir(dir);
                        }
                        if compact_button(ui, "REFRESH").clicked() {
                            studio.reload_sample_dir();
                        }
                    });
                    ui.add(
                        egui::Label::new(
                            RichText::new(studio.sample_dir.display().to_string())
                                .font(theme::mono(6.6))
                                .color(theme::TEXT_DIM),
                        )
                        .truncate(),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut studio.sample_filter)
                            .hint_text("filter samples")
                            .desired_width(f32::INFINITY)
                            .font(theme::mono(8.0)),
                    );

                    let filter = studio.sample_filter.trim().to_lowercase();
                    // References only: the list can be long and this runs
                    // every frame while the drawer is open.
                    let entries: Vec<&SampleEntry> = studio
                        .sample_entries
                        .iter()
                        .filter(|entry| {
                            filter.is_empty() || entry.name.to_lowercase().contains(&filter)
                        })
                        .collect();

                    let list_h = (panel_h - 122.0).max(80.0);
                    let mut new_dir: Option<PathBuf> = None;
                    let mut open_file: Option<PathBuf> = None;

                    egui::ScrollArea::vertical()
                        .max_height(list_h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if entries.is_empty() {
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new(if filter.is_empty() {
                                        "No WAV / FLAC / MP3 files in this folder."
                                    } else {
                                        "Nothing matches the filter."
                                    })
                                    .font(theme::mono(7.0))
                                    .color(theme::TEXT_FAINT),
                                );
                                return;
                            }
                            for entry in &entries {
                                let (rect, response) = ui.allocate_exact_size(
                                    Vec2::new(ui.available_width(), ROW_H),
                                    Sense::click(),
                                );
                                if !ui.is_rect_visible(rect) {
                                    continue;
                                }
                                let hovered = response.hovered();
                                let selected = !entry.is_dir
                                    && studio
                                        .sample_editor
                                        .as_ref()
                                        .is_some_and(|editor| editor.path == entry.path);
                                let fill = if selected {
                                    theme::accent_alpha(theme::CYAN, 42)
                                } else if hovered {
                                    theme::PANEL_RAISED
                                } else {
                                    theme::PANEL_SUNK
                                };
                                ui.painter().rect_filled(rect, CornerRadius::same(1), fill);
                                if selected {
                                    ui.painter().rect_filled(
                                        Rect::from_min_size(
                                            rect.min,
                                            Vec2::new(2.0, rect.height()),
                                        ),
                                        CornerRadius::same(1),
                                        theme::CYAN,
                                    );
                                }
                                let prefix = if entry.is_dir { "> " } else { "  " };
                                let color = if entry.is_dir {
                                    theme::AMBER
                                } else {
                                    theme::TEXT
                                };
                                let name_painter = ui
                                    .painter()
                                    .with_clip_rect(rect.shrink2(Vec2::new(6.0, 0.0)));
                                name_painter.text(
                                    Pos2::new(rect.left() + 5.0, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    format!("{prefix}{}", entry.name),
                                    theme::mono(7.4),
                                    color,
                                );
                                if !entry.is_dir {
                                    let extension = entry
                                        .path
                                        .extension()
                                        .map(|value| value.to_string_lossy().to_ascii_uppercase())
                                        .unwrap_or_default();
                                    ui.painter().text(
                                        Pos2::new(rect.right() - 5.0, rect.center().y),
                                        Align2::RIGHT_CENTER,
                                        extension,
                                        theme::mono(5.8),
                                        theme::accent_alpha(theme::CYAN, 150),
                                    );
                                }
                                let clicked = response.clicked();
                                if hovered {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    response.on_hover_text(if entry.is_dir {
                                        format!("Open {}", entry.path.display())
                                    } else {
                                        format!("Edit {}", entry.path.display())
                                    });
                                }
                                if clicked {
                                    if entry.is_dir {
                                        new_dir = Some(entry.path.clone());
                                    } else {
                                        open_file = Some(entry.path.clone());
                                    }
                                }
                            }
                        });

                    ui.add_space(2.0);
                    ui.label(
                        RichText::new("CLICK = EDIT · DRAG FROM THE EDITOR INTO A CELL")
                            .font(theme::mono(5.8))
                            .color(theme::TEXT_FAINT),
                    );

                    if let Some(dir) = new_dir {
                        studio.set_sample_dir(dir);
                    }
                    if let Some(path) = open_file {
                        studio.open_sample_editor_path(&path);
                    }
                });
        });
}

fn compact_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).font(theme::mono(6.6)))
            .fill(theme::PANEL_SUNK)
            .min_size(Vec2::new(0.0, 17.0)),
    )
}

/// The sample editor: waveform, trim handles, destructive-free edits and the
/// drag handle that assigns the rendered sample to a clip cell.
pub fn draw_editor(ctx: &egui::Context, studio: &mut Studio, audio: &mut AudioRuntime) {
    if !studio.sample_editor_open {
        return;
    }
    let Some(title_sample) = studio.sample_editor.as_ref() else {
        studio.sample_editor_open = false;
        return;
    };
    let short_name: String = title_sample.name.chars().take(30).collect();
    let title = format!("SAMPLE EDITOR // {short_name}");
    // Sit in the right part of the window, leaving the matrix on the left.
    // On narrow screens shrink the editor instead of covering the grid.
    let screen = ctx.content_rect();
    let window_w = (screen.width() * 0.42).clamp(500.0, 720.0);
    let window_x = (screen.right() - window_w - 8.0).max(8.0);
    let slider_w = (window_w - 150.0).clamp(180.0, 460.0);
    let mut open = true;
    let mut preview = false;
    let mut stop = false;
    let mut trim_drag = studio.sample_trim_drag;
    let preview_started = studio.preview_started;

    {
        let Some(editor) = studio.sample_editor.as_mut() else {
            return;
        };
        // Re-render after the debounce window; the waveform itself is drawn
        // from the untouched source, so dragging never waits on this.
        editor.refresh(audio.sample_rate, false);

        egui::Window::new(title)
            .open(&mut open)
            .frame(
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(CornerRadius::same(3))
                    .inner_margin(8),
            )
            // Keep the clip matrix visible on the left: this is where the
            // sample will be dragged.
            .default_pos(Pos2::new(window_x, 124.0))
            .default_size(Vec2::new(window_w, 580.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.set_width(window_w - 20.0);
                ui.set_max_width(window_w - 20.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{}  ·  {:.2} s  ·  {} Hz  ·  {}",
                            editor.name,
                            editor.source_duration_secs(),
                            editor.source_rate,
                            editor
                                .path
                                .extension()
                                .map(|value| value.to_string_lossy().to_ascii_uppercase())
                                .unwrap_or_default()
                        ))
                        .font(theme::mono(7.4))
                        .color(theme::TEXT_DIM),
                    );
                    ui.separator();
                    ui.label(
                        RichText::new(format!("{:.2} s EDITED", editor.edited_duration_secs()))
                            .font(theme::mono(6.8))
                            .color(theme::AMBER),
                    );
                });
                ui.add_space(4.0);

                draw_waveform(
                    ui,
                    editor,
                    preview_started,
                    &mut trim_drag,
                    window_w - 36.0,
                );

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("PLAY").font(theme::display(9.0)),
                            )
                            .fill(theme::accent_alpha(theme::GREEN, 35)),
                        )
                        .clicked()
                    {
                        preview = true;
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("STOP").font(theme::display(8.0)),
                            )
                            .fill(theme::PANEL_SUNK),
                        )
                        .clicked()
                    {
                        stop = true;
                    }
                    ui.separator();

                    // The payload is only produced once the edit has been
                    // rendered, so a drop always carries what the user hears.
                    if editor.is_dirty() {
                        ui.label(
                            RichText::new("RENDERING EDITS...")
                                .font(theme::mono(7.0))
                                .color(theme::AMBER),
                        );
                    } else {
                        let clip = editor.to_clip(audio.sample_rate);
                        ui.dnd_drag_source(
                            egui::Id::unique("sample_editor_drag"),
                            clip,
                            |ui| {
                                ui.add(
                                    egui::Button::new(
                                        RichText::new("DRAG TO A CLIP CELL  ->")
                                            .font(theme::display(8.5)),
                                    )
                                    .fill(theme::accent_alpha(theme::MAGENTA, 40))
                                    .stroke(Stroke::new(
                                        1.0,
                                        theme::accent_alpha(theme::MAGENTA, 180),
                                    )),
                                )
                            },
                        )
                        .response
                        .on_hover_text(
                            "Drag this into any cell of the clip matrix (session view)",
                        );
                    }
                    ui.separator();
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("RESET EDITS").font(theme::display(7.5)),
                            )
                            .fill(theme::PANEL_SUNK),
                        )
                        .clicked()
                    {
                        editor.edits = crate::samples::SampleEdits::default();
                        editor.mark_dirty();
                        trim_drag = None;
                    }
                });

                ui.add_space(6.0);
                let mut changed = false;
                ui.spacing_mut().slider_width = slider_w;
                ui.spacing_mut().interact_size.y = 18.0;
                let source_duration = editor.source_duration_secs();
                let edited_duration = editor.edited_duration_secs();

                changed |= ui
                    .add(
                        egui::Slider::new(&mut editor.edits.gain_db, -24.0..=24.0)
                            .text("GAIN dB"),
                    )
                    .changed();
                ui.horizontal(|ui| {
                    changed |= ui
                        .checkbox(&mut editor.edits.normalize, "NORMALIZE")
                        .changed();
                    changed |= ui
                        .checkbox(&mut editor.edits.reverse, "REVERSE")
                        .changed();
                });

                let max_trim = (source_duration - 0.01).max(0.0);
                let max_start = (source_duration - editor.edits.trim_end - 0.01).max(0.0);
                let max_end = (source_duration - editor.edits.trim_start - 0.01).max(0.0);
                changed |= ui
                    .add(
                        egui::Slider::new(&mut editor.edits.trim_start, 0.0..=max_trim)
                            .text("TRIM START s"),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(&mut editor.edits.trim_end, 0.0..=max_trim)
                            .text("TRIM END s"),
                    )
                    .changed();
                editor.edits.trim_start = editor.edits.trim_start.clamp(0.0, max_start.max(0.0));
                editor.edits.trim_end = editor.edits.trim_end.clamp(0.0, max_end.max(0.0));

                let max_fade = (edited_duration * 0.5).max(0.0);
                changed |= ui
                    .add(
                        egui::Slider::new(&mut editor.edits.fade_in, 0.0..=max_fade)
                            .text("FADE IN s"),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(&mut editor.edits.fade_out, 0.0..=max_fade)
                            .text("FADE OUT s"),
                    )
                    .changed();

                if changed {
                    editor.mark_dirty();
                }
                ui.add_space(2.0);
                ui.label(
                    RichText::new(
                        "Trim, gain, fades and reverse are rendered before the sample reaches the engine.",
                    )
                    .font(theme::mono(6.4))
                    .color(theme::TEXT_FAINT),
                );
            });
    }

    studio.sample_trim_drag = trim_drag;
    if preview {
        studio.preview_sample(audio);
    }
    if stop {
        studio.stop_preview(audio);
    }
    if !open {
        studio.sample_editor_open = false;
        studio.stop_preview(audio);
    }
}

fn draw_waveform(
    ui: &mut egui::Ui,
    editor: &mut DecodedSample,
    preview_started: Option<std::time::Instant>,
    trim_drag: &mut Option<TrimEdge>,
    max_width: f32,
) {
    let height = 200.0;
    // Never take more than the window's fixed content width: an unbounded
    // `available_width` here is what made the window grow over the matrix.
    let width = ui.available_width().min(max_width);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if rect.width() < 20.0 {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(2), theme::BG_DEEP);
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, theme::BORDER_SOFT),
        egui::StrokeKind::Inside,
    );

    // Source waveform: one vertical span per pixel.
    let mid = rect.center().y;
    let amp = rect.height() * 0.44;
    let width = rect.width().floor().max(1.0) as usize;
    let buckets = editor.peaks.len().max(1);
    if !editor.peaks.is_empty() {
        for x in 0..width {
            let bucket = (x * buckets / width).min(buckets - 1);
            let value = editor.peaks[bucket].clamp(0.0, 1.0);
            let px = rect.left() + x as f32;
            painter.line_segment(
                [
                    Pos2::new(px, mid - value * amp),
                    Pos2::new(px, mid + value * amp),
                ],
                Stroke::new(1.0, theme::accent_alpha(theme::CYAN, 190)),
            );
        }
    }
    painter.line_segment(
        [Pos2::new(rect.left(), mid), Pos2::new(rect.right(), mid)],
        Stroke::new(0.5, theme::accent_alpha(theme::BORDER, 160)),
    );

    let duration = editor.source_duration_secs().max(0.001);
    let x_of = |seconds: f32| rect.left() + (seconds / duration).clamp(0.0, 1.0) * rect.width();
    let start_x = x_of(editor.edits.trim_start);
    let end_x = x_of((duration - editor.edits.trim_end).max(0.0)).max(start_x + 1.0);

    // Shade the trimmed-away regions.
    for shaded in [
        Rect::from_min_max(rect.min, Pos2::new(start_x, rect.bottom())),
        Rect::from_min_max(Pos2::new(end_x, rect.top()), rect.max),
    ] {
        if shaded.width() > 0.0 {
            painter.rect_filled(
                shaded,
                CornerRadius::ZERO,
                theme::accent_alpha(theme::BG_DEEP, 190),
            );
        }
    }

    // Loop-style trim handles.
    for (edge, x) in [(TrimEdge::Start, start_x), (TrimEdge::End, end_x)] {
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.4, theme::AMBER),
        );
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(x - 2.0, rect.top() + 2.0), Vec2::new(4.0, 12.0)),
            CornerRadius::same(1),
            theme::AMBER,
        );
        let hit = Rect::from_min_max(
            Pos2::new(x - 5.0, rect.top()),
            Pos2::new(x + 5.0, rect.bottom()),
        );
        let response = ui.interact(
            hit,
            ui.make_persistent_id(match edge {
                TrimEdge::Start => "sample_trim_start",
                TrimEdge::End => "sample_trim_end",
            }),
            Sense::click_and_drag(),
        );
        let hovered = response.hovered();
        let dragged = response.dragged();
        let drag_stopped = response.drag_stopped();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            response.on_hover_text("Drag to trim");
        }
        if dragged {
            if let Some(pointer) = ui.ctx().pointer_interact_pos() {
                let fraction = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                let seconds = fraction * duration;
                match edge {
                    TrimEdge::Start => {
                        let max = (duration - editor.edits.trim_end - 0.01).max(0.0);
                        editor.edits.trim_start = seconds.clamp(0.0, max);
                    }
                    TrimEdge::End => {
                        let min = (editor.edits.trim_start + 0.01).min(duration);
                        editor.edits.trim_end = (duration - seconds.clamp(min, duration)).max(0.0);
                    }
                }
                editor.mark_dirty();
                *trim_drag = Some(edge);
            }
        }
        if drag_stopped {
            *trim_drag = None;
        }
    }
    if trim_drag.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    // Preview cursor, if the one-shot is still running.
    let edited = editor.edited_duration_secs();
    if let Some(started) = preview_started {
        let elapsed = started.elapsed().as_secs_f32();
        if edited > 0.0 && elapsed < edited {
            let px = start_x + (elapsed / edited) * (end_x - start_x);
            painter.line_segment(
                [Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())],
                Stroke::new(1.4, theme::CYAN),
            );
        }
    }

    painter.text(
        Pos2::new(rect.left() + 5.0, rect.top() + 5.0),
        Align2::LEFT_TOP,
        "SOURCE WAVEFORM · DRAG THE AMBER HANDLES TO TRIM",
        theme::display(6.4),
        theme::accent_alpha(theme::CYAN, 190),
    );
    painter.text(
        Pos2::new(rect.right() - 5.0, rect.top() + 5.0),
        Align2::RIGHT_TOP,
        format!(
            "TRIM {:.2} – {:.2} s",
            editor.edits.trim_start,
            (duration - editor.edits.trim_end).max(0.0)
        ),
        theme::mono(6.4),
        theme::AMBER,
    );
}
