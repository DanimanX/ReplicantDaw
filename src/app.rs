//! The eframe application: owns the audio runtime and paints the session.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Pos2, Rect, Vec2};

use rtrb::{Consumer, RingBuffer};

use crate::audio::{AudioRuntime, Command, EngineIo, MidiRuntime, RecordEvent, TRACK_COUNT};
use crate::plugins::{PluginCommand, PluginRuntime};
use crate::state::UiState;
use crate::studio::Studio;
use crate::theme;
use crate::ui;

pub struct ReplicantApp {
    audio: AudioRuntime,
    state: UiState,
    studio: Studio,
    record_rx: Consumer<RecordEvent>,
    last_input_track: usize,
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

        // Rings are created before the engine so MIDI, recording and plugin
        // audio all flow into the same audio callback instance.
        let mut midi = MidiRuntime::new();
        let midi_rx = midi.take_consumer();
        let (mut plugins, plugin_in, plugin_out) = PluginRuntime::start(48_000.0);
        let (record_tx, record_rx) = RingBuffer::new(1024);
        let io = EngineIo {
            midi: midi_rx,
            record_events: Some(record_tx),
            plugin_input: Some(plugin_in),
            plugin_output: Some(plugin_out),
            ..EngineIo::default()
        };
        let mut audio = AudioRuntime::start_with_io(io);
        plugins.send(PluginCommand::SetSampleRate {
            sample_rate: audio.sample_rate as f32,
        });
        let mut studio = Studio::new(midi, plugins);

        let mut state = UiState::new();
        state.sample_rate = audio.sample_rate;
        state.analyzer.set_sample_rate(audio.sample_rate as f32);
        studio.refresh_project_list();

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

        // Optional demo hooks used by the headless screenshot harness.
        if let Ok(path) = std::env::var("REPLICANT_DAW_OPEN_PLUGIN") {
            let format = if path.to_lowercase().ends_with(".clap") {
                crate::plugins::PluginFormat::Clap
            } else {
                crate::plugins::PluginFormat::Vst3
            };
            let name = std::path::Path::new(&path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| "Plugin".to_string());
            let descriptor = crate::plugins::PluginDescriptor {
                name,
                path: std::path::PathBuf::from(path),
                format,
            };
            studio.load_plugin(0, &descriptor);
        } else if let Ok(slot) = std::env::var("REPLICANT_DAW_OPEN_SLOT") {
            if let Ok(index) = slot.parse::<usize>() {
                studio.selected_slot = index.min(crate::plugins::MAX_PLUGIN_SLOTS - 1);
            }
        }
        if let Ok(windows) = std::env::var("REPLICANT_DAW_SMOKE_WINDOWS") {
            for window in windows.split(',') {
                match window.trim() {
                    "rack" => studio.rack_open = true,
                    "midi" => studio.midi_open = true,
                    "project" => studio.project_open = true,
                    "browser" => studio.browser_open = true,
                    _ => {}
                }
            }
        }

        if let Ok(path) = std::env::var("REPLICANT_DAW_PROJECT") {
            match studio.load_project(std::path::Path::new(&path), &mut state, &mut audio) {
                Ok(()) => log::info!("Loaded project {path}"),
                Err(error) => log::warn!("Could not load project {path}: {error}"),
            }
        }

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
            studio,
            record_rx,
            last_input_track: 0,
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
        self.studio.pump();

        while let Ok(event) = self.record_rx.pop() {
            self.state.apply_record_event(event);
        }

        if self.state.selected_track != self.last_input_track {
            self.last_input_track = self.state.selected_track;
            let track = self.state.selected_track as u8;
            self.push(Command::SetInputTrack { track });
        }

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
        let (space, stop, save, open_project, plugins) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::Escape),
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.modifiers.command && i.key_pressed(egui::Key::P),
            )
        });
        if save {
            match self.studio.save_project(&self.state) {
                Ok(path) => self.studio.notify(format!("Saved {}", path.display())),
                Err(error) => self.studio.notify(error),
            }
        }
        if open_project {
            self.studio.project_open = true;
            self.studio.refresh_project_list();
        }
        if plugins {
            self.studio.browser_open = true;
        }
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

        let menu_h = 24.0;
        let top_h = 60.0;
        let bottom_h = (inner.height() * 0.245).clamp(168.0, 252.0);
        let menu_rect = Rect::from_min_size(inner.min, Vec2::new(inner.width(), menu_h));
        let top_rect = Rect::from_min_size(
            Pos2::new(inner.left(), inner.top() + menu_h + 5.0),
            Vec2::new(inner.width(), top_h),
        );
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

        ui::studio_windows::draw_menu_bar(ui, menu_rect, &mut self.studio, &mut self.state);
        ui::transport::draw(ui, top_rect, &mut self.state, &mut self.audio);
        ui::session::draw_matrix(ui, matrix_rect, &mut self.state, &mut self.audio);
        ui::session::draw_mixer(ui, mixer_rect, &mut self.state, &mut self.audio);
        ui::lanes::draw_lanes(ui, tracks_rect, &mut self.state);
        ui::lanes::draw_fft(ui, fft_rect, &mut self.state);
        ui::devices::draw(ui, bottom_rect, &mut self.state, &mut self.audio);
        ui::studio_windows::draw_windows(
            ui.ctx(),
            &mut self.studio,
            &mut self.state,
            &mut self.audio,
        );
    }

    fn maybe_screenshot(&mut self, ctx: &egui::Context) {
        let Some(plan) = &self.smoke else {
            return;
        };
        if self.screenshot_requested || self.started.elapsed() < plan.delay {
            return;
        }
        self.screenshot_requested = true;

        if let Ok(save_path) = std::env::var("REPLICANT_DAW_SMOKE_SAVE") {
            match self
                .studio
                .save_project_as(&self.state, std::path::PathBuf::from(&save_path))
            {
                Ok(path) => println!("SMOKE_PROJECT_SAVED {}", path.display()),
                Err(error) => eprintln!("SMOKE_PROJECT_FAILED {error}"),
            }
        }

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
