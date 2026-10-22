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
    pub peaks: [f32; TRACK_COUNT],
    pub master_peaks: [f32; 2],
    pub cpu: f32,
    pub voices: u8,
    pub effects: Vec<UiEffect>,
    pub master_gain: f32,
    pub synth: [SynthParams; TRACK_COUNT],
    pub recorded_active: [bool; TRACK_COUNT],
    /// Floating "add effect" menu state.
    pub add_effect_open: bool,
    pub add_menu_pos: egui::Pos2,
    pub pending_chain_reset: bool,
    pub scope: VecDeque<f32>,
    scope_scratch: Vec<f32>,
    pub analyzer: SpectrumAnalyzer,
    pub selected_track: usize,
    pub hovered_scene: Option<usize>,
    /// Mirror of the engine's recorded clips, used for display and saving.
    pub recorded: [Vec<RecordedNote>; TRACK_COUNT],
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
            peaks: [0.0; TRACK_COUNT],
            master_peaks: [0.0; 2],
            cpu: 0.0,
            voices: 0,
            effects: default_chain(),
            master_gain: 0.90,
            synth: std::array::from_fn(|index| SynthParams::from_spec(&TRACKS[index])),
            recorded_active: [true; TRACK_COUNT],
            add_effect_open: false,
            add_menu_pos: egui::Pos2::new(200.0, 200.0),
            pending_chain_reset: false,
            scope: VecDeque::with_capacity(MAX_SCOPE),
            scope_scratch: Vec::with_capacity(8192),
            analyzer: SpectrumAnalyzer::new(48_000.0),
            selected_track: 0,
            hovered_scene: None,
            recorded: std::array::from_fn(|_| Vec::new()),
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
        if track >= TRACK_COUNT {
            return;
        }
        if event.reset {
            self.recorded[track].clear();
            return;
        }
        if self.recorded[track].len() < crate::audio::messages::MAX_RECORDED_NOTES {
            self.recorded[track].push(event.note);
        }
    }

    pub fn recorded_notes(&self, track: usize) -> &[RecordedNote] {
        self.recorded
            .get(track)
            .map(|notes| notes.as_slice())
            .unwrap_or(&[])
    }

    pub fn clear_recorded(&mut self, track: usize) {
        if let Some(notes) = self.recorded.get_mut(track) {
            notes.clear();
        }
    }

    pub fn set_recorded(&mut self, track: usize, notes: Vec<RecordedNote>) {
        if let Some(slot) = self.recorded.get_mut(track) {
            *slot = notes;
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
        for i in 0..points {
            let t = i as f32 / points.max(1) as f32;
            let step = (t * 16.0) as usize;
            let gate = match crate::audio::note_at(track, scene, step) {
                Some(note) => ((note % 12) as f32 / 12.0 * 0.5 + 0.5) * 0.6,
                None => 0.0,
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
}
