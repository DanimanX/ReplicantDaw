//! UI-side session state. This mirrors the engine's observable state and
//! owns the visual-only data (meter ballistics, FFT history, selection).

use std::collections::VecDeque;
use std::f32::consts::TAU;

use rtrb::Consumer;

use crate::audio::messages::RecordedNote;
use crate::audio::{
    gain_to_db, EffectKind, RecordEvent, SynthParams, Telemetry, DEFAULT_BPM, SCENE_COUNT, TRACKS,
    TRACK_COUNT,
};
use crate::dsp::spectrum::SpectrumAnalyzer;

pub const MAX_SCOPE: usize = 4096;

/// One effect module mirrored from the engine chain.
#[derive(Clone, Copy, Debug)]
pub struct UiEffect {
    pub kind: EffectKind,
    pub params: [f32; 3],
    pub bypass: bool,
}

impl UiEffect {
    pub fn new(kind: EffectKind) -> Self {
        Self {
            kind,
            params: kind.defaults(),
            bypass: false,
        }
    }
}

/// The default master chain, matching the engine.
pub fn default_chain() -> Vec<UiEffect> {
    [
        EffectKind::Drive,
        EffectKind::Tone,
        EffectKind::Filter,
        EffectKind::Width,
    ]
    .into_iter()
    .map(UiEffect::new)
    .collect()
}

pub struct UiState {
    pub bpm: f32,
    pub playing: bool,
    pub position_frames: u64,
    pub sample_rate: u32,
    pub track_gain_db: [f32; TRACK_COUNT],
    pub track_pan: [f32; TRACK_COUNT],
    pub mute: [bool; TRACK_COUNT],
    pub solo: [bool; TRACK_COUNT],
    /// Currently launched scene per track (`None` = clip stopped).
    pub launched: [Option<u8>; TRACK_COUNT],
    /// User-editable track names (defaults come from the track specs).
    pub track_name: [String; TRACK_COUNT],
    pub peaks: [f32; TRACK_COUNT],
    pub master_peaks: [f32; 2],
    pub cpu: f32,
    pub voices: u8,
    pub effects: Vec<UiEffect>,
    pub master_gain: f32,
    /// Audio-clip envelope per track (gain in dB, loop-normalized fades).
    pub clip_gain_db: [f32; TRACK_COUNT],
    pub clip_fade_in: [f32; TRACK_COUNT],
    pub clip_fade_out: [f32; TRACK_COUNT],
    pub synth: [SynthParams; TRACK_COUNT],
    /// Internal FM synth audible on this track.
    pub synth_enabled: [bool; TRACK_COUNT],
    /// Explicit instrument source choice; `None` follows the plugin type.
    pub instrument_plugin: [Option<bool>; TRACK_COUNT],
    /// Per-slot clip playback enable, indexed by (track, scene).
    pub recorded_active: [[bool; SCENE_COUNT]; TRACK_COUNT],
    /// Floating "add effect" menu state.
    pub add_effect_open: bool,
    pub add_menu_pos: egui::Pos2,
    pub pending_chain_reset: bool,
    pub scope: VecDeque<f32>,
    scope_scratch: Vec<f32>,
    pub analyzer: SpectrumAnalyzer,
    pub selected_track: usize,
    pub hovered_scene: Option<usize>,
    /// Mirror of the engine's session clips, one per (track, scene).
    pub recorded: [[Vec<RecordedNote>; SCENE_COUNT]; TRACK_COUNT],
    pub recording: bool,
    pub record_track: u8,
    pub input_track: u8,
    pub recorded_total: u16,
    pub plugin_latency: u16,
    pub plugin_midi: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self::new()
    }
}

