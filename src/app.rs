//! The eframe application: owns the audio runtime and paints the session.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Pos2, Rect, Vec2};

use crate::audio::{AudioRuntime, Command, TRACK_COUNT};
use crate::state::UiState;
use crate::theme;
use crate::ui;

pub struct ReplicantApp {
    audio: AudioRuntime,
    state: UiState,
    last_update: Instant,
    started: Instant,
    frames: u64,
    smoke: Option<SmokePlan>,
    screenshot_requested: bool,
}

struct SmokePlan {
    path: PathBuf,
    delay: Duration,
}

impl ReplicantApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let mut audio = AudioRuntime::start();
        let mut state = UiState::new();
        state.sample_rate = audio.sample_rate;
        state.analyzer.set_sample_rate(audio.sample_rate as f32);

        let smoke = std::env::var("REPLICANT_DAW_SMOKE")
            .ok()
            .filter(|path| !path.trim().is_empty())
            .map(|path| {
                let delay = std::env::var("REPLICANT_DAW_SMOKE_SECS")
                    .ok()
                    .and_then(|value| value.parse::<f32>().ok())
                    .unwrap_or(3.0)
                    .clamp(0.5, 30.0);
                SmokePlan {
                    path: PathBuf::from(path),
                    delay: Duration::from_secs_f32(delay),
                }
            });

        if smoke.is_some() {
            // A deterministic, lively frame for documentation and CI.
            let _ = audio.command_tx.push(Command::SetBpm(142.0));
            let _ = audio.command_tx.push(Command::Play);
            state.playing = true;
            for track in 0..TRACK_COUNT {
                let _ = audio.command_tx.push(Command::LaunchClip {
                    track: track as u8,
                    scene: crate::audio::DEFAULT_SCENES[track] as u8,
                });
            }
        }

        log::info!(
            "Replicant DAW {} | backend={} | device={} | {} Hz",
            crate::APP_VERSION,
            audio.backend,
            audio.device_name,
            audio.sample_rate
        );

        Self {
            audio,
            state,
            last_update: Instant::now(),
            started: Instant::now(),
            frames: 0,
            smoke,
            screenshot_requested: false,
        }
    }

    fn push(&mut self, command: Command) {
        let _ = self.audio.command_tx.push(command);
    }

    fn pump_audio(&mut self, dt: f32) {
        while let Ok(telemetry) = self.audio.telemetry_rx.pop() {
            if telemetry.sample_rate != self.state.sample_rate {
                self.state
                    .analyzer
                    .set_sample_rate(telemetry.sample_rate as f32);
            }
            self.state.ingest(telemetry, dt);
        }
        self.state.drain_scope(&mut self.audio.scope_rx);
        if self.frames % 2 == 0 {
            self.state.analyse();
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (space, stop) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if space {
            if self.state.playing {
                self.push(Command::Stop);
                self.state.playing = false;
                self.state.position_frames = 0;
            } else {
                self.push(Command::Play);
                self.state.playing = true;
            }
        }
        if stop {
            self.push(Command::Stop);
            self.state.playing = false;
            self.state.position_frames = 0;
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::ZERO, theme::BG);
        theme::paint_backdrop(ui.painter(), rect);

        let pad = 8.0;
        let gap = 8.0;
        let inner = rect.shrink(pad);
        if inner.width() < 200.0 || inner.height() < 200.0 {
            return;
        }

        let top_h = 60.0;
        let bottom_h = (inner.height() * 0.245).clamp(168.0, 252.0);
        let top_rect = Rect::from_min_size(inner.min, Vec2::new(inner.width(), top_h));
        let bottom_rect = Rect::from_min_max(
            Pos2::new(inner.left(), inner.bottom() - bottom_h),
            inner.max,
        );
        let upper = Rect::from_min_max(
            Pos2::new(inner.left(), top_rect.bottom() + gap),
            Pos2::new(inner.right(), bottom_rect.top() - gap),
        );

        let left_w = (upper.width() * 0.455).clamp(470.0, 920.0);
        let left = Rect::from_min_size(upper.min, Vec2::new(left_w, upper.height()));
        let right = Rect::from_min_max(Pos2::new(left.right() + gap, upper.top()), upper.max);

        let matrix_h = (left.height() * 0.56).clamp(210.0, 460.0);
        let matrix_rect = Rect::from_min_size(left.min, Vec2::new(left.width(), matrix_h));
        let mixer_rect =
            Rect::from_min_max(Pos2::new(left.left(), matrix_rect.bottom() + gap), left.max);

        let tracks_h = (right.height() * 0.50).clamp(170.0, 430.0);
        let tracks_rect = Rect::from_min_size(right.min, Vec2::new(right.width(), tracks_h));
        let fft_rect = Rect::from_min_max(
            Pos2::new(right.left(), tracks_rect.bottom() + gap),
            right.max,
        );

        ui::transport::draw(ui, top_rect, &mut self.state, &mut self.audio);
        ui::session::draw_matrix(ui, matrix_rect, &mut self.state, &mut self.audio);
        ui::session::draw_mixer(ui, mixer_rect, &mut self.state, &mut self.audio);
        ui::lanes::draw_lanes(ui, tracks_rect, &mut self.state);
        ui::lanes::draw_fft(ui, fft_rect, &mut self.state);
        ui::devices::draw(ui, bottom_rect, &mut self.state, &mut self.audio);
    }

    fn maybe_screenshot(&mut self, ctx: &egui::Context) {
        let Some(plan) = &self.smoke else {
            return;
        };
        if self.screenshot_requested || self.started.elapsed() < plan.delay {
            return;
        }
        self.screenshot_requested = true;

        let path = plan.path.clone();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let ui_ctx = ctx.clone();
        ctx.request_screenshot(move |image| {
            match save_color_image(&image, &path) {
                Ok(()) => {
                    println!("SMOKE_SCREENSHOT_SAVED {}", path.display());
                    log::info!("Saved smoke screenshot to {}", path.display());
                }
                Err(error) => {
                    eprintln!("SMOKE_SCREENSHOT_FAILED {error}");
                    std::process::exit(2);
                }
            }
            ui_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            ui_ctx.request_repaint();
        });
        ctx.request_repaint();
    }
}

impl eframe::App for ReplicantApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = (now - self.last_update).as_secs_f32().clamp(0.0, 0.25);
        self.last_update = now;
        self.frames = self.frames.wrapping_add(1);

        self.pump_audio(dt);
        self.handle_shortcuts(ui.ctx());
        self.draw(ui);
        self.maybe_screenshot(ui.ctx());

        // Keep meters, playhead and the waterfall alive.
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }

    fn on_exit(&mut self) {
        log::info!("Replicant DAW shutting down");
    }
}

/// Encode an egui screenshot to PNG.
pub fn save_color_image(image: &egui::ColorImage, path: &std::path::Path) -> Result<(), String> {
    let [width, height] = image.size;
    let mut rgba = Vec::with_capacity(width * height * 4);
    for pixel in &image.pixels {
        rgba.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
    }
    image::save_buffer(
        path,
        &rgba,
        width as u32,
        height as u32,
        image::ColorType::Rgba8,
    )
    .map_err(|error| format!("could not write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;

    #[test]
    fn screenshot_round_trip() {
        let image = egui::ColorImage::filled([4, 3], Color32::from_rgb(7, 11, 18));
        let path = std::env::temp_dir().join("replicant-daw-test.png");
        save_color_image(&image, &path).expect("save image");
        let loaded = image::open(&path).expect("open image");
        assert_eq!((loaded.width(), loaded.height()), (4, 3));
        let _ = std::fs::remove_file(path);
    }
}
