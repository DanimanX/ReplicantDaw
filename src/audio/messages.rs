//! Messages exchanged between the UI thread and the audio thread.
//!
//! Both directions travel through lock-free SPSC ring buffers (`rtrb`).
//! The UI never blocks the audio callback and the audio callback never
//! allocates, locks or waits.

pub const TRACK_COUNT: usize = 8;
pub const SCENE_COUNT: usize = 8;
pub const STEPS_PER_BAR: usize = 16;

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
