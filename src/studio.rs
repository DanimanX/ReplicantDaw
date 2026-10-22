//! Studio services shared by the UI: MIDI input, plugin hosting and projects.
//!
//! The audio engine only sees lock-free rings. Everything that can allocate,
//! block or load third-party code lives here, on the UI or plugin-worker
//! threads.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;

use crate::audio::messages::{Command, RecordedClip, RecordedNote, TRACK_COUNT};
use crate::audio::{db_to_gain, AudioRuntime, MidiMessage, MidiRuntime};
use crate::plugins::{
    scan_all, ParamInfo, PluginCommand, PluginDescriptor, PluginEvent, PluginRuntime,
    PluginSlotInfo, MAX_PLUGIN_SLOTS,
};
use crate::project::{Project, ProjectPlugin};
use crate::samples::{list_directory, DecodedSample, SampleClip, SampleEntry, SampleSlot};
use crate::state::UiState;

pub const MAX_STATUS_CHARS: usize = 160;

/// Pointer gesture in the piano roll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollDragMode {
    Move,
    Resize,
}

/// Which edge of the clip loop is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopEdge {
    /// The start handle.
    Start,
    /// The end handle.
    End,
    /// Empty ruler: build a new region from this anchor step.
    New(u16),
}

/// Which edge of a sample's trim region is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimEdge {
    Start,
    End,
}

#[derive(Clone, Copy, Debug)]
pub struct RollDrag {
    pub note_index: usize,
    pub original_start: u16,
    pub original_note: u8,
    pub original_length: u16,
    pub mode: RollDragMode,
    /// Pointer position when the drag began. Absolute, because egui's
    /// `drag_delta` is per-frame and cannot drive an absolute edit.
    pub pointer_origin: egui::Pos2,
}

pub struct Studio {
    pub midi: MidiRuntime,
    pub plugins: PluginRuntime,
    pub slots: [Option<PluginSlotInfo>; MAX_PLUGIN_SLOTS],
    pub slot_pending: [bool; MAX_PLUGIN_SLOTS],
    /// Slots that had a plugin on the previous frame, used to notice loads.
    slot_loaded: [bool; MAX_PLUGIN_SLOTS],
    pub plugin_sent: [bool; MAX_PLUGIN_SLOTS],
    pub selected_slot: usize,

    pub scan_results: Vec<PluginDescriptor>,
    pub scan_running: bool,
    scan_rx: Option<Receiver<Vec<PluginDescriptor>>>,
    pub browser_open: bool,
    /// True while the picker is choosing an instrument for the device chain.
    pub instrument_browser: bool,
    pub browser_target: usize,
    pub browser_filter: String,
    pub rack_open: bool,
    pub midi_open: bool,
    pub project_open: bool,
    pub synth_open: bool,
    pub zoom: f32,

    /// Piano-roll editor. `piano_roll_scene` is informational: clips are
    /// stored per track in the audio engine.
    pub piano_roll_open: bool,
    pub piano_roll_track: usize,
    pub piano_roll_scene: u8,
    pub roll_base_note: i32,
    pub roll_selected: Option<usize>,
    pub roll_drag: Option<RollDrag>,
    /// Which edge of the clip loop the user is dragging, if any.
    pub roll_loop_drag: Option<LoopEdge>,

    /// Audio-clip envelope editor.
    pub audio_editor_open: bool,
    pub audio_editor_track: usize,

    /// Sample browser drawer and sample editor.
    pub sample_browser_open: bool,
    pub sample_dir: PathBuf,
    pub sample_entries: Vec<SampleEntry>,
    pub sample_filter: String,
    pub sample_editor_open: bool,
    pub sample_editor: Option<DecodedSample>,
    /// Clip-level sample editor: loop region, slot trim and launch.
    pub sample_clip_open: bool,
    pub sample_clip_target: (usize, usize),
    /// Which trim handle the user is dragging in the sample editor.
    pub sample_trim_drag: Option<TrimEdge>,
    /// When the current one-shot preview began, for drawing its cursor.
    pub preview_started: Option<Instant>,

    /// Full plugin parameter editor (opens maximized, like a plugin window).
    pub plugin_editor_open: bool,
    pub plugin_editor_track: usize,
    pub plugin_editor_fullscreen: bool,
    pub plugin_editor_filter: String,

    /// Inline track rename prompt.
    pub rename_track: Option<usize>,
    pub rename_buffer: String,
    pub rename_focus: bool,

    pub project_name: String,
    pub project_path: Option<PathBuf>,
    pub projects: Vec<PathBuf>,
    pub status: String,
    pub logs: Vec<String>,

    pub virtual_keys: [bool; 24],
    pub next_request: u64,
    /// Last time plugin parameters were re-read from the plugins.
    last_param_refresh: Instant,
    /// The DAW's own native window, used as the plugin editor's parent.
    host_window: Option<u64>,
}

