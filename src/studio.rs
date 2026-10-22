//! Studio services shared by the UI: MIDI input, plugin hosting and projects.
//!
//! The audio engine only sees lock-free rings. Everything that can allocate,
//! block or load third-party code lives here, on the UI or plugin-worker
//! threads.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use base64::Engine as _;

use crate::audio::messages::{Command, RecordedClip, RecordedNote, TRACK_COUNT};
use crate::audio::{db_to_gain, AudioRuntime, MidiMessage, MidiRuntime};
use crate::plugins::{
    scan_all, ParamInfo, PluginCommand, PluginDescriptor, PluginEvent, PluginRuntime,
    PluginSlotInfo, MAX_PLUGIN_SLOTS,
};
use crate::project::{Project, ProjectPlugin};
use crate::state::UiState;

pub const MAX_STATUS_CHARS: usize = 160;

/// Pointer gesture in the piano roll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollDragMode {
    Move,
    Resize,
}

#[derive(Clone, Copy, Debug)]
pub struct RollDrag {
    pub note_index: usize,
    pub original_start: u16,
    pub original_note: u8,
    pub original_length: u16,
    pub mode: RollDragMode,
}

pub struct Studio {
    pub midi: MidiRuntime,
    pub plugins: PluginRuntime,
    pub slots: [Option<PluginSlotInfo>; MAX_PLUGIN_SLOTS],
    pub slot_pending: [bool; MAX_PLUGIN_SLOTS],
    pub selected_slot: usize,

    pub scan_results: Vec<PluginDescriptor>,
    pub scan_running: bool,
    scan_rx: Option<Receiver<Vec<PluginDescriptor>>>,
    pub browser_open: bool,
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

    /// Audio-clip envelope editor.
    pub audio_editor_open: bool,
    pub audio_editor_track: usize,

    pub project_name: String,
    pub project_path: Option<PathBuf>,
    pub projects: Vec<PathBuf>,
    pub status: String,
    pub logs: Vec<String>,

    pub virtual_keys: [bool; 24],
    pub next_request: u64,
}

impl Studio {
    pub fn new(midi: MidiRuntime, plugins: PluginRuntime) -> Self {
        let mut studio = Self {
            midi,
            plugins,
            slots: std::array::from_fn(|_| None),
            slot_pending: [false; MAX_PLUGIN_SLOTS],
            selected_slot: 0,
            scan_results: Vec::new(),
            scan_running: false,
            scan_rx: None,
            browser_open: false,
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
            audio_editor_open: false,
            audio_editor_track: 0,
            project_name: "Untitled".to_string(),
            project_path: None,
            projects: Project::list_available(),
            status: String::new(),
            logs: Vec::new(),
            virtual_keys: [false; 24],
            next_request: 1,
        };
        studio.status = format!(
            "{} MIDI input(s) · {} project(s)",
            studio.midi.ports.len(),
            studio.projects.len()
        );
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

    /// Per-frame service pump.
    pub fn pump(&mut self) {
        self.midi.pump();

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
            PluginEvent::Log(message) => self.log_error(message),
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

            let mut clip = RecordedClip::default();
            state.set_recorded(index, track.notes.clone());
            for note in &track.notes {
                clip.push(*note);
            }
            let _ = audio.command_tx.push(Command::SetRecordedClip {
                track: index as u8,
                clip,
            });
            state.recorded_active[index] = track.clip_enabled;
            let _ = audio.command_tx.push(Command::SetRecordedActive {
                track: index as u8,
                active: track.clip_enabled,
            });

            if let Some(synth) = &track.synth {
                let params = synth.to_params();
                state.synth[index] = params;
                for param in crate::audio::SynthParam::ALL {
                    let _ = audio.command_tx.push(Command::SetSynthParam {
                        track: index as u8,
                        param,
                        value: params.get(param),
                    });
                }
            }

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

/// Convenience for the UI: does this track have recorded material?
pub fn has_recorded(state: &UiState, track: usize) -> bool {
    !state.recorded_notes(track).is_empty()
}

/// Convert recorded notes to the fixed-capacity engine clip.
pub fn clip_from_notes(notes: &[RecordedNote]) -> RecordedClip {
    let mut clip = RecordedClip::default();
    for note in notes {
        clip.push(*note);
    }
    clip
}

/// Push a track's edited notes to the audio engine.
pub fn sync_track_notes(state: &UiState, audio: &mut AudioRuntime, track: usize) {
    if track >= TRACK_COUNT {
        return;
    }
    let clip = clip_from_notes(state.recorded_notes(track));
    let _ = audio.command_tx.push(Command::SetRecordedClip {
        track: track as u8,
        clip,
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
    fn descriptor_from_project_is_total_for_known_formats() {
        let descriptor =
            PluginDescriptor::from_project(PathBuf::from("/tmp/X.clap"), "CLAP").unwrap();
        assert_eq!(descriptor.format, crate::plugins::PluginFormat::Clap);
        assert!(PluginDescriptor::from_project(PathBuf::from("/tmp/x.au"), "AU").is_none());
    }
}