impl UiState {
    pub fn new() -> Self {
        let launched = std::array::from_fn(|track| Some(crate::audio::DEFAULT_SCENES[track] as u8));
        Self {
            bpm: DEFAULT_BPM,
            playing: false,
            position_frames: 0,
            sample_rate: 48_000,
            track_gain_db: std::array::from_fn(|i| TRACKS[i].gain_db),
            track_pan: std::array::from_fn(|i| TRACKS[i].pan),
            mute: [false; TRACK_COUNT],
            solo: [false; TRACK_COUNT],
            launched,
            track_name: std::array::from_fn(|index| TRACKS[index].name.to_string()),
            peaks: [0.0; TRACK_COUNT],
            master_peaks: [0.0; 2],
            cpu: 0.0,
            voices: 0,
            effects: default_chain(),
            master_gain: 0.90,
            clip_gain_db: [0.0; TRACK_COUNT],
            clip_fade_in: [0.0; TRACK_COUNT],
            clip_fade_out: [0.0; TRACK_COUNT],
            synth: std::array::from_fn(|index| SynthParams::from_spec(&TRACKS[index])),
            synth_enabled: [true; TRACK_COUNT],
            instrument_plugin: [None; TRACK_COUNT],
            recorded_active: [[true; SCENE_COUNT]; TRACK_COUNT],
            add_effect_open: false,
            add_menu_pos: egui::Pos2::new(200.0, 200.0),
            pending_chain_reset: false,
            scope: VecDeque::with_capacity(MAX_SCOPE),
            scope_scratch: Vec::with_capacity(8192),
            analyzer: SpectrumAnalyzer::new(48_000.0),
            selected_track: 0,
            hovered_scene: None,
            recorded: std::array::from_fn(|_| std::array::from_fn(|_| Vec::new())),
            recording: false,
            record_track: 0,
            input_track: 0,
            recorded_total: 0,
            plugin_latency: 0,
            plugin_midi: true,
        }
    }

    pub fn ingest(&mut self, frame: Telemetry, dt: f32) {
        self.playing = frame.playing;
        self.position_frames = frame.position_frames;
        self.sample_rate = frame.sample_rate;
        self.voices = frame.active_voices;
        self.recording = frame.recording;
        self.record_track = frame.record_track;
        self.recorded_total = frame.recorded_notes;
        self.plugin_latency = frame.plugin_latency;
        self.cpu += (frame.cpu_load - self.cpu) * 0.22;

        let decay = (-dt.max(0.001) * 7.5).exp();
        for (index, peak) in frame.track_peaks.iter().enumerate() {
            self.peaks[index] = (self.peaks[index] * decay).max(*peak).clamp(0.0, 4.0);
        }
        for channel in 0..2 {
            self.master_peaks[channel] = (self.master_peaks[channel] * decay)
                .max(frame.master_peak[channel])
                .clamp(0.0, 4.0);
        }
    }

    pub fn drain_scope(&mut self, rx: &mut Consumer<f32>) {
        self.scope_scratch.clear();
        for _ in 0..8192 {
            match rx.pop() {
                Ok(sample) => self.scope_scratch.push(sample),
                Err(_) => break,
            }
        }
        if self.scope_scratch.is_empty() {
            return;
        }

        self.analyzer.push_samples(&self.scope_scratch);
        for &sample in &self.scope_scratch {
            if self.scope.len() >= MAX_SCOPE {
                self.scope.pop_front();
            }
            self.scope.push_back(sample);
        }
    }

    /// Apply a recording event mirrored from the engine.
    pub fn apply_record_event(&mut self, event: RecordEvent) {
        let track = event.track as usize;
        let scene = event.scene as usize;
        if track >= TRACK_COUNT || scene >= SCENE_COUNT {
            return;
        }
        if event.reset {
            self.recorded[track][scene].clear();
            return;
        }
        if self.recorded[track][scene].len() < crate::audio::messages::MAX_RECORDED_NOTES {
            self.recorded[track][scene].push(event.note);
        }
    }

    pub fn recorded_notes(&self, track: usize, scene: usize) -> &[RecordedNote] {
        self.recorded
            .get(track)
            .and_then(|slots| slots.get(scene))
            .map(|notes| notes.as_slice())
            .unwrap_or(&[])
    }

    pub fn clear_recorded(&mut self, track: usize, scene: usize) {
        if let Some(slot) = self.recorded.get_mut(track).and_then(|s| s.get_mut(scene)) {
            slot.clear();
        }
    }

    pub fn set_recorded(&mut self, track: usize, scene: usize, notes: Vec<RecordedNote>) {
        if let Some(slot) = self.recorded.get_mut(track).and_then(|s| s.get_mut(scene)) {
            *slot = notes;
        }
    }

