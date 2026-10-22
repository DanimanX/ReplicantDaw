//! Arrangement lanes and the 3D spectral waterfall.

use egui::{Align2, Color32, CornerRadius, Mesh, Pos2, Rect, Shape, Stroke, Ui, Vec2};

use crate::audio::{TRACKS, TRACK_COUNT};
use crate::dsp::spectrum::{BINS, HISTORY};
use crate::state::UiState;
use crate::theme;

pub const TIMELINE_BARS: usize = 16;

pub fn draw_lanes(ui: &mut Ui, rect: Rect, state: &mut UiState) {
    let content = theme::paint_panel(
        ui.painter(),
        rect,
        "ARRANGEMENT // NEURAL TRACKS",
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

    for (track, spec) in TRACKS.iter().enumerate() {
        let accent = theme::track_accent(track);
        let lane = Rect::from_min_size(
            Pos2::new(lane_area.left(), lane_area.top() + track as f32 * lane_h),
            Vec2::new(lane_area.width(), lane_h),
        );
        let gutter_rect = Rect::from_min_size(
            Pos2::new(content.left(), lane.top()),
            Vec2::new(gutter, lane_h),
        );

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
            spec.name,
            theme::display(7.0),
            if state.selected_track == track {
                accent
            } else {
                theme::TEXT_DIM
            },
        );
        let launched = state.launched[track];
        let scene = launched.unwrap_or(0) as usize;
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

        // Audio clip blocks, one per four bars.
        let block_bars = 4usize;
        let blocks = if launched.is_some() {
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