impl Studio {
    pub fn new(midi: MidiRuntime, plugins: PluginRuntime) -> Self {
        let mut studio = Self {
            midi,
            plugins,
            slots: std::array::from_fn(|_| None),
            slot_pending: [false; MAX_PLUGIN_SLOTS],
            slot_loaded: [false; MAX_PLUGIN_SLOTS],
            plugin_sent: [false; MAX_PLUGIN_SLOTS],
            selected_slot: 0,
            scan_results: Vec::new(),
            scan_running: false,
            scan_rx: None,
            browser_open: false,
            instrument_browser: false,
            browser_target: 0,
            browser_filter: String::new(),
            rack_open: false,
            midi_open: false,
            project_open: false,
            synth_open: false,
            zoom: 1.0,
            piano_roll_open: false,
            piano_roll_track: 0,
            piano_roll_scene: 0,
            roll_base_note: 36,
            roll_selected: None,
            roll_drag: None,
            roll_loop_drag: None,
            audio_editor_open: false,
            audio_editor_track: 0,
            sample_browser_open: false,
            sample_dir: crate::samples::initial_directory(),
            sample_entries: Vec::new(),
            sample_filter: String::new(),
            sample_editor_open: false,
            sample_editor: None,
            sample_clip_open: false,
            sample_clip_target: (0, 0),
            sample_trim_drag: None,
            preview_started: None,
            plugin_editor_open: false,
            plugin_editor_track: 0,
            plugin_editor_fullscreen: false,
            plugin_editor_filter: String::new(),
            rename_track: None,
            rename_buffer: String::new(),
            rename_focus: false,
            project_name: "Untitled".to_string(),
            project_path: None,
            projects: Project::list_available(),
            status: String::new(),
            logs: Vec::new(),
            virtual_keys: [false; 24],
            next_request: 1,
            last_param_refresh: Instant::now(),
            host_window: None,
        };
        studio.status = format!(
            "{} MIDI input(s) · {} project(s)",
            studio.midi.ports.len(),
            studio.projects.len()
        );
        // Preload the plugins installed on this machine so the instrument
        // browser is populated the first time it opens.
        studio.start_scan();
        studio
    }

    pub fn notify(&mut self, message: impl Into<String>) {
        let message = message.into();
        let message: String = message.chars().take(MAX_STATUS_CHARS).collect();
        log::info!("{message}");
        self.status = message;
    }