    /// User-facing track name (falls back to the static spec).
    pub fn track_label(&self, track: usize) -> &str {
        self.track_name
            .get(track)
            .map(|name| name.as_str())
            .unwrap_or(TRACKS[track.min(TRACK_COUNT - 1)].name)
    }

    pub fn rename_track(&mut self, track: usize, name: &str) {
        if let Some(slot) = self.track_name.get_mut(track) {
            let cleaned = name.trim();
            *slot = if cleaned.is_empty() {
                TRACKS[track.min(TRACK_COUNT - 1)].name.to_string()
            } else {
                cleaned.chars().take(24).collect()
            };
        }
    }

    /// Does any session slot of this track hold a MIDI clip?
    pub fn track_has_clips(&self, track: usize) -> bool {
        self.recorded
            .get(track)
            .is_some_and(|slots| slots.iter().any(|notes| !notes.is_empty()))
    }

    /// Append a note to a track's clip. Returns the new note index.
    pub fn add_note(
        &mut self,
        track: usize,
        scene: usize,
        note: u8,
        velocity: u8,
        start_step: u16,
        length_steps: u16,
    ) -> Option<usize> {
        let notes = self.recorded.get_mut(track)?.get_mut(scene)?;
        if notes.len() >= crate::audio::messages::MAX_RECORDED_NOTES {
            return None;
        }
        notes.push(RecordedNote {
            note,
            velocity,
            start_step: start_step.min(crate::audio::LOOP_STEPS as u16 - 1),
            length_steps: length_steps.max(1),
        });
        Some(notes.len() - 1)
    }

    pub fn remove_note(&mut self, track: usize, scene: usize, index: usize) {
        if let Some(notes) = self.recorded.get_mut(track).and_then(|s| s.get_mut(scene)) {
            if index < notes.len() {
                notes.remove(index);
            }
        }
    }

    /// Move a note to a new start step and pitch, clamped to the loop.
    pub fn move_note(
        &mut self,
        track: usize,
        scene: usize,
        index: usize,
        start_step: i32,
        note: i32,
    ) {
        if let Some(notes) = self.recorded.get_mut(track).and_then(|s| s.get_mut(scene)) {
            if let Some(target) = notes.get_mut(index) {
                target.start_step = start_step.clamp(0, crate::audio::LOOP_STEPS as i32 - 1) as u16;
                target.note = note.clamp(0, 127) as u8;
                let remaining = crate::audio::LOOP_STEPS as u16 - target.start_step;
                target.length_steps = target.length_steps.min(remaining.max(1));
            }
        }
    }

    /// Resize a note, keeping at least one step and staying inside the loop.
    pub fn resize_note(&mut self, track: usize, scene: usize, index: usize, length_steps: i32) {
        if let Some(notes) = self.recorded.get_mut(track).and_then(|s| s.get_mut(scene)) {
            if let Some(target) = notes.get_mut(index) {
                let max = crate::audio::LOOP_STEPS as i32 - target.start_step as i32;
                target.length_steps = length_steps.clamp(1, max.max(1)) as u16;
            }
        }
    }

    pub fn set_note_velocity(&mut self, track: usize, scene: usize, index: usize, velocity: u8) {
        if let Some(notes) = self.recorded.get_mut(track).and_then(|s| s.get_mut(scene)) {
            if let Some(target) = notes.get_mut(index) {
                target.velocity = velocity.clamp(1, 127);
            }
        }
    }

    /// Synth parameters for the selected track.
    pub fn synth_params(&self, track: usize) -> SynthParams {
        self.synth[track.min(TRACK_COUNT - 1)]
    }

    /// Append an effect to the mirrored chain, bounded by the engine limit.
    pub fn add_effect(&mut self, kind: EffectKind) -> Option<usize> {
        if self.effects.len() >= crate::audio::MAX_EFFECTS {
            return None;
        }
        self.effects.push(UiEffect::new(kind));
        Some(self.effects.len() - 1)
    }

    pub fn remove_effect(&mut self, index: usize) {
        if index < self.effects.len() {
            self.effects.remove(index);
        }
    }

