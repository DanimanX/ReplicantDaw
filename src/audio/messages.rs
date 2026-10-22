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
    /// Session slot the note belongs to.
    pub scene: u8,
    pub note: RecordedNote,
    /// True when recording started from a clean clip.
    pub reset: bool,
}

/// Editable parameters of the two-operator FM synth (per track).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SynthParam {
    /// Modulator : carrier frequency ratio.
    Ratio,
    /// FM modulation index.
    Index,
    Attack,
    Decay,
    Sustain,
    Release,
    Level,
}

impl SynthParam {
    pub const ALL: [Self; 7] = [
        Self::Ratio,
        Self::Index,
        Self::Attack,
        Self::Decay,
        Self::Sustain,
        Self::Release,
        Self::Level,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ratio => "RATIO",
            Self::Index => "INDEX",
            Self::Attack => "ATTACK",
            Self::Decay => "DECAY",
            Self::Sustain => "SUSTAIN",
            Self::Release => "RELEASE",
            Self::Level => "LEVEL",
        }
    }

    /// Inclusive range accepted by the engine.
    pub fn range(self) -> (f32, f32) {
        match self {
            Self::Ratio => (0.25, 8.0),
            Self::Index => (0.0, 8.0),
            Self::Attack => (0.001, 2.0),
            Self::Decay => (0.005, 3.0),
            Self::Sustain => (0.0, 1.0),
            Self::Release => (0.01, 4.0),
            Self::Level => (0.0, 1.5),
        }
    }

    /// True when the parameter is perceptually logarithmic (time constants).
    pub fn logarithmic(self) -> bool {
        matches!(self, Self::Attack | Self::Decay | Self::Release)
    }

    pub fn format(self, value: f32) -> String {
        match self {
            Self::Ratio => format!("{value:.2}x"),
            Self::Index => format!("{value:.2}"),
            Self::Attack | Self::Decay | Self::Release => {
                if value >= 1.0 {
                    format!("{value:.2}s")
                } else {
                    format!("{:.0}ms", value * 1000.0)
                }
            }
            Self::Sustain | Self::Level => format!("{:.0}%", value * 100.0),
        }
    }
}

/// Effect modules that can be inserted in the master chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectKind {
    Drive,
    Filter,
    Tone,
    Width,
    Delay,
}

/// One knob exposed by an effect module.
#[derive(Clone, Copy, Debug)]
pub struct EffectKnob {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub logarithmic: bool,
    pub suffix: &'static str,
}

