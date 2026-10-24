//! UI-side session state. This mirrors the engine's observable state and
//! owns the visual-only data (meter ballistics, FFT history, selection).

use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::sync::Arc;

use rtrb::Consumer;

use crate::audio::messages::RecordedNote;
use crate::audio::{
    gain_to_db, EffectKind, RecordEvent, Telemetry, DEFAULT_BPM, SCENE_COUNT, TRACKS, TRACK_COUNT,
};
use crate::dsp::spectrum::SpectrumAnalyzer;
use crate::samples::{SampleClip, SampleSlot};

pub const MAX_SCOPE: usize = 4096;

/// What an arrangement clip plays.
///
/// Both variants own their material, so an arrangement clip is independent of
/// the session slot it may have been dragged from: editing or clearing one
/// never touches the other.
#[derive(Clone, Debug)]
pub enum ArrangementContent {
    /// A rendered audio sample.
    Audio {
        clip: Arc<SampleClip>,
        loop_enabled: bool,
        gain_db: f32,
    },
    /// Notes copied from a session slot, looping inside their own region.
    Midi {
        /// Name of the slot the notes came from, kept so the block can be
        /// labelled after the session has changed.
        name: String,
        notes: Vec<RecordedNote>,
        loop_start: u16,
        loop_end: u16,
    },
}

impl ArrangementContent {
    pub fn label(&self) -> String {
        match self {
            Self::Audio { clip, .. } => format!("AUDIO · {}", clip.name),
            Self::Midi { name, notes, .. } => format!("MIDI · {name} · {}N", notes.len()),
        }
    }

    pub fn audio(&self) -> Option<&Arc<SampleClip>> {
        match self {
            Self::Audio { clip, .. } => Some(clip),
            Self::Midi { .. } => None,
        }
    }

    pub fn notes(&self) -> &[RecordedNote] {
        match self {
            Self::Audio { .. } => &[],
            Self::Midi { notes, .. } => notes,
        }
    }

    /// Loop region of a MIDI clip, or `None` for audio.
    pub fn loop_region(&self) -> Option<(u16, u16)> {
        match self {
            Self::Audio { .. } => None,
            Self::Midi {
                loop_start,
                loop_end,
                ..
            } => Some((*loop_start, *loop_end)),
        }
    }
}

/// A clip placed on the arrangement timeline.
#[derive(Clone, Debug)]
pub struct ArrangementClip {
    pub id: u64,
    pub track: usize,
    /// Timeline position in 16th notes, from the start of the 16-bar loop.
    pub start_step: u16,
    pub length_steps: u16,
    pub content: ArrangementContent,
}

impl ArrangementClip {
    /// Snapshot of this clip's material, ready to be carried by a drag.
    pub fn drag_content(&self) -> crate::samples::DragContent {
        match &self.content {
            ArrangementContent::Audio { clip, .. } => {
                crate::samples::DragContent::Audio(Arc::clone(clip))
            }
            ArrangementContent::Midi {
                name,
                notes,
                loop_start,
                loop_end,
            } => crate::samples::DragContent::Midi {
                name: name.clone(),
                notes: notes.clone(),
                loop_start: *loop_start,
                loop_end: *loop_end,
            },
        }
    }
}

/// Sound source of a track's single instrument slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstrumentSource {
    /// Nothing loaded yet; the device chain offers `ADD PLUGIN / INSTRUMENT`.
    #[default]
    Empty,
    /// The CLAP/VST3 plugin hosted in the slot.
    Plugin,
}