    pub fn move_effect(&mut self, from: usize, to: usize) {
        if from < self.effects.len() && to < self.effects.len() && from != to {
            let effect = self.effects.remove(from);
            self.effects.insert(to, effect);
        }
    }

    pub fn reset_chain(&mut self) {
        self.effects = default_chain();
        self.master_gain = 0.90;
    }

    pub fn analyse(&mut self) {
        self.analyzer.compute();
        self.analyzer.push_history();
    }

    /// Beat position in the 16-bar loop.
    pub fn beats(&self) -> f64 {
        if self.bpm <= 1.0 {
            return 0.0;
        }
        self.position_frames as f64 * self.bpm as f64 / (self.sample_rate as f64 * 60.0)
    }

    pub fn playhead_beats(&self) -> f32 {
        self.beats() as f32
    }

    /// (bar, beat, sixteenth) all 1-based for display, plus the fraction
    /// through the current sixteenth for micro-animation.
    pub fn transport_display(&self) -> (u32, u32, u32, f32) {
        let beats = self.beats();
        let bar = (beats / 4.0).floor() as u32 + 1;
        let beat = (beats.floor() as u32) % 4 + 1;
        let step = ((beats * 4.0).floor() as u32) % 4 + 1;
        let fraction = (beats * 4.0).fract() as f32;
        (bar, beat, step, fraction)
    }

    /// Elapsed loop time in seconds.
    pub fn elapsed_secs(&self) -> f64 {
        self.position_frames as f64 / self.sample_rate.max(1) as f64
    }

    pub fn master_db(&self) -> f32 {
        gain_to_db(self.master_peaks[0].max(self.master_peaks[1]))
    }

    pub fn any_solo(&self) -> bool {
        self.solo.iter().any(|s| *s)
    }

    /// Deterministic pseudo-waveform for a clip slot. Purely visual; the
    /// actual notes come from `audio::tracks::note_at`.
    pub fn clip_waveform(&self, track: usize, scene: usize, points: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(points);
        let accent_phase = (track * 7 + scene * 3) as f32 * 0.37;
        let recorded = self.recorded_notes(track, scene);
        for i in 0..points {
            let t = i as f32 / points.max(1) as f32;
            let step = (t * 16.0) as usize;
            let gate = if recorded.is_empty() {
                match crate::audio::note_at(track, scene, step) {
                    Some(note) => ((note % 12) as f32 / 12.0 * 0.5 + 0.5) * 0.6,
                    None => 0.0,
                }
            } else {
                let loop_step = (t * crate::audio::LOOP_STEPS as f32) as u16;
                recorded
                    .iter()
                    .find(|note| loop_step >= note.start_step && loop_step < note.end_step())
                    .map(|note| ((note.note % 12) as f32 / 12.0 * 0.5 + 0.5) * 0.7)
                    .unwrap_or(0.0)
            };
            let carrier = (t * TAU * (7.0 + track as f32 * 0.8) + accent_phase).sin();
            let harmonic = (t * TAU * (17.0 + scene as f32) + accent_phase * 2.0).sin();
            out.push((carrier * 0.55 + harmonic * 0.2) * gate);
        }
        out
    }