impl EffectKind {
    pub const ALL: [Self; 5] = [
        Self::Drive,
        Self::Filter,
        Self::Tone,
        Self::Width,
        Self::Delay,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Drive => "SYNAPSE DRIVE",
            Self::Filter => "QUANTUM RESONATOR",
            Self::Tone => "NEURAL GEN V5",
            Self::Width => "MIRROR WIDTH",
            Self::Delay => "ECHO MEMORY",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Drive => "DRIVE",
            Self::Filter => "FILTER",
            Self::Tone => "TONE",
            Self::Width => "WIDTH",
            Self::Delay => "DELAY",
        }
    }

    /// Stable identifier used in project files.
    pub fn id(self) -> &'static str {
        self.short()
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.id().eq_ignore_ascii_case(id))
    }

    pub fn knobs(self) -> &'static [EffectKnob] {
        match self {
            Self::Drive => &[
                EffectKnob {
                    name: "DRIVE",
                    min: 0.0,
                    max: 1.0,
                    default: 0.12,
                    logarithmic: false,
                    suffix: "%",
                },
                EffectKnob {
                    name: "MIX",
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    logarithmic: false,
                    suffix: "%",
                },
                EffectKnob {
                    name: "OUT",
                    min: 0.0,
                    max: 2.0,
                    default: 1.0,
                    logarithmic: false,
                    suffix: "x",
                },
            ],
            Self::Filter => &[
                EffectKnob {
                    name: "CUTOFF",
                    min: 40.0,
                    max: 12_000.0,
                    default: 5_200.0,
                    logarithmic: true,
                    suffix: "Hz",
                },
                EffectKnob {
                    name: "RES",
                    min: 0.0,
                    max: 1.0,
                    default: 0.18,
                    logarithmic: false,
                    suffix: "%",
                },
                EffectKnob {
                    name: "MIX",
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    logarithmic: false,
                    suffix: "%",
                },
            ],
            Self::Tone => &[
                EffectKnob {
                    name: "CUT",
                    min: 300.0,
                    max: 18_000.0,
                    default: 16_000.0,
                    logarithmic: true,
                    suffix: "Hz",
                },
                EffectKnob {
                    name: "MIX",
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    logarithmic: false,
                    suffix: "%",
                },
                EffectKnob {
                    name: "OUT",
                    min: 0.0,
                    max: 2.0,
                    default: 1.0,
                    logarithmic: false,
                    suffix: "x",
                },
            ],
            Self::Width => &[
                EffectKnob {
                    name: "WIDTH",
                    min: 0.0,
                    max: 1.5,
                    default: 1.0,
                    logarithmic: false,
                    suffix: "%",
                },
                EffectKnob {
                    name: "MIX",
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    logarithmic: false,
                    suffix: "%",
                },
                EffectKnob {
                    name: "OUT",
                    min: 0.0,
                    max: 2.0,
                    default: 1.0,
                    logarithmic: false,
                    suffix: "x",
                },
            ],
            Self::Delay => &[
                EffectKnob {
                    name: "TIME",
                    min: 0.02,
                    max: 1.0,
                    default: 0.28,
                    logarithmic: true,
                    suffix: "s",
                },
                EffectKnob {
                    name: "FEEDBACK",
                    min: 0.0,
                    max: 0.9,
                    default: 0.35,
                    logarithmic: false,
                    suffix: "%",
                },
                EffectKnob {
                    name: "MIX",
                    min: 0.0,
                    max: 1.0,
                    default: 0.25,
                    logarithmic: false,
                    suffix: "%",
                },
            ],
        }
    }

    pub fn defaults(self) -> [f32; 3] {
        let knobs = self.knobs();
        [knobs[0].default, knobs[1].default, knobs[2].default]
    }
}

/// Maximum number of effect modules on the master chain.
pub const MAX_EFFECTS: usize = 6;

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
    /// Change one editable parameter of a track's FM synth.
    SetSynthParam {
        track: u8,
        param: SynthParam,
        value: f32,
    },
    /// Append an effect module at the end of the master chain.
    AddEffect {
        kind: EffectKind,
    },
    RemoveEffect {
        index: u8,
    },
    /// Move an effect inside the chain (indices are clamped by the engine).
    MoveEffect {
        from: u8,
        to: u8,
    },
    SetEffectParam {
        index: u8,
        knob: u8,
        value: f32,
    },
    SetEffectBypass {
        index: u8,
        bypass: bool,
    },
    ClearEffects,
    SetMasterGain {
        gain: f32,
    },
    /// Enable or mute one session clip slot.
    SetRecordedActive {
        track: u8,
        scene: u8,
        active: bool,
    },
    /// Start recording incoming MIDI into one session slot.
    RecordStart {
        track: u8,
        scene: u8,
    },
    RecordStop,
    ClearRecorded {
        track: u8,
        scene: u8,
    },
    /// Replace the MIDI clip of one session slot (editor and project load).
    SetRecordedClip {
        track: u8,
        scene: u8,
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
    /// Enable or disable the CLAP/VST3 insert hosted on a track.
    SetTrackPlugin {
        track: u8,
        active: bool,
    },
    /// Mute or unmute the internal FM synth of a track. Used when an
    /// instrument plugin replaces it in the track's instrument slot.
    SetTrackSynth {
        track: u8,
        enabled: bool,
    },
    /// Audio-clip envelope applied to a track before the master bus.
    /// Fades are normalized to the 16-bar loop (0.0 = off, 1.0 = whole loop).
    SetTrackClip {
        track: u8,
        gain: f32,
        fade_in: f32,
        fade_out: f32,
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