    fn log_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        log::warn!("{message}");
        self.logs.push(message.clone());
        if self.logs.len() > 32 {
            self.logs.remove(0);
        }
        self.status = message.chars().take(MAX_STATUS_CHARS).collect();
    }

    /// Re-read the current sample folder.
    pub fn reload_sample_dir(&mut self) {
        match list_directory(&self.sample_dir) {
            Ok(entries) => {
                self.sample_entries = entries;
                self.sample_filter.clear();
            }
            Err(error) => {
                self.sample_entries.clear();
                self.log_error(error);
            }
        }
    }

    /// Move the browser to another folder and list it.
    pub fn set_sample_dir(&mut self, dir: PathBuf) {
        self.sample_dir = dir;
        self.reload_sample_dir();
    }

    /// Open a sample file in the editor window.
    pub fn open_sample_editor_path(&mut self, path: &std::path::Path) {
        match DecodedSample::load(path) {
            Ok(sample) => {
                self.notify(format!(
                    "Loaded {} · {:.2} s · {} Hz · {}",
                    sample.name,
                    sample.source_duration_secs(),
                    sample.source_rate,
                    path.display()
                ));
                self.sample_editor = Some(sample);
                self.sample_editor_open = true;
                self.preview_started = None;
                // The editor takes over the space; the browser stays one
                // click away behind the SAMPLES tab.
                self.sample_browser_open = false;
            }
            Err(error) => self.log_error(error),
        }
    }

    /// Reopen the editor for a sample that is already in a clip slot.
    pub fn open_sample_editor_clip(&mut self, clip: &SampleClip) {
        self.sample_editor = Some(DecodedSample::from_clip(clip));
        self.sample_editor_open = true;
        self.preview_started = None;
    }

    /// Put a rendered sample in a session slot and hand it to the engine.
    pub fn assign_sample(
        &mut self,
        state: &mut UiState,
        audio: &mut AudioRuntime,
        track: usize,
        scene: usize,
        clip: Arc<SampleClip>,
    ) {
        let name = clip.name.clone();
        let duration = clip.duration_secs();
        // A slot holds one kind of clip. Assigning audio replaces any MIDI
        // notes that were programmed there instead of layering both.
        if !state.recorded_notes(track, scene).is_empty() {
            state.clear_recorded(track, scene);
            let _ = audio.command_tx.push(Command::SetRecordedClip {
                track: track as u8,
                scene: scene as u8,
                clip: RecordedClip::default(),
            });
        }
        state.set_sample_slot(
            track,
            scene,
            Some(Arc::new(SampleSlot::new(Arc::clone(&clip)))),
        );
        let _ = audio.sample_tx.push(crate::audio::SampleUpload {
            track: track as u8,
            scene: scene as u8,
            sample: Some(Arc::clone(&clip.buffer)),
        });
        // The slot plays its own sample right away, on its own loop.
        let _ = audio.command_tx.push(Command::SetSampleLoop {
            track: track as u8,
            scene: scene as u8,
            enabled: true,
        });
        let _ = audio.command_tx.push(Command::LaunchClip {
            track: track as u8,
            scene: scene as u8,
        });
        state.launched[track] = Some(scene as u8);
        self.notify(format!(
            "{} → track {:02} · slot {:02} ({:.2} s)",
            name,
            track + 1,
            scene + 1,
            duration
        ));
    }

    /// Remove a sample from a clip slot and stop that slot's playback.
    pub fn clear_sample(
        &mut self,
        state: &mut UiState,
        audio: &mut AudioRuntime,
        track: usize,
        scene: usize,
    ) {
        if state.sample_clip(track, scene).is_none() {
            return;
        }
        state.set_sample_slot(track, scene, None);
        let _ = audio.sample_tx.push(crate::audio::SampleUpload {
            track: track as u8,
            scene: scene as u8,
            sample: None,
        });
        self.notify(format!(
            "Sample cleared from track {:02} · slot {:02}",
            track + 1,
            scene + 1
        ));
    }

    /// Open the clip editor for a sample slot: its loop, trim and launch.
    pub fn open_sample_clip(&mut self, track: usize, scene: usize) {
        self.sample_clip_target = (track, scene);
        self.sample_clip_open = true;
        // Both windows describe the same audio; stacking them hid the clip's
        // own controls. `EDIT SOURCE` reopens the sampler on demand.
        self.sample_editor_open = false;
        self.preview_started = None;
    }

    /// Toggle looping for one sample slot and tell the engine.
    pub fn set_sample_loop(
        &mut self,
        state: &mut UiState,
        audio: &mut AudioRuntime,
        track: usize,
        scene: usize,
        enabled: bool,
    ) {
        if !state.set_sample_loop(track, scene, enabled) {
            return;
        }
        let _ = audio.command_tx.push(Command::SetSampleLoop {
            track: track as u8,
            scene: scene as u8,
            enabled,
        });
        self.notify(format!(
            "Slot {:02} · track {:02} loop {}",
            scene + 1,
            track + 1,
            if enabled { "on" } else { "off" }
        ));
    }

    /// Change the per-slot trim of a sample.
    pub fn set_sample_gain(
        &mut self,
        state: &mut UiState,
        audio: &mut AudioRuntime,
        track: usize,
        scene: usize,
        gain_db: f32,
    ) {
        if !state.set_sample_gain(track, scene, gain_db) {
            return;
        }
        let _ = audio.command_tx.push(Command::SetSampleGain {
            track: track as u8,
            scene: scene as u8,
            gain_db,
        });
    }

    /// Audition the edited sample through the master bus.
    pub fn preview_sample(&mut self, audio: &mut AudioRuntime) {
        let Some(editor) = &mut self.sample_editor else {
            return;
        };
        let clip = editor.to_clip(audio.sample_rate);
        let _ = audio.sample_tx.push(crate::audio::SampleUpload {
            track: crate::audio::PREVIEW_TRACK,
            scene: 0,
            sample: Some(Arc::clone(&clip.buffer)),
        });
        self.preview_started = Some(Instant::now());
    }

    pub fn stop_preview(&mut self, audio: &mut AudioRuntime) {
        let _ = audio.command_tx.push(Command::StopPreview);
        self.preview_started = None;
    }

    /// Per-frame service pump.
    pub fn pump(&mut self) {
        self.midi.pump();

        // While a plugin's own window is open, it is the user editing the
        // parameters: keep the host's mirrored values fresh.
        if self.last_param_refresh.elapsed() >= Duration::from_millis(400)
            && self.slots.iter().flatten().any(|slot| slot.gui_open)
        {
            self.last_param_refresh = Instant::now();
            self.plugins.send(PluginCommand::RefreshParams);
        }

        for _ in 0..32 {
            let event = match self.plugins.event_rx.pop() {
                Ok(event) => event,
                Err(_) => break,
            };
            self.handle_plugin_event(event);
        }

        if let Some(rx) = &self.scan_rx {
            match rx.try_recv() {
                Ok(results) => {
                    let count = results.len();
                    self.scan_results = results;
                    self.scan_running = false;
                    self.scan_rx = None;
                    self.notify(format!("Plugin scan complete: {count} plugin(s) found"));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.scan_running = false;
                    self.scan_rx = None;
                }
            }
        }
    }

    fn handle_plugin_event(&mut self, event: PluginEvent) {
        match event {
            PluginEvent::Slots(infos) => {
                self.slots = std::array::from_fn(|_| None);
                self.slot_pending = [false; MAX_PLUGIN_SLOTS];
                for info in infos {
                    let slot = info.slot;
                    if slot < MAX_PLUGIN_SLOTS {
                        self.slots[slot] = Some(info);
                    }
                }
            }
            PluginEvent::State { .. } => {
                // Consumed by `collect_plugin_states` when saving.
            }
            PluginEvent::Gui {
                slot,
                open,
                message,
            } => {
                if let Some(info) = self.slots.get_mut(slot as usize).and_then(Option::as_mut) {
                    info.gui_open = open;
                }
                match message {
                    Some(message) => self.log_error(message),
                    None if open => self.notify("Plugin editor opened"),
                    None => self.notify("Plugin editor closed"),
                }
            }
            PluginEvent::Log(message) => self.log_error(message),
        }
    }

    /// Open the plugin picker for a track's instrument slot.
    ///
    /// The list is populated by the automatic scan that runs at startup, and
    /// rescanned immediately if nothing has been found yet.
    pub fn open_instrument_browser(&mut self, track: usize) {
        self.browser_target = track.min(TRACK_COUNT - 1);
        self.browser_open = true;
        self.instrument_browser = true;
        if self.scan_results.is_empty() && !self.scan_running {
            self.start_scan();
        }
    }

    /// Choose the internal synth as the instrument source for `track`.
    pub fn select_synth(&mut self, state: &mut UiState, track: usize) {
        if track < TRACK_COUNT {
            state.instrument[track] = crate::state::InstrumentSource::Synth;
        }
    }

    pub fn start_scan(&mut self) {
        if self.scan_running {
            return;
        }
        self.scan_running = true;
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        std::thread::Builder::new()
            .name("replicant-plugin-scan".into())
            .spawn(move || {
                let found = scan_all();
                let _ = tx.send(found);
            })
            .expect("spawn plugin scanner");
        self.notify("Scanning plugin directories…");
    }

    pub fn load_plugin(
        &mut self,
        slot: usize,
        descriptor: &PluginDescriptor,
        audio: &mut AudioRuntime,
    ) {
        if slot >= MAX_PLUGIN_SLOTS {
            return;
        }
        if !descriptor.hostable() {
            self.log_error(format!(
                "{} is a {} plugin. VST2 hosting needs the proprietary Steinberg SDK; use VST3 or CLAP.",
                descriptor.name, descriptor.format
            ));
            return;
        }
        self.slot_pending[slot] = true;
        self.selected_slot = slot;
        self.plugins.send(PluginCommand::Load {
            slot: slot as u8,
            descriptor: descriptor.clone(),
            state: None,
        });
        let _ = audio.command_tx.push(Command::SetTrackPlugin {
            track: slot as u8,
            active: true,
        });
        self.notify(format!(
            "Loading {} ({}) on track {:02}",
            descriptor.name,
            descriptor.format,
            slot + 1
        ));
    }

    pub fn unload_plugin(&mut self, slot: usize, audio: &mut AudioRuntime) {
        if slot >= MAX_PLUGIN_SLOTS {
            return;
        }
        self.plugins
            .send(PluginCommand::Unload { slot: slot as u8 });
        let _ = audio.command_tx.push(Command::SetTrackPlugin {
            track: slot as u8,
            active: false,
        });
        self.slots[slot] = None;
        self.slot_pending[slot] = false;
    }

    /// Remember the DAW's native window so plugin editors float above it.
    pub fn set_host_window(&mut self, window: Option<u64>) {
        self.host_window = window;
    }

    /// Ask the worker to open the plugin's own editor window.
    pub fn open_plugin_gui(&mut self, slot: usize) {
        if slot >= MAX_PLUGIN_SLOTS {
            return;
        }
        self.plugins.send(PluginCommand::OpenGui {
            slot: slot as u8,
            transient_for: self.host_window,
        });
        self.notify(format!(
            "Opening the plugin editor on track {:02}",
            slot + 1
        ));
    }

    pub fn close_plugin_gui(&mut self, slot: usize) {
        if slot >= MAX_PLUGIN_SLOTS {
            return;
        }
        self.plugins
            .send(PluginCommand::CloseGui { slot: slot as u8 });
    }

    pub fn set_bypass(&mut self, slot: usize, bypass: bool) {
        self.plugins.send(PluginCommand::Bypass {
            slot: slot as u8,
            bypass,
        });
        if let Some(info) = &mut self.slots[slot] {
            info.bypass = bypass;
        }
    }

    pub fn set_param(&mut self, slot: usize, param: &ParamInfo, value: f32) {
        self.plugins.send(PluginCommand::SetParam {
            slot: slot as u8,
            id: param.id,
            value,
        });
        if let Some(info) = &mut self.slots[slot] {
            if let Some(target) = info.params.iter_mut().find(|p| p.id == param.id) {
                target.value = value;
            }
        }
    }

    fn collect_plugin_states(&mut self) -> Vec<(u8, Vec<u8>)> {
        let request = self.next_request;
        self.next_request += 1;
        self.plugins.send(PluginCommand::SaveState { request });

        let deadline = Instant::now() + Duration::from_millis(900);
        while Instant::now() < deadline {
            while let Ok(event) = self.plugins.event_rx.pop() {
                match event {
                    PluginEvent::State { request: id, slots } if id == request => return slots,
                    other => self.handle_plugin_event(other),
                }
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        self.log_error("Plugin state request timed out; saving without plugin state");
        Vec::new()
    }

    pub fn save_project(&mut self, state: &UiState) -> Result<PathBuf, String> {
        let states = self.collect_plugin_states();
        let mut project = Project::from_session(&self.project_name, state.bpm, state);

        for (slot, info) in self.slots.iter().enumerate() {
            let Some(info) = info else {
                continue;
            };
            let encoded = states
                .iter()
                .find(|(index, _)| *index as usize == slot)
                .map(|(_, bytes)| base64::engine::general_purpose::STANDARD.encode(bytes))
                .unwrap_or_default();
            project.plugins.push(ProjectPlugin {
                slot: slot as u8,
                name: info.name.clone(),
                vendor: info.vendor.clone(),
                format: info.format.label().to_string(),
                path: info.path.clone(),
                bypass: info.bypass,
                state: encoded,
            });
        }

        let path = self
            .project_path
            .clone()
            .unwrap_or_else(|| Project::path_for_name(&self.project_name));
        project.save(&path)?;
        self.project_path = Some(path.clone());
        self.refresh_project_list();
        self.notify(format!("Saved {}", path.display()));
        Ok(path)
    }

    pub fn save_project_as(&mut self, state: &UiState, path: PathBuf) -> Result<PathBuf, String> {
        if self.project_name == "Untitled" {
            if let Some(stem) = path.file_stem() {
                self.project_name = stem.to_string_lossy().to_string();
            }
        }
        self.project_path = Some(path);
        self.save_project(state)
    }

    pub fn load_project(
        &mut self,
        path: &std::path::Path,
        state: &mut UiState,
        audio: &mut AudioRuntime,
    ) -> Result<(), String> {
        let project = Project::load(path)?;

        // Any editor from the previous project would point at stale audio.
        self.sample_editor_open = false;
        self.sample_editor = None;
        self.preview_started = None;

        // Drop any current plugins first so state restore starts clean.
        for slot in 0..MAX_PLUGIN_SLOTS {
            self.unload_plugin(slot, audio);
        }

        state.bpm = project.bpm;
        let _ = audio.command_tx.push(Command::SetBpm(project.bpm));

        // Rebuild the modular chain from scratch and apply the master gain.
        state.effects.clear();
        let _ = audio.command_tx.push(Command::ClearEffects);
        for effect in &project.effects {
            let Some(kind) = effect.effect_kind() else {
                self.log_error(format!("Unknown effect module '{}'", effect.kind));
                continue;
            };
            if state.add_effect(kind).is_some() {
                let index = state.effects.len() as u8 - 1;
                let _ = audio.command_tx.push(Command::AddEffect { kind });
                let mut effect_state = state.effects[index as usize];
                for knob in 0..3 {
                    effect_state.params[knob] = effect.params[knob];
                    let _ = audio.command_tx.push(Command::SetEffectParam {
                        index,
                        knob: knob as u8,
                        value: effect.params[knob],
                    });
                }
                effect_state.bypass = effect.bypass;
                state.effects[index as usize] = effect_state;
                if effect.bypass {
                    let _ = audio.command_tx.push(Command::SetEffectBypass {
                        index,
                        bypass: true,
                    });
                }
            }
        }
        state.master_gain = project.master_gain;
        let _ = audio.command_tx.push(Command::SetMasterGain {
            gain: project.master_gain,
        });

        // Drop samples from the previous project before loading this one.
        state.clear_samples();
        for track in 0..TRACK_COUNT {
            for scene in 0..crate::audio::SCENE_COUNT {
                let _ = audio.sample_tx.push(crate::audio::SampleUpload {
                    track: track as u8,
                    scene: scene as u8,
                    sample: None,
                });
            }
        }

        for (index, track) in project.tracks.iter().take(TRACK_COUNT).enumerate() {
            state.track_gain_db[index] = track.gain_db;
            state.track_pan[index] = track.pan;
            state.mute[index] = track.mute;
            state.solo[index] = track.solo;
            state.launched[index] = track.scene;

            let _ = audio.command_tx.push(Command::SetTrackGain {
                track: index as u8,
                gain: crate::audio::db_to_gain(track.gain_db),
            });
            let _ = audio.command_tx.push(Command::SetTrackPan {
                track: index as u8,
                pan: track.pan,
            });
            let _ = audio.command_tx.push(Command::SetTrackMute {
                track: index as u8,
                mute: track.mute,
            });
            let _ = audio.command_tx.push(Command::SetTrackSolo {
                track: index as u8,
                solo: track.solo,
            });

            match track.scene {
                Some(scene) => {
                    let _ = audio.command_tx.push(Command::LaunchClip {
                        track: index as u8,
                        scene,
                    });
                }
                None => {
                    let _ = audio
                        .command_tx
                        .push(Command::StopClip { track: index as u8 });
                }
            }

            state.rename_track(index, &track.name);
            for scene in 0..crate::audio::SCENE_COUNT {
                state.set_recorded(index, scene, Vec::new());
                state.recorded_active[index][scene] = true;
                let _ = audio.command_tx.push(Command::SetRecordedClip {
                    track: index as u8,
                    scene: scene as u8,
                    clip: RecordedClip::default(),
                });
            }
            for entry in &track.clips {
                let scene = (entry.scene as usize).min(crate::audio::SCENE_COUNT - 1);
                state.set_clip_loop(
                    index,
                    scene,
                    entry.loop_start.unwrap_or(0),
                    entry.loop_end.unwrap_or(crate::audio::LOOP_STEPS as u16),
                );
                sync_clip_loop(state, audio, index, scene);
                state.set_recorded(index, scene, entry.notes.clone());
                state.recorded_active[index][scene] = entry.enabled;
                let _ = audio.command_tx.push(Command::SetRecordedClip {
                    track: index as u8,
                    scene: scene as u8,
                    clip: clip_from_notes(&entry.notes),
                });
                let _ = audio.command_tx.push(Command::SetRecordedActive {
                    track: index as u8,
                    scene: scene as u8,
                    active: entry.enabled,
                });
            }

            for entry in &track.samples {
                let scene = (entry.scene as usize).min(crate::audio::SCENE_COUNT - 1);
                match DecodedSample::load(&entry.path) {
                    Ok(mut sample) => {
                        sample.edits = entry.edits();
                        let clip = sample.to_clip(audio.sample_rate);
                        // One clip kind per slot: a stale MIDI clip from a
                        // hand-edited project must not play under the sample.
                        if !state.recorded_notes(index, scene).is_empty() {
                            state.clear_recorded(index, scene);
                            let _ = audio.command_tx.push(Command::SetRecordedClip {
                                track: index as u8,
                                scene: scene as u8,
                                clip: RecordedClip::default(),
                            });
                        }
                        let mut slot = SampleSlot::new(Arc::clone(&clip));
                        slot.loop_enabled = entry.loop_enabled;
                        slot.gain_db = entry.slot_gain_db;
                        let loop_enabled = slot.loop_enabled;
                        let gain_db = slot.gain_db;
                        state.set_sample_slot(index, scene, Some(Arc::new(slot)));
                        let _ = audio.sample_tx.push(crate::audio::SampleUpload {
                            track: index as u8,
                            scene: scene as u8,
                            sample: Some(Arc::clone(&clip.buffer)),
                        });
                        let _ = audio.command_tx.push(Command::SetSampleLoop {
                            track: index as u8,
                            scene: scene as u8,
                            enabled: loop_enabled,
                        });
                        let _ = audio.command_tx.push(Command::SetSampleGain {
                            track: index as u8,
                            scene: scene as u8,
                            gain_db,
                        });
                    }
                    Err(error) => self.log_error(format!(
                        "Could not restore sample '{}': {error}",
                        entry.path.display()
                    )),
                }
            }

            let patch = track
                .synth
                .as_ref()
                .map(|synth| synth.to_patch())
                .unwrap_or_else(|| crate::audio::SynthXt::from_track(index));
            state.synth[index] = patch;
            for param in crate::audio::SynthParam::all() {
                let _ = audio.command_tx.push(Command::SetSynthParam {
                    track: index as u8,
                    param,
                    value: patch.get(param),
                });
            }

            state.instrument[index] = crate::state::InstrumentSource::from_id(&track.instrument);
            state.clip_gain_db[index] = track.clip_gain_db;
            state.clip_fade_in[index] = track.fade_in;
            state.clip_fade_out[index] = track.fade_out;
            sync_track_clip(state, audio, index);
        }

        for plugin in &project.plugins {
            let Some(descriptor) = self
                .scan_results
                .iter()
                .find(|candidate| candidate.path == plugin.path)
                .cloned()
                .or_else(|| PluginDescriptor::from_project(plugin.path.clone(), &plugin.format))
            else {
                self.log_error(format!(
                    "Project references {} ({}) which is not available",
                    plugin.name,
                    plugin.path.display()
                ));
                continue;
            };
            let state_bytes = if plugin.state.is_empty() {
                None
            } else {
                base64::engine::general_purpose::STANDARD
                    .decode(plugin.state.as_bytes())
                    .ok()
            };
            self.plugins.send(PluginCommand::Load {
                slot: plugin.slot,
                descriptor,
                state: state_bytes,
            });
            let _ = audio.command_tx.push(Command::SetTrackPlugin {
                track: plugin.slot,
                active: true,
            });
            if plugin.bypass {
                self.plugins.send(PluginCommand::Bypass {
                    slot: plugin.slot,
                    bypass: true,
                });
            }
        }

        self.project_name = project.name;
        self.project_path = Some(path.to_path_buf());
        self.refresh_project_list();
        self.notify(format!("Loaded {}", path.display()));
        Ok(())
    }

    pub fn refresh_project_list(&mut self) {
        self.projects = Project::list_available();
    }

    pub fn project_label(&self) -> String {
        match &self.project_path {
            Some(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| self.project_name.clone()),
            None => format!("{}.rproj", self.project_name),
        }
    }

    /// Send a virtual-keyboard note event to the engine.
    pub fn send_virtual_note(&mut self, key: usize) {
        const BASE: u8 = 60;
        let note = BASE + key as u8;
        self.midi.send(MidiMessage::NoteOn {
            channel: 0,
            note,
            velocity: 100,
        });
    }

    pub fn release_virtual_note(&mut self, key: usize) {
        const BASE: u8 = 60;
        let note = BASE + key as u8;
        self.midi.send(MidiMessage::NoteOff { channel: 0, note });
    }

    /// Open the piano roll for a track. `scene` only provides context in the
    /// header because the engine stores one MIDI clip per track.
    pub fn open_piano_roll(&mut self, track: usize, scene: u8) {
        self.piano_roll_open = true;
        self.piano_roll_track = track.min(TRACK_COUNT - 1);
        self.piano_roll_scene = scene;
        self.roll_selected = None;
        self.roll_drag = None;
    }

    pub fn open_audio_editor(&mut self, track: usize) {
        self.audio_editor_open = true;
        self.audio_editor_track = track.min(TRACK_COUNT - 1);
    }

    /// Open the large plugin editor. `fullscreen` fills the whole viewport.
    pub fn open_plugin_editor(&mut self, track: usize, fullscreen: bool) {
        self.plugin_editor_open = true;
        self.plugin_editor_track = track.min(TRACK_COUNT - 1);
        self.plugin_editor_fullscreen = fullscreen;
        self.selected_slot = track.min(TRACK_COUNT - 1);
    }

    /// Keep each track's internal synth mute in line with its instrument slot.
    ///
    /// Runs every frame, so a plugin loaded on a track that is not currently
    /// selected still takes over that track's sound source.
    pub fn sync_instrument_sources(&mut self, state: &mut UiState, audio: &mut AudioRuntime) {
        for track in 0..TRACK_COUNT {
            // A newly loaded instrument plugin takes over an empty slot.
            let loaded = self.slots[track].is_some();
            if loaded && !self.slot_loaded[track] {
                let is_instrument = self.slots[track]
                    .as_ref()
                    .is_some_and(|slot| slot.is_instrument);
                state.instrument[track] = source_after_load(state.instrument[track], is_instrument);
            }
            self.slot_loaded[track] = loaded;

            let want_synth = state.instrument[track] == crate::state::InstrumentSource::Synth;
            if state.synth_sent[track] != want_synth {
                state.synth_sent[track] = want_synth;
                let _ = audio.command_tx.push(Command::SetTrackSynth {
                    track: track as u8,
                    enabled: want_synth,
                });
            }

            let want_plugin = state.instrument[track] == crate::state::InstrumentSource::Plugin && self.slots[track].is_some();
            if self.plugin_sent[track] != want_plugin {
                self.plugin_sent[track] = want_plugin;
                let _ = audio.command_tx.push(Command::SetTrackPlugin {
                    track: track as u8,
                    active: want_plugin,
                });
            }
        }
    }

    /// Start renaming a track in the small inline prompt.
    pub fn begin_rename(&mut self, track: usize, current: &str) {
        self.rename_track = Some(track.min(TRACK_COUNT - 1));
        self.rename_buffer = current.to_string();
        self.rename_focus = true;
    }
}

impl PluginDescriptor {
    fn from_project(path: PathBuf, format: &str) -> Option<Self> {
        let format = match format {
            "CLAP" => crate::plugins::PluginFormat::Clap,
            "VST3" => crate::plugins::PluginFormat::Vst3,
            "VST2" => crate::plugins::PluginFormat::Vst2,
            _ => return None,
        };
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Plugin".to_string());
        Some(Self { name, path, format })
    }
}

/// Deciding the instrument source when a plugin is loaded or unloaded.
///
/// Loading a plugin only takes over an *empty* or already-plugin slot when the
/// plugin is an instrument; an effect plugin leaves the current source alone.
pub fn source_after_load(
    current: crate::state::InstrumentSource,
    plugin_is_instrument: bool,
) -> crate::state::InstrumentSource {
    use crate::state::InstrumentSource;
    if plugin_is_instrument {
        InstrumentSource::Plugin
    } else if current == InstrumentSource::Plugin {
        InstrumentSource::Empty
    } else {
        current
    }
}

/// Deciding the instrument source when a plugin is unloaded.
pub fn source_after_unload(
    current: crate::state::InstrumentSource,
) -> crate::state::InstrumentSource {
    use crate::state::InstrumentSource;
    match current {
        InstrumentSource::Plugin => InstrumentSource::Empty,
        other => other,
    }
}

/// Convenience for the UI: does this session slot hold recorded material?
pub fn has_recorded(state: &UiState, track: usize, scene: usize) -> bool {
    !state.recorded_notes(track, scene).is_empty()
}

/// Convert recorded notes to the fixed-capacity engine clip.
pub fn clip_from_notes(notes: &[RecordedNote]) -> RecordedClip {
    let mut clip = RecordedClip::default();
    for note in notes {
        clip.push(*note);
    }
    clip
}

/// Push one session slot's edited notes to the audio engine.
pub fn sync_track_notes(state: &UiState, audio: &mut AudioRuntime, track: usize, scene: usize) {
    if track >= TRACK_COUNT || scene >= crate::audio::SCENE_COUNT {
        return;
    }
    let clip = clip_from_notes(state.recorded_notes(track, scene));
    let _ = audio.command_tx.push(Command::SetRecordedClip {
        track: track as u8,
        scene: scene as u8,
        clip,
    });
}

/// Push one clip's loop region to the audio engine.
pub fn sync_clip_loop(state: &UiState, audio: &mut AudioRuntime, track: usize, scene: usize) {
    if track >= TRACK_COUNT || scene >= crate::audio::SCENE_COUNT {
        return;
    }
    let (start, end) = state.clip_loop(track, scene);
    let _ = audio.command_tx.push(Command::SetClipLoop {
        track: track as u8,
        scene: scene as u8,
        start,
        end,
    });
}

/// Push a track's audio-clip envelope to the audio engine.
pub fn sync_track_clip(state: &UiState, audio: &mut AudioRuntime, track: usize) {
    if track >= TRACK_COUNT {
        return;
    }
    let _ = audio.command_tx.push(Command::SetTrackClip {
        track: track as u8,
        gain: db_to_gain(state.clip_gain_db[track]),
        fade_in: state.clip_fade_in[track],
        fade_out: state.clip_fade_out[track],
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_conversion_preserves_notes() {
        let notes = vec![
            RecordedNote {
                note: 60,
                velocity: 90,
                start_step: 0,
                length_steps: 2,
            },
            RecordedNote {
                note: 64,
                velocity: 80,
                start_step: 8,
                length_steps: 1,
            },
        ];
        let clip = clip_from_notes(&notes);
        assert_eq!(clip.len, 2);
        assert_eq!(clip.as_slice(), notes.as_slice());
    }

    #[test]
    fn instrument_slot_decides_the_sound_source() {
        use crate::state::InstrumentSource;

        assert!(InstrumentSource::Synth.synth_audible());
        assert!(!InstrumentSource::Empty.synth_audible());
        assert!(!InstrumentSource::Plugin.synth_audible());

        // A new track starts empty: the chain shows ADD PLUGIN / INSTRUMENT.
        assert_eq!(InstrumentSource::default(), InstrumentSource::Empty);

        // Instrument plugins take over an empty or plugin slot.
        assert_eq!(
            source_after_load(InstrumentSource::Empty, true),
            InstrumentSource::Plugin
        );
        // Effect plugins leave an empty slot empty.
        assert_eq!(
            source_after_load(InstrumentSource::Empty, false),
            InstrumentSource::Empty
        );
        // An explicit FM choice survives an effect plugin.
        assert_eq!(
            source_after_load(InstrumentSource::Synth, false),
            InstrumentSource::Synth
        );
        // Unloading a plugin empties the slot.
        assert_eq!(
            source_after_unload(InstrumentSource::Plugin),
            InstrumentSource::Empty
        );
        assert_eq!(
            source_after_unload(InstrumentSource::Synth),
            InstrumentSource::Synth
        );
    }

    #[test]
    fn descriptor_from_project_is_total_for_known_formats() {
        let descriptor =
            PluginDescriptor::from_project(PathBuf::from("/tmp/X.clap"), "CLAP").unwrap();
        assert_eq!(descriptor.format, crate::plugins::PluginFormat::Clap);
        assert!(PluginDescriptor::from_project(PathBuf::from("/tmp/x.au"), "AU").is_none());
    }
}