impl InstrumentSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Empty => "EMPTY",
            Self::Plugin => "PLUGIN",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Plugin => "plugin",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id.to_ascii_lowercase().as_str() {
            // Projects from releases with the internal synth load with an
            // empty slot, so they cannot resurrect a source this build lacks.
            "synth" | "fm" => Self::Empty,
            "plugin" | "vst3" | "clap" => Self::Plugin,
            _ => Self::Empty,
        }
    }
}

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
    /// What makes this track sound: nothing or the hosted plugin. New tracks
    /// start empty, as the device chain shows.
    pub instrument: [InstrumentSource; TRACK_COUNT],
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
    /// Loop region of each clip, as `(start, end)` in 16th notes.
    pub clip_loop: [[(u16, u16); SCENE_COUNT]; TRACK_COUNT],
    /// Audio sample assigned to each session slot, if any.
    pub sample_slots: [[Option<Arc<SampleSlot>>; SCENE_COUNT]; TRACK_COUNT],
    /// Audio clips placed on the arrangement timeline.
    pub arrangement: Vec<ArrangementClip>,
    arrangement_serial: u64,
    pub recording: bool,
    pub record_track: u8,
    pub input_track: u8,
    pub recorded_total: u16,
    pub plugin_latency: u16,
    pub plugin_midi: bool,
    /// Engine playhead of each track's launched clip, in 16th notes.
    pub clip_steps: [u16; TRACK_COUNT],
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
            instrument: [InstrumentSource::Empty; TRACK_COUNT],
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
            clip_loop: [[(0, crate::audio::LOOP_STEPS as u16); SCENE_COUNT]; TRACK_COUNT],
            sample_slots: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            arrangement: Vec::new(),
            arrangement_serial: 1,
            recording: false,
            record_track: 0,
            input_track: 0,
            recorded_total: 0,
            plugin_latency: 0,
            plugin_midi: true,
            clip_steps: [0; TRACK_COUNT],
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
        self.clip_steps = frame.clip_steps;
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

    /// Engine phase of the clip currently launched on `track`, if `scene` is
    /// the slot that is playing. `None` means this clip is not the one the
    /// engine is advancing, so there is no honest playhead to draw.
    pub fn clip_phase(&self, track: usize, scene: usize) -> Option<u16> {
        if self.launched.get(track).copied().flatten() == Some(scene as u8) {
            self.clip_steps.get(track).copied()
        } else {
            None
        }
    }

    /// Loop region of a clip, never empty and always inside the 16-bar loop.
    pub fn clip_loop(&self, track: usize, scene: usize) -> (u16, u16) {
        let total = crate::audio::LOOP_STEPS as u16;
        let (start, end) = self
            .clip_loop
            .get(track)
            .and_then(|slots| slots.get(scene))
            .copied()
            .unwrap_or((0, total));
        let start = start.min(total - 1);
        let end = end.clamp(start + 1, total);
        (start, end)
    }

    /// Set the loop region of one clip. `start`/`end` are dragged by the user,
    /// so they are ordered and clamped rather than trusted.
    pub fn set_clip_loop(&mut self, track: usize, scene: usize, start: u16, end: u16) {
        let total = crate::audio::LOOP_STEPS as u16;
        let (lo, hi) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let lo = lo.min(total - 1);
        let hi = hi.clamp(lo + 1, total);
        if let Some(slot) = self.clip_loop.get_mut(track).and_then(|s| s.get_mut(scene)) {
            *slot = (lo, hi);
        }
    }

    /// Audio sample assigned to a session slot, with its slot settings.
    pub fn sample_slot(&self, track: usize, scene: usize) -> Option<&Arc<SampleSlot>> {
        self.sample_slots
            .get(track)
            .and_then(|slots| slots.get(scene))
            .and_then(|slot| slot.as_ref())
    }

    /// The sample itself, for drawing and editing.
    pub fn sample_clip(
        &self,
        track: usize,
        scene: usize,
    ) -> Option<&Arc<crate::samples::SampleClip>> {
        self.sample_slot(track, scene).map(|slot| &slot.clip)
    }

    pub fn set_sample_slot(&mut self, track: usize, scene: usize, sample: Option<Arc<SampleSlot>>) {
        if let Some(slot) = self
            .sample_slots
            .get_mut(track)
            .and_then(|slots| slots.get_mut(scene))
        {
            *slot = sample;
        }
    }

    /// Number of 16th-note steps a sample spans at the current tempo.
    pub fn steps_for_duration(&self, seconds: f32, start_step: u16) -> u16 {
        let seconds_per_step = 60.0 / self.bpm.max(1.0) / 4.0;
        let steps = (seconds / seconds_per_step).round().max(1.0) as u32;
        let remaining = crate::audio::LOOP_STEPS as u32
            - start_step.min(crate::audio::LOOP_STEPS as u16 - 1) as u32;
        steps.min(remaining).max(1) as u16
    }

    /// Place an audio clip on the arrangement, sized to the sample's duration.
    pub fn add_arrangement_clip(
        &mut self,
        track: usize,
        start_step: u16,
        clip: Arc<SampleClip>,
        loop_enabled: bool,
        gain_db: f32,
    ) -> u64 {
        let start_step = start_step.min(crate::audio::LOOP_STEPS as u16 - 1);
        let length_steps = self.steps_for_duration(clip.duration_secs(), start_step);
        self.insert_arrangement_clip(
            track,
            start_step,
            length_steps,
            ArrangementContent::Audio {
                clip,
                loop_enabled,
                gain_db,
            },
        )
    }

    /// Place an audio clip with an explicit length (used by project restore and
    /// by resizing).
    pub fn add_arrangement_clip_with_length(
        &mut self,
        track: usize,
        start_step: u16,
        length_steps: u16,
        clip: Arc<SampleClip>,
        loop_enabled: bool,
        gain_db: f32,
    ) -> u64 {
        self.insert_arrangement_clip(
            track,
            start_step,
            length_steps,
            ArrangementContent::Audio {
                clip,
                loop_enabled,
                gain_db,
            },
        )
    }

    /// Copy a session slot's notes onto the timeline as an independent clip.
    pub fn add_arrangement_midi(
        &mut self,
        track: usize,
        start_step: u16,
        name: String,
        notes: Vec<RecordedNote>,
        loop_start: u16,
        loop_end: u16,
    ) -> u64 {
        // The clip is as long as the region it loops, so what you drag is what
        // you see; it can be stretched afterwards.
        let length = loop_end.saturating_sub(loop_start).max(1);
        self.insert_arrangement_clip(
            track,
            start_step,
            length,
            ArrangementContent::Midi {
                name,
                notes,
                loop_start,
                loop_end,
            },
        )
    }

    /// Add a clip with an explicit length, keeping its content.
    pub fn insert_arrangement_clip(
        &mut self,
        track: usize,
        start_step: u16,
        length_steps: u16,
        content: ArrangementContent,
    ) -> u64 {
        let id = self.arrangement_serial;
        self.arrangement_serial = self.arrangement_serial.wrapping_add(1).max(1);
        let start_step = start_step.min(crate::audio::LOOP_STEPS as u16 - 1);
        let max_length = crate::audio::LOOP_STEPS as u16 - start_step;
        self.arrangement.push(ArrangementClip {
            id,
            track: track.min(TRACK_COUNT - 1),
            start_step,
            length_steps: length_steps.clamp(1, max_length.max(1)),
            content,
        });
        id
    }

    /// Change a clip's length, clamped to the timeline.
    pub fn resize_arrangement_clip(&mut self, id: u64, length_steps: u16) {
        if let Some(clip) = self.arrangement.iter_mut().find(|clip| clip.id == id) {
            let max_length = crate::audio::LOOP_STEPS as u16 - clip.start_step;
            clip.length_steps = length_steps.clamp(1, max_length.max(1));
        }
    }

    pub fn arrangement_clip(&self, id: u64) -> Option<&ArrangementClip> {
        self.arrangement.iter().find(|clip| clip.id == id)
    }

    pub fn remove_arrangement_clip(&mut self, id: u64) -> Option<ArrangementClip> {
        let index = self.arrangement.iter().position(|clip| clip.id == id)?;
        Some(self.arrangement.remove(index))
    }

    pub fn move_arrangement_clip(&mut self, id: u64, track: usize, start_step: u16) {
        if let Some(clip) = self.arrangement.iter_mut().find(|clip| clip.id == id) {
            clip.track = track.min(TRACK_COUNT - 1);
            clip.start_step = start_step.min(crate::audio::LOOP_STEPS as u16 - 1);
        }
    }

    pub fn clear_arrangement(&mut self) {
        self.arrangement.clear();
    }

    /// Toggle whether the slot repeats its sample inside the clip loop.
    pub fn set_sample_loop(&mut self, track: usize, scene: usize, enabled: bool) -> bool {
        match self
            .sample_slots
            .get_mut(track)
            .and_then(|slots| slots.get_mut(scene))
            .and_then(|slot| slot.as_mut())
        {
            Some(slot) => {
                let mut updated = (**slot).clone();
                updated.loop_enabled = enabled;
                *slot = Arc::new(updated);
                true
            }
            None => false,
        }
    }

    /// Per-slot trim in dB, applied before the track fader.
    pub fn set_sample_gain(&mut self, track: usize, scene: usize, gain_db: f32) -> bool {
        match self
            .sample_slots
            .get_mut(track)
            .and_then(|slots| slots.get_mut(scene))
            .and_then(|slot| slot.as_mut())
        {
            Some(slot) => {
                let mut updated = (**slot).clone();
                updated.gain_db = gain_db.clamp(-48.0, 12.0);
                *slot = Arc::new(updated);
                true
            }
            None => false,
        }
    }

    pub fn clear_samples(&mut self) {
        self.sample_slots = std::array::from_fn(|_| std::array::from_fn(|_| None));
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
    fn clip_phase_is_only_reported_for_the_launched_slot() {
        let mut state = UiState::new();
        state.launched[2] = Some(3);
        state.clip_steps[2] = 41;
        assert_eq!(state.clip_phase(2, 3), Some(41));
        assert_eq!(state.clip_phase(2, 2), None);
        assert_eq!(state.clip_phase(0, 3), None);
    }

    #[test]
    fn sample_slots_are_addressed_by_track_and_scene() {
        let mut state = UiState::new();
        let clip = std::sync::Arc::new(crate::samples::SampleClip {
            path: std::path::PathBuf::from("/tmp/kick.wav"),
            name: "kick".to_string(),
            source: std::sync::Arc::new(vec![0.0; 4]),
            source_rate: 48_000,
            source_frames: 2,
            edits: crate::samples::SampleEdits::default(),
            buffer: std::sync::Arc::new(crate::audio::SampleBuffer {
                frames: vec![0.0; 4],
                sample_rate: 48_000,
            }),
            peaks: vec![0.5],
        });
        state.set_sample_slot(
            1,
            4,
            Some(std::sync::Arc::new(crate::samples::SampleSlot::new(clip))),
        );
        assert!(state.sample_clip(1, 4).is_some());
        assert!(state.sample_clip(1, 3).is_none());
        assert!(state.sample_clip(0, 4).is_none());
        assert!(state.set_sample_loop(1, 4, false));
        assert!(!state.sample_slot(1, 4).expect("slot").loop_enabled);
        assert!(state.set_sample_gain(1, 4, -6.0));
        assert!((state.sample_slot(1, 4).expect("slot").gain_db + 6.0).abs() < 1e-6);
        state.set_sample_slot(1, 4, None);
        assert!(state.sample_clip(1, 4).is_none());
    }

    #[test]
    fn arrangement_clips_can_be_placed_moved_and_removed() {
        let mut state = UiState::new();
        let clip = std::sync::Arc::new(crate::samples::SampleClip {
            path: std::path::PathBuf::from("/tmp/loop.wav"),
            name: "loop".to_string(),
            source: std::sync::Arc::new(vec![0.0; 4]),
            source_rate: 48_000,
            source_frames: 2,
            edits: crate::samples::SampleEdits::default(),
            buffer: std::sync::Arc::new(crate::audio::SampleBuffer {
                frames: vec![0.0; 4],
                sample_rate: 48_000,
            }),
            peaks: vec![0.5],
        });
        let id = state.add_arrangement_clip(2, 32, clip, true, -3.0);
        assert!(state.arrangement_clip(id).is_some());
        assert_eq!(state.steps_for_duration(1.0, 0), 9);

        state.move_arrangement_clip(id, 4, 64);
        let moved = state.arrangement_clip(id).expect("clip");
        assert_eq!(moved.track, 4);
        assert_eq!(moved.start_step, 64);

        assert!(state.remove_arrangement_clip(id).is_some());
        assert!(state.arrangement_clip(id).is_none());
        state.clear_arrangement();
        assert!(state.arrangement.is_empty());
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
    fn clip_loop_is_ordered_and_clamped() {
        let mut state = UiState::new();
        let total = crate::audio::LOOP_STEPS as u16;
        assert_eq!(state.clip_loop(0, 0), (0, total));

        state.set_clip_loop(0, 0, 32, 16);
        assert_eq!(state.clip_loop(0, 0), (16, 32));

        state.set_clip_loop(0, 0, 250, 400);
        assert_eq!(state.clip_loop(0, 0), (250, total));

        // A zero-width drag still has to play something.
        state.set_clip_loop(0, 0, 40, 40);
        assert_eq!(state.clip_loop(0, 0), (40, 41));

        // Slots are independent.
        assert_eq!(state.clip_loop(0, 1), (0, total));
    }

    #[test]
    fn clip_envelope_defaults_to_unity() {
        let state = UiState::new();
        assert!(state.clip_gain_db.iter().all(|gain| *gain == 0.0));
        assert!(state.clip_fade_in.iter().all(|fade| *fade == 0.0));
        assert!(state.clip_fade_out.iter().all(|fade| *fade == 0.0));
    }
}
