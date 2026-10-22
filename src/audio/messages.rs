//! Messages exchanged between the UI thread and the audio thread.
//!
//! Both directions travel through lock-free SPSC ring buffers (`rtrb`).
//! The UI never blocks the audio callback and the audio callback never
//! allocates, locks or waits.

use serde::{Deserialize, Serialize};

pub const TRACK_COUNT: usize = 8;
pub const SCENE_COUNT: usize = 8;
pub const STEPS_PER_BAR: usize = 16;
/// Steps in the 16-bar session loop (16th-note resolution).
pub const LOOP_STEPS: usize = 256;
/// Maximum notes stored in one recorded clip.
pub const MAX_RECORDED_NOTES: usize = 512;

/// A note captured by MIDI recording or loaded from a project.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedNote {
    pub note: u8,
    pub velocity: u8,
    /// Start position in 16th notes from the loop start.
    pub start_step: u16,
    pub length_steps: u16,
}

impl RecordedNote {
    pub fn end_step(&self) -> u16 {
        self.start_step.saturating_add(self.length_steps.max(1))
    }
}

/// Fixed-capacity clip buffer so the audio thread never allocates.
#[derive(Clone, Copy)]
pub struct RecordedClip {
    pub notes: [RecordedNote; MAX_RECORDED_NOTES],
    pub len: u16,
}

impl Default for RecordedClip {
    fn default() -> Self {
        Self {
            notes: [RecordedNote::default(); MAX_RECORDED_NOTES],
            len: 0,
        }
    }
}

impl std::fmt::Debug for RecordedClip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordedClip")
            .field("len", &self.len)
            .finish()
    }
}

impl RecordedClip {
    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn push(&mut self, note: RecordedNote) {
        let index = self.len as usize;
        if index < MAX_RECORDED_NOTES {
            self.notes[index] = note;
            self.len += 1;
        }
    }

    pub fn as_slice(&self) -> &[RecordedNote] {
        &self.notes[..self.len as usize]
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Note captured by the engine, mirrored to the UI for display and saving.
#[derive(Clone, Copy, Debug)]
pub struct RecordEvent {
    pub track: u8,
    pub note: RecordedNote,
    /// True when recording started from a clean clip.
    pub reset: bool,
}

/// Parameters exposed by the modular device chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxParam {
    /// Output gain, linear (0.0 ..= 1.4).
    Gain,
    /// High-cut tone filter in Hz (300 ..= 18_000).
    Tone,
    /// Resonant low-pass cutoff in Hz (40 ..= 18_000).
    Cutoff,
    /// Filter resonance, normalized (0.0 ..= 1.0).
    Resonance,
    /// Saturation drive, normalized (0.0 ..= 1.0).
    Drive,
    /// Stereo width, 0.0 = mono, 1.0 = normal, 1.5 = wide.
    Width,
}

/// Commands sent from the UI thread to the audio thread.
///
/// The recorded-clip variant is intentionally large: it keeps clip loading
/// allocation-free on the audio thread. The queue is sized for UI events, not
/// for audio-rate traffic.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug)]
pub enum Command {
    Play,
    Stop,
    SetBpm(f32),
    SetTrackGain {
        track: u8,
        gain: f32,
    },
    SetTrackPan {
        track: u8,
        pan: f32,
    },
    SetTrackMute {
        track: u8,
        mute: bool,
    },
    SetTrackSolo {
        track: u8,
        solo: bool,
    },
    /// Launch the clip in `scene` on `track`, replacing any running clip.
    LaunchClip {
        track: u8,
        scene: u8,
    },
    StopClip {
        track: u8,
    },
    SetFx {
        param: FxParam,
        value: f32,
    },
    /// Start recording incoming MIDI on `track`, replacing its clip.
    RecordStart {
        track: u8,
    },
    RecordStop,
    ClearRecorded {
        track: u8,
    },
    /// Replace a track's recorded clip (project load).
    SetRecordedClip {
        track: u8,
        clip: RecordedClip,
    },
    /// Route incoming MIDI to the plugin chain as well as the internal synth.
    SetPluginMidi {
        enabled: bool,
    },
    /// Choose the track that receives live MIDI (and records it when armed).
    SetInputTrack {
        track: u8,
    },
}

/// Telemetry published by the audio thread at roughly 60 Hz.
#[derive(Clone, Copy, Debug)]
pub struct Telemetry {
    /// Frames elapsed in the current loop.
    pub position_frames: u64,
    pub playing: bool,
    pub bpm: f32,
    /// Per-track peak since the previous telemetry frame (linear).
    pub track_peaks: [f32; TRACK_COUNT],
    pub master_peak: [f32; 2],
    /// Audio callback load in percent of the real-time budget.
    pub cpu_load: f32,
    pub active_voices: u8,
    pub sample_rate: u32,
    pub recording: bool,
    pub record_track: u8,
    pub recorded_notes: u16,
    pub plugin_latency: u16,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            position_frames: 0,
            playing: false,
            bpm: 142.0,
            track_peaks: [0.0; TRACK_COUNT],
            master_peak: [0.0; 2],
            cpu_load: 0.0,
            active_voices: 0,
            sample_rate: 48_000,
            recording: false,
            record_track: 0,
            recorded_notes: 0,
            plugin_latency: 0,
        }
    }
}

/// Convert a decibel value to a linear gain; -inf and very low values map to 0.
pub fn db_to_gain(db: f32) -> f32 {
    if db <= -60.0 {
        0.0
    } else {
        10.0_f32.powf(db / 20.0)
    }
}

/// Convert a linear magnitude to decibels, clamped to -120 dB.
pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 1.0e-6 {
        -120.0
    } else {
        (20.0 * gain.log10()).max(-120.0)
    }
}