    pub fn scene_name(scene: usize) -> &'static str {
        crate::audio::SCENE_NAMES[scene.min(SCENE_COUNT - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_display_starts_at_bar_one() {
        let state = UiState::new();
        let (bar, beat, step, _) = state.transport_display();
        assert_eq!((bar, beat, step), (1, 1, 1));
    }

    #[test]
    fn transport_display_tracks_bpm_and_rate() {
        let mut state = UiState::new();
        state.bpm = 120.0;
        state.sample_rate = 48_000;
        state.position_frames = 48_000; // one second = two beats
        let (bar, beat, step, _) = state.transport_display();
        assert_eq!((bar, beat, step), (1, 3, 1));
    }

    #[test]
    fn meter_decays_between_frames() {
        let mut state = UiState::new();
        state.peaks[0] = 1.0;
        let t = Telemetry {
            track_peaks: [0.0; TRACK_COUNT],
            ..Telemetry::default()
        };
        for _ in 0..60 {
            state.ingest(t, 1.0 / 60.0);
        }
        assert!(state.peaks[0] < 0.01, "peak {}", state.peaks[0]);
    }

    #[test]
    fn effect_chain_helpers_mirror_the_engine() {
        let mut state = UiState::new();
        assert_eq!(state.effects.len(), 4);
        assert_eq!(state.effects[0].kind, crate::audio::EffectKind::Drive);

        let index = state
            .add_effect(crate::audio::EffectKind::Delay)
            .expect("append effect");
        assert_eq!(index, 4);
        state.move_effect(4, 0);
        assert_eq!(state.effects[0].kind, crate::audio::EffectKind::Delay);

        state.remove_effect(0);
        assert_eq!(state.effects.len(), 4);

        while state.add_effect(crate::audio::EffectKind::Filter).is_some() {}
        assert_eq!(state.effects.len(), crate::audio::MAX_EFFECTS);

        state.reset_chain();
        assert_eq!(state.effects.len(), 4);
        assert!((state.master_gain - 0.90).abs() < 1e-6);
    }

    #[test]
    fn synth_params_are_editable_per_track() {
        let mut state = UiState::new();
        state.synth[3].set(crate::audio::SynthParam::Index, 6.0);
        assert!((state.synth_params(3).mod_index - 6.0).abs() < 1e-6);
        assert!((state.synth_params(2).mod_index - 6.0).abs() > 1e-3);
    }

    #[test]
    fn clip_waveform_is_bounded() {
        let state = UiState::new();
        for track in 0..TRACK_COUNT {
            for scene in 0..SCENE_COUNT {
                let wave = state.clip_waveform(track, scene, 64);
                assert_eq!(wave.len(), 64);
                assert!(wave.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
            }
        }
    }

    #[test]
    fn note_editing_helpers_clamp_to_the_loop() {
        let mut state = UiState::new();
        let index = state.add_note(2, 3, 64, 100, 8, 4).expect("add note");
        assert_eq!(state.recorded_notes(2, 3).len(), 1);

        state.move_note(2, 3, index, -20, 200);
        let note = state.recorded_notes(2, 3)[0];
        assert_eq!(note.start_step, 0);
        assert_eq!(note.note, 127);

        state.resize_note(2, 3, index, 999);
        assert_eq!(
            state.recorded_notes(2, 3)[0].length_steps,
            crate::audio::LOOP_STEPS as u16
        );

        state.set_note_velocity(2, 3, index, 0);
        assert_eq!(state.recorded_notes(2, 3)[0].velocity, 1);

        state.remove_note(2, 3, index);
        assert!(state.recorded_notes(2, 3).is_empty());
    }

    #[test]
    fn session_slots_hold_independent_clips() {
        let mut state = UiState::new();
        state.add_note(1, 0, 60, 100, 0, 2).expect("slot 0");
        state.add_note(1, 5, 72, 90, 16, 4).expect("slot 5");
        assert_eq!(state.recorded_notes(1, 0).len(), 1);
        assert_eq!(state.recorded_notes(1, 0)[0].note, 60);
        assert_eq!(state.recorded_notes(1, 5)[0].note, 72);
        assert!(state.recorded_notes(1, 1).is_empty());

        state.clear_recorded(1, 0);
        assert!(state.recorded_notes(1, 0).is_empty());
        assert_eq!(state.recorded_notes(1, 5).len(), 1);
    }

    #[test]
    fn track_names_are_editable_and_bounded() {
        let mut state = UiState::new();
        state.rename_track(0, "  MY BASS  ");
        assert_eq!(state.track_label(0), "MY BASS");
        state.rename_track(0, "   ");
        assert_eq!(state.track_label(0), TRACKS[0].name);
        state.rename_track(1, &"X".repeat(80));
        assert_eq!(state.track_label(1).chars().count(), 24);
    }

    #[test]
    fn clip_envelope_defaults_to_unity() {
        let state = UiState::new();
        assert!(state.clip_gain_db.iter().all(|gain| *gain == 0.0));
        assert!(state.clip_fade_in.iter().all(|fade| *fade == 0.0));
        assert!(state.clip_fade_out.iter().all(|fade| *fade == 0.0));
    }
}
