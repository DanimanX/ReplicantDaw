//! NEURAL GEN XT: a hybrid wavetable/subtractive synthesizer.
//!
//! The architecture follows the classic hybrid synths (Surge XT among them):
//! two oscillators with unison, two multimode filters, three envelopes, two
//! LFOs and a modulation matrix. It is written from scratch for this DAW, not
//! a port: the DSP is deliberately small, allocation-free and branch-light so
//! a 16-voice track costs a fraction of a core.
//!
//! Everything in this module is `Copy` and lives in preallocated buffers; the
//! audio callback only reads parameters and ticks voices.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use crate::dsp::midi_to_hz;

pub const OSC_COUNT: usize = 2;
pub const FILTER_COUNT: usize = 2;
pub const ENV_COUNT: usize = 3;
pub const LFO_COUNT: usize = 2;
pub const MOD_SLOTS: usize = 6;
pub const MAX_UNISON: usize = 5;

/// Envelope index: amplitude.
pub const ENV_AMP: usize = 0;
/// Envelope index: filter.
pub const ENV_FILTER: usize = 1;
/// Envelope index: free modulation.
pub const ENV_MOD: usize = 2;

/// Destination slots of the modulation matrix.
const DEST_COUNT: usize = 13;
const DEST_PITCH: usize = 0;
const DEST_OSC1_PITCH: usize = 1;
const DEST_OSC2_PITCH: usize = 2;
const DEST_CUTOFF1: usize = 3;
const DEST_CUTOFF2: usize = 4;
const DEST_RES1: usize = 5;
const DEST_RES2: usize = 6;
const DEST_OSC1_LEVEL: usize = 7;
const DEST_OSC2_LEVEL: usize = 8;
const DEST_PAN: usize = 9;
const DEST_DRIVE: usize = 10;
const DEST_OSC1_MORPH: usize = 11;
const DEST_OSC2_MORPH: usize = 12;

pub const WAVETABLE_COUNT: usize = 10;
pub const WT_FRAMES: usize = 5;
pub const WT_SIZE: usize = 256;

pub const WAVETABLE_NAMES: [&str; WAVETABLE_COUNT] = [
    "BASIC SHAPES",
    "SURGE CLASSIC",
    "VOCAL FORMANTS",
    "CYBER METAL",
    "HARMONIC SPECTRUM",
    "PWM MODERN",
    "ANALOG WARMTH",
    "SUB DRONE",
    "RESONANCE SWEEP",
    "DIGITAL ACID",
];

static WAVETABLE_BANK: std::sync::OnceLock<[[[f32; WT_SIZE]; WT_FRAMES]; WAVETABLE_COUNT]> =
    std::sync::OnceLock::new();

fn init_wavetable_bank() -> [[[f32; WT_SIZE]; WT_FRAMES]; WAVETABLE_COUNT] {
    let mut bank = [[[0.0_f32; WT_SIZE]; WT_FRAMES]; WAVETABLE_COUNT];
    for (table_idx, table) in bank.iter_mut().enumerate() {
        for (frame_idx, frame) in table.iter_mut().enumerate() {
            let mut max_abs = 0.0_f32;
            for (sample_idx, sample) in frame.iter_mut().enumerate() {
                let t = sample_idx as f32 / WT_SIZE as f32;
                let val = generate_raw_wt_sample(table_idx, frame_idx, t);
                *sample = val;
                max_abs = max_abs.max(val.abs());
            }
            if max_abs > 1.0e-5 {
                let norm = 0.98 / max_abs;
                for sample in frame.iter_mut() {
                    *sample *= norm;
                }
            }
        }
    }
    bank
}

fn generate_raw_wt_sample(table: usize, frame: usize, t: f32) -> f32 {
    match table {
        // Table 0: Basic Shapes (Sine -> Triangle -> Saw -> Square -> Pulse)
        0 => match frame {
            0 => fast_sine(t),
            1 => 1.0 - 4.0 * (t - 0.5).abs(),
            2 => {
                let mut s = 0.0;
                for k in 1..=16 {
                    let kf = k as f32;
                    s += (kf * TAU * t).sin() / kf;
                }
                -2.0 / PI * s
            }
            3 => {
                let mut s = 0.0;
                for k in (1..=23).step_by(2) {
                    let kf = k as f32;
                    s += (kf * TAU * t).sin() / kf;
                }
                4.0 / PI * s
            }
            _ => {
                if t < 0.20 {
                    1.0
                } else {
                    -0.25
                }
            }
        },
        // Table 1: Surge Classic (Warm fundamental harmonics sweeping into multi-peak spectral)
        1 => match frame {
            0 => (TAU * t).sin() + 0.45 * (2.0 * TAU * t).sin(),
            1 => {
                0.7 * (TAU * t).sin()
                    + 0.5 * (2.0 * TAU * t).sin()
                    + 0.35 * (3.0 * TAU * t).sin()
                    + 0.2 * (4.0 * TAU * t).sin()
            }
            2 => {
                (TAU * t).sin()
                    + 0.6 * (3.0 * TAU * t).sin()
                    + 0.4 * (5.0 * TAU * t).sin()
                    + 0.3 * (7.0 * TAU * t).sin()
            }
            3 => {
                let mut s = 0.0;
                for k in 1..=14 {
                    let f = k as f32;
                    let boost = 1.0 / (1.0 + (f - 6.0).powi(2) * 0.4);
                    s += (f * TAU * t).sin() * (1.0 / f + boost * 0.85);
                }
                s
            }
            _ => {
                let mut s = 0.0;
                for k in 1..=16 {
                    let f = k as f32;
                    let b1 = 1.0 / (1.0 + (f - 4.0).powi(2) * 0.5);
                    let b2 = 1.0 / (1.0 + (f - 11.0).powi(2) * 0.5);
                    s += (f * TAU * t).sin() * (0.6 / f + b1 * 0.9 + b2 * 0.7);
                }
                s
            }
        },
        // Table 2: Vocal Formants (/a/ -> /e/ -> /i/ -> /o/ -> /u/)
        2 => {
            let (f1_center, f2_center) = match frame {
                0 => (8.0, 12.0),
                1 => (5.0, 18.0),
                2 => (3.0, 23.0),
                3 => (5.0, 9.0),
                _ => (3.0, 8.0),
            };
            let mut s = 0.0;
            for k in 1..=24 {
                let f = k as f32;
                let b1 = 1.0 / (1.0 + (f - f1_center).powi(2) * 0.5);
                let b2 = 1.0 / (1.0 + (f - f2_center).powi(2) * 0.5);
                s += (f * TAU * t).sin() * (b1 * 1.0 + b2 * 0.75 + 0.15 / f);
            }
            s
        }
        // Table 3: Cyber Metal (Inharmonic partials & FM bell-like textures)
        3 => match frame {
            0 => (TAU * t).sin() + 0.65 * (2.76 * TAU * t).sin() + 0.35 * (5.40 * TAU * t).sin(),
            1 => {
                (TAU * t).sin()
                    + 0.55 * (1.414 * TAU * t).sin()
                    + 0.4 * (PI * TAU * t).sin()
                    + 0.25 * (4.2 * TAU * t).sin()
            }
            2 => (TAU * t).sin() + 0.7 * (3.75 * TAU * t).sin() + 0.5 * (6.33 * TAU * t).sin(),
            3 => (TAU * t + 1.8 * (2.0 * TAU * t).sin()).sin(),
            _ => (TAU * t + 3.2 * (3.0 * TAU * t).sin()).sin(),
        },
        // Table 4: Harmonic Spectrum (Additive partial sweeps)
        4 => {
            let max_harmonic = match frame {
                0 => 1,
                1 => 4,
                2 => 8,
                3 => 12,
                _ => 16,
            };
            let mut s = 0.0;
            for k in 1..=max_harmonic {
                let f = k as f32;
                let amp = if frame == 4 { 1.0 / f.sqrt() } else { 1.0 / f };
                s += (f * TAU * t).sin() * amp;
            }
            s
        }
        // Table 5: PWM Modern (50% square down to 10% needle pulse)
        5 => {
            let duty = match frame {
                0 => 0.50,
                1 => 0.40,
                2 => 0.30,
                3 => 0.20,
                _ => 0.10,
            };
            if t < duty {
                1.0 - duty
            } else {
                -duty
            }
        }
        // Table 6: Analog Warmth (Overdriven analog shapes)
        6 => match frame {
            0 => ((TAU * t).sin() * 2.0).tanh() + 0.18 * (2.0 * TAU * t).cos(),
            1 => ((1.0 - 4.0 * (t - 0.5).abs()) * 2.2).tanh(),
            2 => (2.0 * t - 1.0) + 0.35 * (2.0 * t - 1.0).powi(2),
            3 => ((2.0 * t - 1.0) * 2.5).clamp(-0.85, 0.85),
            _ => {
                let wave = (TAU * t).sin() * 2.5;
                if wave > 1.0 {
                    2.0 - wave
                } else if wave < -1.0 {
                    -2.0 - wave
                } else {
                    wave
                }
            }
        },
        // Table 7: Sub Drone (Heavy deep foundations)
        7 => match frame {
            0 => (TAU * t).sin() + 0.5 * (0.5 * TAU * t).sin(),
            1 => (TAU * t).sin() + 0.35 * (3.0 * TAU * t).sin() + 0.4 * (0.5 * TAU * t).sin(),
            2 => (1.0 - 4.0 * (t - 0.5).abs()) + 0.4 * (TAU * t).sin(),
            3 => {
                let mut s = 0.0;
                for k in 1..=6 {
                    let f = k as f32;
                    s += (f * TAU * t).sin() / (f * f);
                }
                s
            }
            _ => ((TAU * t).sin() * 3.2).tanh(),
        },
        // Table 8: Resonance Sweep (Resonant harmonic peak moving up)
        8 => {
            let peak_harmonic = match frame {
                0 => 2.0,
                1 => 4.0,
                2 => 7.0,
                3 => 10.0,
                _ => 14.0,
            };
            let mut s = 0.0;
            for k in 1..=18 {
                let f = k as f32;
                let res = 1.0 / (1.0 + (f - peak_harmonic).powi(2) * 0.6);
                s += (f * TAU * t).sin() * (0.2 / f + res * 1.2);
            }
            s
        }
        // Table 9: Digital Acid (Aggressive sync-morph wave)
        _ => match frame {
            0 => 2.0 * t - 1.0 + 0.4 * (3.0 * TAU * t).sin(),
            1 => (t * 1.5).fract() * 2.0 - 1.0,
            2 => (t * 2.2).fract() * 2.0 - 1.0,
            3 => (t * 3.0 * PI).sin().abs() * 2.0 - 1.0,
            _ => ((t * 3.7).fract() * 2.0 - 1.0) * (1.0 - t * 0.7),
        },
    }
}

pub fn sample_wavetable(table: usize, morph: f32, phase: f32, _increment: f32) -> f32 {
    let bank = WAVETABLE_BANK.get_or_init(init_wavetable_bank);
    let table = table.min(WAVETABLE_COUNT - 1);
    let morph = morph.clamp(0.0, 1.0);
    let f_pos = morph * (WT_FRAMES - 1) as f32;
    let f0 = (f_pos.floor() as usize).min(WT_FRAMES - 2);
    let f1 = f0 + 1;
    let f_frac = f_pos - f0 as f32;

    let p = phase.fract() * WT_SIZE as f32;
    let p0 = (p.floor() as usize) % WT_SIZE;
    let p1 = (p0 + 1) % WT_SIZE;
    let p_frac = p - p0 as f32;

    let s0 = (1.0 - p_frac) * bank[table][f0][p0] + p_frac * bank[table][f0][p1];
    let s1 = (1.0 - p_frac) * bank[table][f1][p0] + p_frac * bank[table][f1][p1];

    (1.0 - f_frac) * s0 + f_frac * s1
}

pub fn get_wavetable_frame_preview(table: usize, morph: f32, points: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(points);
    for i in 0..points {
        let phase = i as f32 / points as f32;
        out.push(sample_wavetable(table, morph, phase, 0.01));
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Waveform {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
    Pulse,
    Noise,
    Wavetable,
}

impl Waveform {
    pub const ALL: [Self; 7] = [
        Self::Sine,
        Self::Triangle,
        Self::Saw,
        Self::Square,
        Self::Pulse,
        Self::Noise,
        Self::Wavetable,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Sine => "SINE",
            Self::Triangle => "TRI",
            Self::Saw => "SAW",
            Self::Square => "SQR",
            Self::Pulse => "PULSE",
            Self::Noise => "NOISE",
            Self::Wavetable => "WT",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Sine => "sine",
            Self::Triangle => "triangle",
            Self::Saw => "saw",
            Self::Square => "square",
            Self::Pulse => "pulse",
            Self::Noise => "noise",
            Self::Wavetable => "wavetable",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "triangle" => Self::Triangle,
            "saw" => Self::Saw,
            "square" => Self::Square,
            "pulse" => Self::Pulse,
            "noise" => Self::Noise,
            "wavetable" => Self::Wavetable,
            _ => Self::Sine,
        }
    }

    pub fn from_index(index: f32) -> Self {
        Self::ALL[(index.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FilterKind {
    #[default]
    LowPass12,
    LowPass24,
    HighPass12,
    HighPass24,
    BandPass,
    Notch,
}

impl FilterKind {
    pub const ALL: [Self; 6] = [
        Self::LowPass12,
        Self::LowPass24,
        Self::HighPass12,
        Self::HighPass24,
        Self::BandPass,
        Self::Notch,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::LowPass12 => "LP 12",
            Self::LowPass24 => "LP 24",
            Self::HighPass12 => "HP 12",
            Self::HighPass24 => "HP 24",
            Self::BandPass => "BAND",
            Self::Notch => "NOTCH",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::LowPass12 => "lp12",
            Self::LowPass24 => "lp24",
            Self::HighPass12 => "hp12",
            Self::HighPass24 => "hp24",
            Self::BandPass => "band",
            Self::Notch => "notch",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "lp24" => Self::LowPass24,
            "hp12" => Self::HighPass12,
            "hp24" => Self::HighPass24,
            "band" => Self::BandPass,
            "notch" => Self::Notch,
            _ => Self::LowPass12,
        }
    }

    pub fn from_index(index: f32) -> Self {
        Self::ALL[(index.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LfoShape {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
    Random,
}

impl LfoShape {
    pub const ALL: [Self; 5] = [
        Self::Sine,
        Self::Triangle,
        Self::Saw,
        Self::Square,
        Self::Random,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Sine => "SINE",
            Self::Triangle => "TRI",
            Self::Saw => "SAW",
            Self::Square => "SQR",
            Self::Random => "S&H",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Sine => "sine",
            Self::Triangle => "triangle",
            Self::Saw => "saw",
            Self::Square => "square",
            Self::Random => "random",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "triangle" => Self::Triangle,
            "saw" => Self::Saw,
            "square" => Self::Square,
            "random" => Self::Random,
            _ => Self::Sine,
        }
    }

    pub fn from_index(index: f32) -> Self {
        Self::ALL[(index.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ModSource {
    #[default]
    Off,
    Lfo1,
    Lfo2,
    Env2,
    Env3,
    Velocity,
    KeyTrack,
    Random,
}

impl ModSource {
    pub const ALL: [Self; 8] = [
        Self::Off,
        Self::Lfo1,
        Self::Lfo2,
        Self::Env2,
        Self::Env3,
        Self::Velocity,
        Self::KeyTrack,
        Self::Random,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Lfo1 => "LFO 1",
            Self::Lfo2 => "LFO 2",
            Self::Env2 => "ENV 2",
            Self::Env3 => "ENV 3",
            Self::Velocity => "VEL",
            Self::KeyTrack => "KEY",
            Self::Random => "RND",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Lfo1 => "lfo1",
            Self::Lfo2 => "lfo2",
            Self::Env2 => "env2",
            Self::Env3 => "env3",
            Self::Velocity => "velocity",
            Self::KeyTrack => "key",
            Self::Random => "random",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "lfo1" => Self::Lfo1,
            "lfo2" => Self::Lfo2,
            "env2" => Self::Env2,
            "env3" => Self::Env3,
            "velocity" => Self::Velocity,
            "key" => Self::KeyTrack,
            "random" => Self::Random,
            _ => Self::Off,
        }
    }

    pub fn from_index(index: f32) -> Self {
        Self::ALL[(index.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ModDest {
    #[default]
    Off,
    Pitch,
    Osc1Pitch,
    Osc2Pitch,
    Cutoff1,
    Cutoff2,
    Resonance1,
    Resonance2,
    Osc1Level,
    Osc2Level,
    Pan,
    Drive,
    Osc1Morph,
    Osc2Morph,
}

impl ModDest {
    pub const ALL: [Self; 14] = [
        Self::Off,
        Self::Pitch,
        Self::Osc1Pitch,
        Self::Osc2Pitch,
        Self::Cutoff1,
        Self::Cutoff2,
        Self::Resonance1,
        Self::Resonance2,
        Self::Osc1Level,
        Self::Osc2Level,
        Self::Pan,
        Self::Drive,
        Self::Osc1Morph,
        Self::Osc2Morph,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Pitch => "PITCH",
            Self::Osc1Pitch => "OSC 1 PITCH",
            Self::Osc2Pitch => "OSC 2 PITCH",
            Self::Cutoff1 => "CUTOFF 1",
            Self::Cutoff2 => "CUTOFF 2",
            Self::Resonance1 => "RES 1",
            Self::Resonance2 => "RES 2",
            Self::Osc1Level => "OSC 1 LEVEL",
            Self::Osc2Level => "OSC 2 LEVEL",
            Self::Pan => "PAN",
            Self::Drive => "DRIVE",
            Self::Osc1Morph => "OSC 1 MORPH",
            Self::Osc2Morph => "OSC 2 MORPH",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Pitch => "pitch",
            Self::Osc1Pitch => "osc1_pitch",
            Self::Osc2Pitch => "osc2_pitch",
            Self::Cutoff1 => "cutoff1",
            Self::Cutoff2 => "cutoff2",
            Self::Resonance1 => "res1",
            Self::Resonance2 => "res2",
            Self::Osc1Level => "osc1_level",
            Self::Osc2Level => "osc2_level",
            Self::Pan => "pan",
            Self::Drive => "drive",
            Self::Osc1Morph => "osc1_morph",
            Self::Osc2Morph => "osc2_morph",
        }
    }

    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|dest| dest.id() == id)
            .unwrap_or(Self::Off)
    }

    pub fn from_index(index: f32) -> Self {
        Self::ALL[(index.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }

    fn slot(self) -> Option<usize> {
        match self {
            Self::Off => None,
            Self::Pitch => Some(DEST_PITCH),
            Self::Osc1Pitch => Some(DEST_OSC1_PITCH),
            Self::Osc2Pitch => Some(DEST_OSC2_PITCH),
            Self::Cutoff1 => Some(DEST_CUTOFF1),
            Self::Cutoff2 => Some(DEST_CUTOFF2),
            Self::Resonance1 => Some(DEST_RES1),
            Self::Resonance2 => Some(DEST_RES2),
            Self::Osc1Level => Some(DEST_OSC1_LEVEL),
            Self::Osc2Level => Some(DEST_OSC2_LEVEL),
            Self::Pan => Some(DEST_PAN),
            Self::Drive => Some(DEST_DRIVE),
            Self::Osc1Morph => Some(DEST_OSC1_MORPH),
            Self::Osc2Morph => Some(DEST_OSC2_MORPH),
        }
    }
}

/// One oscillator's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OscParams {
    pub wave: Waveform,
    pub table: f32,
    pub morph: f32,
    pub octave: f32,
    pub semitone: f32,
    pub fine: f32,
    pub level: f32,
    pub pan: f32,
    pub unison: f32,
    pub detune: f32,
    pub phase: f32,
}

impl Default for OscParams {
    fn default() -> Self {
        Self {
            wave: Waveform::Wavetable,
            table: 1.0,
            morph: 0.35,
            octave: 0.0,
            semitone: 0.0,
            fine: 0.0,
            level: 0.75,
            pan: 0.0,
            unison: 2.0,
            detune: 8.0,
            phase: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterParams {
    pub enabled: bool,
    pub kind: FilterKind,
    pub cutoff: f32,
    pub resonance: f32,
    pub drive: f32,
    pub keytrack: f32,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            // The second filter starts bypassed: it costs nothing until used.
            enabled: false,
            kind: FilterKind::LowPass24,
            cutoff: 4_000.0,
            resonance: 0.15,
            drive: 0.0,
            keytrack: 0.35,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvParams {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for EnvParams {
    fn default() -> Self {
        Self {
            attack: 0.005,
            decay: 0.35,
            sustain: 0.6,
            release: 0.3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LfoParams {
    pub shape: LfoShape,
    pub rate: f32,
    pub depth: f32,
}

impl Default for LfoParams {
    fn default() -> Self {
        Self {
            shape: LfoShape::Sine,
            rate: 3.2,
            depth: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModRoute {
    pub source: ModSource,
    pub dest: ModDest,
    pub amount: f32,
}

impl Default for ModRoute {
    fn default() -> Self {
        Self {
            source: ModSource::Off,
            dest: ModDest::Off,
            amount: 0.0,
        }
    }
}

/// A complete patch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SynthXt {
    pub osc: [OscParams; OSC_COUNT],
    pub noise: f32,
    pub filter: [FilterParams; FILTER_COUNT],
    pub serial_filters: bool,
    pub env: [EnvParams; ENV_COUNT],
    pub env_amount: f32,
    pub lfo: [LfoParams; LFO_COUNT],
    pub matrix: [ModRoute; MOD_SLOTS],
    pub volume: f32,
    pub glide: f32,
    pub polyphony: f32,
}

impl Default for SynthXt {
    fn default() -> Self {
        Self {
            osc: [OscParams::default(); OSC_COUNT],
            noise: 0.0,
            filter: [
                FilterParams {
                    enabled: true,
                    ..FilterParams::default()
                },
                FilterParams::default(),
            ],
            serial_filters: true,
            env: [EnvParams::default(); ENV_COUNT],
            env_amount: 0.45,
            lfo: [LfoParams::default(); LFO_COUNT],
            matrix: [ModRoute::default(); MOD_SLOTS],
            volume: 0.8,
            glide: 0.0,
            polyphony: 16.0,
        }
    }
}

/// One editable parameter, addressed by group and index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SynthParam {
    OscWave(u8),
    OscTable(u8),
    OscMorph(u8),
    OscOctave(u8),
    OscSemitone(u8),
    OscFine(u8),
    OscLevel(u8),
    OscPan(u8),
    OscUnison(u8),
    OscDetune(u8),
    OscPhase(u8),
    Noise,
    FilterEnabled(u8),
    FilterKind(u8),
    FilterCutoff(u8),
    FilterResonance(u8),
    FilterDrive(u8),
    FilterKeytrack(u8),
    SerialFilters,
    EnvAttack(u8),
    EnvDecay(u8),
    EnvSustain(u8),
    EnvRelease(u8),
    EnvAmount,
    LfoShape(u8),
    LfoRate(u8),
    LfoDepth(u8),
    ModSource(u8),
    ModDest(u8),
    ModAmount(u8),
    Volume,
    Glide,
    Polyphony,
}

impl SynthParam {
    /// Every parameter, in panel order. Built on the UI thread, never in the
    /// audio callback.
    pub fn all() -> Vec<Self> {
        let mut params = Vec::with_capacity(84);
        for osc in 0..OSC_COUNT as u8 {
            params.extend([
                Self::OscWave(osc),
                Self::OscTable(osc),
                Self::OscMorph(osc),
                Self::OscOctave(osc),
                Self::OscSemitone(osc),
                Self::OscFine(osc),
                Self::OscLevel(osc),
                Self::OscPan(osc),
                Self::OscUnison(osc),
                Self::OscDetune(osc),
                Self::OscPhase(osc),
            ]);
        }
        params.push(Self::Noise);
        for filter in 0..FILTER_COUNT as u8 {
            params.extend([
                Self::FilterEnabled(filter),
                Self::FilterKind(filter),
                Self::FilterCutoff(filter),
                Self::FilterResonance(filter),
                Self::FilterDrive(filter),
                Self::FilterKeytrack(filter),
            ]);
        }
        params.push(Self::SerialFilters);
        for env in 0..ENV_COUNT as u8 {
            params.extend([
                Self::EnvAttack(env),
                Self::EnvDecay(env),
                Self::EnvSustain(env),
                Self::EnvRelease(env),
            ]);
        }
        params.push(Self::EnvAmount);
        for lfo in 0..LFO_COUNT as u8 {
            params.extend([Self::LfoShape(lfo), Self::LfoRate(lfo), Self::LfoDepth(lfo)]);
        }
        for slot in 0..MOD_SLOTS as u8 {
            params.extend([
                Self::ModSource(slot),
                Self::ModDest(slot),
                Self::ModAmount(slot),
            ]);
        }
        params.extend([Self::Volume, Self::Glide, Self::Polyphony]);
        params
    }

    pub fn label(self) -> String {
        match self {
            Self::OscWave(i) => format!("OSC {} WAVE", i + 1),
            Self::OscTable(i) => format!("OSC {} TABLE", i + 1),
            Self::OscMorph(i) => format!("OSC {} MORPH", i + 1),
            Self::OscOctave(i) => format!("OSC {} OCT", i + 1),
            Self::OscSemitone(i) => format!("OSC {} SEMI", i + 1),
            Self::OscFine(i) => format!("OSC {} FINE", i + 1),
            Self::OscLevel(i) => format!("OSC {} LEVEL", i + 1),
            Self::OscPan(i) => format!("OSC {} PAN", i + 1),
            Self::OscUnison(i) => format!("OSC {} UNI", i + 1),
            Self::OscDetune(i) => format!("OSC {} DETUNE", i + 1),
            Self::OscPhase(i) => format!("OSC {} PHASE", i + 1),
            Self::Noise => "NOISE".to_string(),
            Self::FilterEnabled(i) => format!("FILTER {} ON", i + 1),
            Self::FilterKind(i) => format!("FILTER {} TYPE", i + 1),
            Self::FilterCutoff(i) => format!("FILTER {} CUTOFF", i + 1),
            Self::FilterResonance(i) => format!("FILTER {} RES", i + 1),
            Self::FilterDrive(i) => format!("FILTER {} DRIVE", i + 1),
            Self::FilterKeytrack(i) => format!("FILTER {} KEY", i + 1),
            Self::SerialFilters => "FILTER ROUTING".to_string(),
            Self::EnvAttack(i) => format!("ENV {} ATTACK", i + 1),
            Self::EnvDecay(i) => format!("ENV {} DECAY", i + 1),
            Self::EnvSustain(i) => format!("ENV {} SUSTAIN", i + 1),
            Self::EnvRelease(i) => format!("ENV {} RELEASE", i + 1),
            Self::EnvAmount => "ENV 2 → CUTOFF".to_string(),
            Self::LfoShape(i) => format!("LFO {} SHAPE", i + 1),
            Self::LfoRate(i) => format!("LFO {} RATE", i + 1),
            Self::LfoDepth(i) => format!("LFO {} DEPTH", i + 1),
            Self::ModSource(i) => format!("MOD {} SOURCE", i + 1),
            Self::ModDest(i) => format!("MOD {} DEST", i + 1),
            Self::ModAmount(i) => format!("MOD {} AMOUNT", i + 1),
            Self::Volume => "VOLUME".to_string(),
            Self::Glide => "GLIDE".to_string(),
            Self::Polyphony => "POLYPHONY".to_string(),
        }
    }

    /// Stable id used by project files.
    pub fn id(self) -> String {
        match self {
            Self::OscWave(i) => format!("osc{}.wave", i + 1),
            Self::OscTable(i) => format!("osc{}.table", i + 1),
            Self::OscMorph(i) => format!("osc{}.morph", i + 1),
            Self::OscOctave(i) => format!("osc{}.octave", i + 1),
            Self::OscSemitone(i) => format!("osc{}.semitone", i + 1),
            Self::OscFine(i) => format!("osc{}.fine", i + 1),
            Self::OscLevel(i) => format!("osc{}.level", i + 1),
            Self::OscPan(i) => format!("osc{}.pan", i + 1),
            Self::OscUnison(i) => format!("osc{}.unison", i + 1),
            Self::OscDetune(i) => format!("osc{}.detune", i + 1),
            Self::OscPhase(i) => format!("osc{}.phase", i + 1),
            Self::Noise => "noise".to_string(),
            Self::FilterEnabled(i) => format!("filter{}.enabled", i + 1),
            Self::FilterKind(i) => format!("filter{}.type", i + 1),
            Self::FilterCutoff(i) => format!("filter{}.cutoff", i + 1),
            Self::FilterResonance(i) => format!("filter{}.res", i + 1),
            Self::FilterDrive(i) => format!("filter{}.drive", i + 1),
            Self::FilterKeytrack(i) => format!("filter{}.key", i + 1),
            Self::SerialFilters => "filter.serial".to_string(),
            Self::EnvAttack(i) => format!("env{}.attack", i + 1),
            Self::EnvDecay(i) => format!("env{}.decay", i + 1),
            Self::EnvSustain(i) => format!("env{}.sustain", i + 1),
            Self::EnvRelease(i) => format!("env{}.release", i + 1),
            Self::EnvAmount => "env2.amount".to_string(),
            Self::LfoShape(i) => format!("lfo{}.shape", i + 1),
            Self::LfoRate(i) => format!("lfo{}.rate", i + 1),
            Self::LfoDepth(i) => format!("lfo{}.depth", i + 1),
            Self::ModSource(i) => format!("mod{}.source", i + 1),
            Self::ModDest(i) => format!("mod{}.dest", i + 1),
            Self::ModAmount(i) => format!("mod{}.amount", i + 1),
            Self::Volume => "volume".to_string(),
            Self::Glide => "glide".to_string(),
            Self::Polyphony => "polyphony".to_string(),
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::all().into_iter().find(|param| param.id() == id)
    }

    /// Inclusive range accepted by the engine.
    pub fn range(self) -> (f32, f32) {
        match self {
            Self::OscWave(_) => (0.0, (Waveform::ALL.len() - 1) as f32),
            Self::OscTable(_) => (0.0, (WAVETABLE_COUNT - 1) as f32),
            Self::OscMorph(_) => (0.0, 1.0),
            Self::OscOctave(_) => (-3.0, 3.0),
            Self::OscSemitone(_) => (-12.0, 12.0),
            Self::OscFine(_) => (-50.0, 50.0),
            Self::OscLevel(_) | Self::OscPhase(_) => (0.0, 1.0),
            Self::OscPan(_) => (-1.0, 1.0),
            Self::OscUnison(_) => (1.0, MAX_UNISON as f32),
            Self::OscDetune(_) => (0.0, 60.0),
            Self::Noise => (0.0, 1.0),
            Self::FilterEnabled(_) => (0.0, 1.0),
            Self::FilterKind(_) => (0.0, (FilterKind::ALL.len() - 1) as f32),
            Self::FilterCutoff(_) => (20.0, 18_000.0),
            Self::FilterResonance(_) | Self::FilterDrive(_) | Self::FilterKeytrack(_) => (0.0, 1.0),
            Self::SerialFilters => (0.0, 1.0),
            Self::EnvAttack(_) => (0.0005, 8.0),
            Self::EnvDecay(_) => (0.005, 12.0),
            Self::EnvSustain(_) => (0.0, 1.0),
            Self::EnvRelease(_) => (0.005, 12.0),
            Self::EnvAmount => (-1.0, 1.0),
            Self::LfoShape(i) => {
                let _ = i;
                (0.0, (LfoShape::ALL.len() - 1) as f32)
            }
            Self::LfoRate(_) => (0.01, 30.0),
            Self::LfoDepth(_) => (0.0, 1.0),
            Self::ModSource(_) => (0.0, (ModSource::ALL.len() - 1) as f32),
            Self::ModDest(_) => (0.0, (ModDest::ALL.len() - 1) as f32),
            Self::ModAmount(_) => (-1.0, 1.0),
            Self::Volume => (0.0, 1.5),
            Self::Glide => (0.0, 2.0),
            Self::Polyphony => (1.0, 32.0),
        }
    }

    /// Perceptually logarithmic parameters (times, cutoffs, rates).
    pub fn logarithmic(self) -> bool {
        matches!(
            self,
            Self::FilterCutoff(_)
                | Self::EnvAttack(_)
                | Self::EnvDecay(_)
                | Self::EnvRelease(_)
                | Self::LfoRate(_)
        )
    }

    pub fn format(self, value: f32) -> String {
        match self {
            Self::OscWave(_) => Waveform::from_index(value).label().to_string(),
            Self::OscTable(_) => {
                let idx = (value.round() as usize).min(WAVETABLE_COUNT - 1);
                WAVETABLE_NAMES[idx].to_string()
            }
            Self::OscMorph(_) => format!("{:.0}%", (value * 100.0).clamp(0.0, 100.0)),
            Self::OscOctave(_) => format!("{:+.0}", value),
            Self::OscSemitone(_) => format!("{:+.0}", value),
            Self::OscFine(_) => format!("{:+.0}c", value),
            Self::OscLevel(_) | Self::Noise | Self::LfoDepth(_) => {
                format!("{:.0}%", value * 100.0)
            }
            Self::OscPan(_) => {
                if value.abs() < 0.02 {
                    "C".to_string()
                } else if value < 0.0 {
                    format!("L{:.0}", -value * 100.0)
                } else {
                    format!("R{:.0}", value * 100.0)
                }
            }
            Self::OscUnison(_) => format!("{:.0}", value),
            Self::OscDetune(_) => format!("{:.1}c", value),
            Self::OscPhase(_) => format!("{:.0}°", value * 360.0),
            Self::FilterEnabled(_) => {
                if value >= 0.5 {
                    "ON".to_string()
                } else {
                    "OFF".to_string()
                }
            }
            Self::FilterKind(_) => FilterKind::from_index(value).label().to_string(),
            Self::FilterCutoff(_) => {
                if value >= 1_000.0 {
                    format!("{:.1}k", value / 1_000.0)
                } else {
                    format!("{value:.0}")
                }
            }
            Self::FilterResonance(_) | Self::FilterDrive(_) | Self::FilterKeytrack(_) => {
                format!("{:.0}%", value * 100.0)
            }
            Self::SerialFilters => {
                if value >= 0.5 {
                    "SERIAL".to_string()
                } else {
                    "PARALLEL".to_string()
                }
            }
            Self::EnvAttack(_) | Self::EnvDecay(_) | Self::EnvRelease(_) | Self::Glide => {
                if value >= 1.0 {
                    format!("{value:.2}s")
                } else {
                    format!("{:.0}ms", value * 1_000.0)
                }
            }
            Self::EnvSustain(_) => format!("{:.0}%", value * 100.0),
            Self::EnvAmount => format!("{:+.0}%", value * 100.0),
            Self::LfoShape(_) => LfoShape::from_index(value).label().to_string(),
            Self::LfoRate(_) => format!("{value:.2}Hz"),
            Self::ModSource(_) => ModSource::from_index(value).label().to_string(),
            Self::ModDest(_) => ModDest::from_index(value).label().to_string(),
            Self::ModAmount(_) => format!("{:+.0}%", value * 100.0),
            Self::Volume => format!("{:.0}%", value * 100.0),
            Self::Polyphony => format!("{:.0}", value),
        }
    }

    /// Value used when the user double-clicks a knob.
    pub fn default_value(self) -> f32 {
        match self {
            Self::OscWave(_) => 6.0, // Default to Wavetable!
            Self::OscTable(i) => {
                if i == 0 {
                    1.0 // Surge Classic
                } else {
                    0.0 // Basic Shapes
                }
            }
            Self::OscMorph(i) => {
                if i == 0 {
                    0.35
                } else {
                    0.50
                }
            }
            Self::OscOctave(_) | Self::OscSemitone(_) | Self::OscFine(_) => 0.0,
            Self::OscLevel(_) => 0.75,
            Self::OscPan(_) | Self::OscPhase(_) => 0.0,
            Self::OscUnison(_) => 2.0,
            Self::OscDetune(_) => 8.0,
            Self::Noise => 0.0,
            Self::FilterEnabled(_) => 0.0,
            Self::FilterKind(_) => 1.0,
            Self::FilterCutoff(i) => {
                if i == 0 {
                    4_000.0
                } else {
                    8_000.0
                }
            }
            Self::FilterResonance(_) => 0.15,
            Self::FilterDrive(_) => 0.0,
            Self::FilterKeytrack(_) => 0.35,
            Self::SerialFilters => 1.0,
            Self::EnvAttack(_) => 0.005,
            Self::EnvDecay(_) => 0.35,
            Self::EnvSustain(_) => 0.6,
            Self::EnvRelease(_) => 0.3,
            Self::EnvAmount => 0.45,
            Self::LfoShape(_) => 0.0,
            Self::LfoRate(_) => 3.2,
            Self::LfoDepth(_) => 1.0,
            Self::ModSource(_) | Self::ModDest(_) | Self::ModAmount(_) => 0.0,
            Self::Volume => 0.8,
            Self::Glide => 0.0,
            Self::Polyphony => 16.0,
        }
    }
}

impl SynthXt {
    pub fn get(&self, param: SynthParam) -> f32 {
        match param {
            SynthParam::OscWave(i) => index_of(&Waveform::ALL, self.osc(i).wave),
            SynthParam::OscTable(i) => self.osc(i).table,
            SynthParam::OscMorph(i) => self.osc(i).morph,
            SynthParam::OscOctave(i) => self.osc(i).octave,
            SynthParam::OscSemitone(i) => self.osc(i).semitone,
            SynthParam::OscFine(i) => self.osc(i).fine,
            SynthParam::OscLevel(i) => self.osc(i).level,
            SynthParam::OscPan(i) => self.osc(i).pan,
            SynthParam::OscUnison(i) => self.osc(i).unison,
            SynthParam::OscDetune(i) => self.osc(i).detune,
            SynthParam::OscPhase(i) => self.osc(i).phase,
            SynthParam::Noise => self.noise,
            SynthParam::FilterEnabled(i) => {
                if self.filter(i).enabled {
                    1.0
                } else {
                    0.0
                }
            }
            SynthParam::FilterKind(i) => index_of(&FilterKind::ALL, self.filter(i).kind),
            SynthParam::FilterCutoff(i) => self.filter(i).cutoff,
            SynthParam::FilterResonance(i) => self.filter(i).resonance,
            SynthParam::FilterDrive(i) => self.filter(i).drive,
            SynthParam::FilterKeytrack(i) => self.filter(i).keytrack,
            SynthParam::SerialFilters => {
                if self.serial_filters {
                    1.0
                } else {
                    0.0
                }
            }
            SynthParam::EnvAttack(i) => self.env[i as usize % ENV_COUNT].attack,
            SynthParam::EnvDecay(i) => self.env[i as usize % ENV_COUNT].decay,
            SynthParam::EnvSustain(i) => self.env[i as usize % ENV_COUNT].sustain,
            SynthParam::EnvRelease(i) => self.env[i as usize % ENV_COUNT].release,
            SynthParam::EnvAmount => self.env_amount,
            SynthParam::LfoShape(i) => {
                index_of(&LfoShape::ALL, self.lfo[i as usize % LFO_COUNT].shape)
            }
            SynthParam::LfoRate(i) => self.lfo[i as usize % LFO_COUNT].rate,
            SynthParam::LfoDepth(i) => self.lfo[i as usize % LFO_COUNT].depth,
            SynthParam::ModSource(i) => index_of(&ModSource::ALL, self.route(i).source),
            SynthParam::ModDest(i) => index_of(&ModDest::ALL, self.route(i).dest),
            SynthParam::ModAmount(i) => self.route(i).amount,
            SynthParam::Volume => self.volume,
            SynthParam::Glide => self.glide,
            SynthParam::Polyphony => self.polyphony,
        }
    }

    /// Set a parameter, clamped to the range the engine accepts.
    pub fn set(&mut self, param: SynthParam, value: f32) {
        if !value.is_finite() {
            return;
        }
        let (min, max) = param.range();
        let value = value.clamp(min, max);
        match param {
            SynthParam::OscWave(i) => self.osc_mut(i).wave = Waveform::from_index(value),
            SynthParam::OscTable(i) => self.osc_mut(i).table = value.round(),
            SynthParam::OscMorph(i) => self.osc_mut(i).morph = value,
            SynthParam::OscOctave(i) => self.osc_mut(i).octave = value.round(),
            SynthParam::OscSemitone(i) => self.osc_mut(i).semitone = value.round(),
            SynthParam::OscFine(i) => self.osc_mut(i).fine = value,
            SynthParam::OscLevel(i) => self.osc_mut(i).level = value,
            SynthParam::OscPan(i) => self.osc_mut(i).pan = value,
            SynthParam::OscUnison(i) => self.osc_mut(i).unison = value.round(),
            SynthParam::OscDetune(i) => self.osc_mut(i).detune = value,
            SynthParam::OscPhase(i) => self.osc_mut(i).phase = value,
            SynthParam::Noise => self.noise = value,
            SynthParam::FilterEnabled(i) => self.filter_mut(i).enabled = value >= 0.5,
            SynthParam::FilterKind(i) => self.filter_mut(i).kind = FilterKind::from_index(value),
            SynthParam::FilterCutoff(i) => self.filter_mut(i).cutoff = value,
            SynthParam::FilterResonance(i) => self.filter_mut(i).resonance = value,
            SynthParam::FilterDrive(i) => self.filter_mut(i).drive = value,
            SynthParam::FilterKeytrack(i) => self.filter_mut(i).keytrack = value,
            SynthParam::SerialFilters => self.serial_filters = value >= 0.5,
            SynthParam::EnvAttack(i) => self.env_mut(i).attack = value,
            SynthParam::EnvDecay(i) => self.env_mut(i).decay = value,
            SynthParam::EnvSustain(i) => self.env_mut(i).sustain = value,
            SynthParam::EnvRelease(i) => self.env_mut(i).release = value,
            SynthParam::EnvAmount => self.env_amount = value,
            SynthParam::LfoShape(i) => self.lfo_mut(i).shape = LfoShape::from_index(value),
            SynthParam::LfoRate(i) => self.lfo_mut(i).rate = value,
            SynthParam::LfoDepth(i) => self.lfo_mut(i).depth = value,
            SynthParam::ModSource(i) => self.route_mut(i).source = ModSource::from_index(value),
            SynthParam::ModDest(i) => self.route_mut(i).dest = ModDest::from_index(value),
            SynthParam::ModAmount(i) => self.route_mut(i).amount = value,
            SynthParam::Volume => self.volume = value,
            SynthParam::Glide => self.glide = value,
            SynthParam::Polyphony => self.polyphony = value.round(),
        }
    }

    fn osc(&self, index: u8) -> &OscParams {
        &self.osc[(index as usize).min(OSC_COUNT - 1)]
    }

    fn osc_mut(&mut self, index: u8) -> &mut OscParams {
        &mut self.osc[(index as usize).min(OSC_COUNT - 1)]
    }

    fn filter(&self, index: u8) -> &FilterParams {
        &self.filter[(index as usize).min(FILTER_COUNT - 1)]
    }

    fn filter_mut(&mut self, index: u8) -> &mut FilterParams {
        &mut self.filter[(index as usize).min(FILTER_COUNT - 1)]
    }

    fn env_mut(&mut self, index: u8) -> &mut EnvParams {
        &mut self.env[(index as usize).min(ENV_COUNT - 1)]
    }

    fn lfo_mut(&mut self, index: u8) -> &mut LfoParams {
        &mut self.lfo[(index as usize).min(LFO_COUNT - 1)]
    }

    fn route(&self, index: u8) -> &ModRoute {
        &self.matrix[(index as usize).min(MOD_SLOTS - 1)]
    }

    fn route_mut(&mut self, index: u8) -> &mut ModRoute {
        &mut self.matrix[(index as usize).min(MOD_SLOTS - 1)]
    }

    /// Flat list of (stable id, value) for project files.
    pub fn to_saved(&self) -> Vec<(String, f32)> {
        SynthParam::all()
            .into_iter()
            .map(|param| (param.id(), self.get(param)))
            .collect()
    }

    /// Apply a saved list on top of the current patch.
    pub fn apply_saved(&mut self, saved: &[(String, f32)]) {
        for (id, value) in saved {
            if let Some(param) = SynthParam::from_id(id) {
                self.set(param, *value);
            }
        }
    }

    /// Load one of the factory patches.
    pub fn load_preset(&mut self, index: usize) {
        *self = Self::default();
        let index = index % PRESETS.len();
        *self = PRESETS[index].patch();
    }

    pub fn preset_name(index: usize) -> &'static str {
        PRESETS[index % PRESETS.len()].name
    }

    /// Default patch for a track: the demo set walks through the presets.
    pub fn from_track(track: usize) -> Self {
        let mut patch = Self::default();
        patch.load_preset(track);
        patch
    }
}

fn index_of<T: PartialEq + Copy>(list: &[T], value: T) -> f32 {
    list.iter().position(|item| *item == value).unwrap_or(0) as f32
}

/// One factory patch: the default patch plus explicit overrides.
pub struct Preset {
    pub name: &'static str,
    overrides: &'static [(SynthParam, f32)],
}

impl Preset {
    pub fn patch(&self) -> SynthXt {
        let mut patch = SynthXt::default();
        for (param, value) in self.overrides {
            patch.set(*param, *value);
        }
        patch
    }
}

pub const PRESETS: [Preset; 16] = [
    Preset {
        name: "SYNAPSE LEAD",
        overrides: &[
            (SynthParam::OscWave(0), 2.0),
            (SynthParam::OscUnison(0), 3.0),
            (SynthParam::OscDetune(0), 12.0),
            (SynthParam::OscLevel(0), 0.8),
            (SynthParam::OscWave(1), 3.0),
            (SynthParam::OscLevel(1), 0.3),
            (SynthParam::OscPan(1), 0.25),
            (SynthParam::FilterCutoff(0), 3_600.0),
            (SynthParam::FilterResonance(0), 0.3),
            (SynthParam::EnvAttack(0), 0.004),
            (SynthParam::EnvSustain(0), 0.75),
            (SynthParam::EnvRelease(0), 0.22),
            (SynthParam::EnvAmount, 0.35),
            (SynthParam::LfoRate(0), 5.4),
            (SynthParam::ModSource(0), 1.0),
            (SynthParam::ModDest(0), 3.0),
            (SynthParam::ModAmount(0), 0.08),
        ],
    },
    Preset {
        name: "BIOLINK PLUCK",
        overrides: &[
            (SynthParam::OscWave(0), 3.0),
            (SynthParam::OscUnison(0), 2.0),
            (SynthParam::OscDetune(0), 6.0),
            (SynthParam::OscLevel(0), 0.7),
            (SynthParam::OscWave(1), 2.0),
            (SynthParam::OscOctave(1), 1.0),
            (SynthParam::OscLevel(1), 0.25),
            (SynthParam::FilterCutoff(0), 2_600.0),
            (SynthParam::FilterResonance(0), 0.35),
            (SynthParam::FilterEnabled(1), 1.0),
            (SynthParam::FilterCutoff(1), 6_000.0),
            (SynthParam::EnvAttack(0), 0.002),
            (SynthParam::EnvDecay(0), 0.18),
            (SynthParam::EnvSustain(0), 0.05),
            (SynthParam::EnvRelease(0), 0.16),
            (SynthParam::EnvAmount, 0.7),
        ],
    },
    Preset {
        name: "NEURAL BASS",
        overrides: &[
            (SynthParam::OscWave(0), 2.0),
            (SynthParam::OscOctave(0), -1.0),
            (SynthParam::OscUnison(0), 2.0),
            (SynthParam::OscDetune(0), 3.0),
            (SynthParam::OscLevel(0), 0.85),
            (SynthParam::OscWave(1), 0.0),
            (SynthParam::OscOctave(1), -2.0),
            (SynthParam::OscLevel(1), 0.55),
            (SynthParam::FilterCutoff(0), 620.0),
            (SynthParam::FilterResonance(0), 0.28),
            (SynthParam::FilterDrive(0), 0.25),
            (SynthParam::EnvAttack(0), 0.003),
            (SynthParam::EnvDecay(0), 0.28),
            (SynthParam::EnvSustain(0), 0.5),
            (SynthParam::EnvRelease(0), 0.14),
            (SynthParam::EnvAmount, 0.6),
            (SynthParam::Polyphony, 8.0),
        ],
    },
    Preset {
        name: "SUB QUANT",
        overrides: &[
            (SynthParam::OscWave(0), 0.0),
            (SynthParam::OscOctave(0), -1.0),
            (SynthParam::OscUnison(0), 1.0),
            (SynthParam::OscLevel(0), 0.95),
            (SynthParam::OscWave(1), 1.0),
            (SynthParam::OscOctave(1), -2.0),
            (SynthParam::OscLevel(1), 0.4),
            (SynthParam::FilterCutoff(0), 420.0),
            (SynthParam::FilterResonance(0), 0.1),
            (SynthParam::EnvAttack(0), 0.02),
            (SynthParam::EnvDecay(0), 0.6),
            (SynthParam::EnvSustain(0), 0.85),
            (SynthParam::EnvRelease(0), 0.35),
            (SynthParam::Glide, 0.08),
            (SynthParam::Polyphony, 4.0),
        ],
    },
    Preset {
        name: "AI VOICELET",
        overrides: &[
            (SynthParam::OscWave(0), 1.0),
            (SynthParam::OscLevel(0), 0.55),
            (SynthParam::OscWave(1), 4.0),
            (SynthParam::OscSemitone(1), 7.0),
            (SynthParam::OscLevel(1), 0.4),
            (SynthParam::Noise, 0.05),
            (SynthParam::FilterKind(0), 4.0),
            (SynthParam::FilterCutoff(0), 1_400.0),
            (SynthParam::FilterResonance(0), 0.55),
            (SynthParam::FilterEnabled(1), 1.0),
            (SynthParam::FilterCutoff(1), 3_200.0),
            (SynthParam::EnvAttack(0), 0.12),
            (SynthParam::EnvDecay(0), 0.8),
            (SynthParam::EnvSustain(0), 0.7),
            (SynthParam::EnvRelease(0), 0.9),
            (SynthParam::EnvAmount, 0.25),
            (SynthParam::LfoRate(0), 5.1),
            (SynthParam::ModSource(0), 1.0),
            (SynthParam::ModDest(0), 3.0),
            (SynthParam::ModAmount(0), 0.22),
        ],
    },
    Preset {
        name: "QUANTUM PAD",
        overrides: &[
            (SynthParam::OscWave(0), 2.0),
            (SynthParam::OscUnison(0), 4.0),
            (SynthParam::OscDetune(0), 22.0),
            (SynthParam::OscLevel(0), 0.7),
            (SynthParam::OscWave(1), 1.0),
            (SynthParam::OscOctave(1), 1.0),
            (SynthParam::OscUnison(1), 2.0),
            (SynthParam::OscDetune(1), 9.0),
            (SynthParam::OscLevel(1), 0.3),
            (SynthParam::FilterCutoff(0), 1_200.0),
            (SynthParam::FilterResonance(0), 0.2),
            (SynthParam::FilterEnabled(1), 1.0),
            (SynthParam::FilterCutoff(1), 2_400.0),
            (SynthParam::EnvAttack(0), 0.8),
            (SynthParam::EnvDecay(0), 2.5),
            (SynthParam::EnvSustain(0), 0.8),
            (SynthParam::EnvRelease(0), 2.2),
            (SynthParam::LfoRate(0), 0.18),
            (SynthParam::ModSource(0), 1.0),
            (SynthParam::ModDest(0), 3.0),
            (SynthParam::ModAmount(0), 0.35),
            (SynthParam::Volume, 0.7),
        ],
    },
    Preset {
        name: "GHOSTWIRE",
        overrides: &[
            (SynthParam::OscWave(0), 0.0),
            (SynthParam::OscLevel(0), 0.8),
            (SynthParam::OscWave(1), 3.0),
            (SynthParam::OscFine(1), -7.0),
            (SynthParam::OscLevel(1), 0.3),
            (SynthParam::FilterKind(0), 5.0),
            (SynthParam::FilterCutoff(0), 1_800.0),
            (SynthParam::FilterResonance(0), 0.4),
            (SynthParam::EnvAttack(0), 0.05),
            (SynthParam::EnvRelease(0), 1.4),
            (SynthParam::EnvSustain(0), 0.65),
            (SynthParam::LfoRate(0), 6.2),
            (SynthParam::ModSource(0), 1.0),
            (SynthParam::ModDest(0), 10.0),
            (SynthParam::ModAmount(0), 0.4),
            (SynthParam::Volume, 0.7),
        ],
    },
    Preset {
        name: "MIRROR CORE",
        overrides: &[
            (SynthParam::OscWave(0), 4.0),
            (SynthParam::OscLevel(0), 0.75),
            (SynthParam::OscWave(1), 2.0),
            (SynthParam::OscSemitone(1), -12.0),
            (SynthParam::OscLevel(1), 0.35),
            (SynthParam::FilterKind(0), 2.0),
            (SynthParam::FilterCutoff(0), 700.0),
            (SynthParam::FilterResonance(0), 0.45),
            (SynthParam::EnvAttack(0), 0.006),
            (SynthParam::EnvDecay(0), 0.25),
            (SynthParam::EnvSustain(0), 0.3),
            (SynthParam::EnvRelease(0), 0.3),
            (SynthParam::LfoShape(0), 3.0),
            (SynthParam::LfoRate(0), 8.0),
            (SynthParam::ModSource(0), 1.0),
            (SynthParam::ModDest(0), 8.0),
            (SynthParam::ModAmount(0), 0.35),
        ],
    },
    Preset {
        name: "NEON PLUCK",
        overrides: &[
            (SynthParam::OscWave(0), 1.0),
            (SynthParam::OscUnison(0), 3.0),
            (SynthParam::OscDetune(0), 17.0),
            (SynthParam::OscLevel(0), 0.8),
            (SynthParam::OscWave(1), 0.0),
            (SynthParam::OscOctave(1), 2.0),
            (SynthParam::OscLevel(1), 0.12),
            (SynthParam::FilterCutoff(0), 5_200.0),
            (SynthParam::FilterResonance(0), 0.22),
            (SynthParam::EnvAttack(0), 0.001),
            (SynthParam::EnvDecay(0), 0.14),
            (SynthParam::EnvSustain(0), 0.02),
            (SynthParam::EnvRelease(0), 0.12),
            (SynthParam::EnvAmount, 0.5),
        ],
    },
    Preset {
        name: "VOID DRONE",
        overrides: &[
            (SynthParam::OscWave(0), 2.0),
            (SynthParam::OscOctave(0), -1.0),
            (SynthParam::OscUnison(0), 3.0),
            (SynthParam::OscDetune(0), 26.0),
            (SynthParam::OscLevel(0), 0.6),
            (SynthParam::OscWave(1), 5.0),
            (SynthParam::OscLevel(1), 0.18),
            (SynthParam::Noise, 0.12),
            (SynthParam::FilterCutoff(0), 820.0),
            (SynthParam::FilterResonance(0), 0.3),
            (SynthParam::FilterDrive(0), 0.3),
            (SynthParam::EnvAttack(0), 1.2),
            (SynthParam::EnvDecay(0), 3.0),
            (SynthParam::EnvSustain(0), 0.9),
            (SynthParam::EnvRelease(0), 3.0),
            (SynthParam::LfoRate(0), 0.09),
            (SynthParam::ModSource(0), 1.0),
            (SynthParam::ModDest(0), 4.0),
            (SynthParam::ModAmount(0), 0.3),
            (SynthParam::Volume, 0.65),
        ],
    },
    Preset {
        name: "PULSE ENGINE",
        overrides: &[
            (SynthParam::OscWave(0), 4.0),
            (SynthParam::OscLevel(0), 0.8),
            (SynthParam::OscWave(1), 2.0),
            (SynthParam::OscFine(1), 12.0),
            (SynthParam::OscLevel(1), 0.3),
            (SynthParam::FilterCutoff(0), 1_500.0),
            (SynthParam::FilterResonance(0), 0.5),
            (SynthParam::EnvAttack(0), 0.004),
            (SynthParam::EnvDecay(0), 0.3),
            (SynthParam::EnvSustain(0), 0.45),
            (SynthParam::EnvRelease(0), 0.2),
            (SynthParam::EnvAmount, 0.85),
            (SynthParam::EnvAttack(1), 0.002),
            (SynthParam::EnvDecay(1), 0.25),
            (SynthParam::EnvSustain(1), 0.2),
        ],
    },
    Preset {
        name: "CHROME BELL",
        overrides: &[
            (SynthParam::OscWave(0), 0.0),
            (SynthParam::OscLevel(0), 0.7),
            (SynthParam::OscWave(1), 0.0),
            (SynthParam::OscOctave(1), 1.0),
            (SynthParam::OscSemitone(1), 4.0),
            (SynthParam::OscLevel(1), 0.35),
            (SynthParam::FilterKind(0), 4.0),
            (SynthParam::FilterCutoff(0), 3_000.0),
            (SynthParam::FilterResonance(0), 0.2),
            (SynthParam::EnvAttack(0), 0.001),
            (SynthParam::EnvDecay(0), 1.1),
            (SynthParam::EnvSustain(0), 0.0),
            (SynthParam::EnvRelease(0), 1.2),
            (SynthParam::EnvAmount, 0.4),
        ],
    },
    Preset {
        name: "DATA STREAM",
        overrides: &[
            (SynthParam::OscWave(0), 3.0),
            (SynthParam::OscLevel(0), 0.7),
            (SynthParam::FilterCutoff(0), 2_200.0),
            (SynthParam::FilterResonance(0), 0.35),
            (SynthParam::EnvAttack(0), 0.002),
            (SynthParam::EnvDecay(0), 0.12),
            (SynthParam::EnvSustain(0), 0.15),
            (SynthParam::EnvRelease(0), 0.1),
            (SynthParam::LfoShape(0), 4.0),
            (SynthParam::LfoRate(0), 12.0),
            (SynthParam::ModSource(0), 1.0),
            (SynthParam::ModDest(0), 1.0),
            (SynthParam::ModAmount(0), 0.5),
            (SynthParam::Polyphony, 8.0),
        ],
    },
    Preset {
        name: "SOLAR WIND",
        overrides: &[
            (SynthParam::OscWave(0), 2.0),
            (SynthParam::OscUnison(0), 5.0),
            (SynthParam::OscDetune(0), 34.0),
            (SynthParam::OscLevel(0), 0.65),
            (SynthParam::OscWave(1), 2.0),
            (SynthParam::OscOctave(1), 1.0),
            (SynthParam::OscFine(1), -9.0),
            (SynthParam::OscLevel(1), 0.3),
            (SynthParam::FilterKind(0), 3.0),
            (SynthParam::FilterCutoff(0), 900.0),
            (SynthParam::FilterResonance(0), 0.25),
            (SynthParam::FilterEnabled(1), 1.0),
            (SynthParam::FilterCutoff(1), 4_000.0),
            (SynthParam::EnvAttack(0), 0.6),
            (SynthParam::EnvRelease(0), 1.8),
            (SynthParam::EnvSustain(0), 0.85),
            (SynthParam::LfoRate(0), 0.12),
            (SynthParam::ModSource(0), 1.0),
            (SynthParam::ModDest(0), 3.0),
            (SynthParam::ModAmount(0), 0.4),
            (SynthParam::Volume, 0.62),
        ],
    },
    Preset {
        name: "SIGNAL LOSS",
        overrides: &[
            (SynthParam::OscWave(0), 5.0),
            (SynthParam::OscLevel(0), 0.45),
            (SynthParam::OscWave(1), 0.0),
            (SynthParam::OscLevel(1), 0.5),
            (SynthParam::Noise, 0.3),
            (SynthParam::FilterKind(0), 5.0),
            (SynthParam::FilterCutoff(0), 900.0),
            (SynthParam::FilterResonance(0), 0.6),
            (SynthParam::EnvAttack(0), 0.03),
            (SynthParam::EnvRelease(0), 0.6),
            (SynthParam::EnvSustain(0), 0.5),
            (SynthParam::LfoShape(0), 4.0),
            (SynthParam::LfoRate(0), 7.5),
            (SynthParam::ModSource(0), 1.0),
            (SynthParam::ModDest(0), 4.0),
            (SynthParam::ModAmount(0), 0.6),
            (SynthParam::Volume, 0.55),
        ],
    },
    Preset {
        name: "FINAL FORM",
        overrides: &[
            (SynthParam::OscWave(0), 2.0),
            (SynthParam::OscUnison(0), 4.0),
            (SynthParam::OscDetune(0), 18.0),
            (SynthParam::OscLevel(0), 0.75),
            (SynthParam::OscWave(1), 4.0),
            (SynthParam::OscOctave(1), -1.0),
            (SynthParam::OscLevel(1), 0.4),
            (SynthParam::FilterKind(0), 1.0),
            (SynthParam::FilterCutoff(0), 2_400.0),
            (SynthParam::FilterResonance(0), 0.3),
            (SynthParam::FilterKind(1), 2.0),
            (SynthParam::FilterEnabled(1), 1.0),
            (SynthParam::FilterCutoff(1), 260.0),
            (SynthParam::EnvAttack(0), 0.02),
            (SynthParam::EnvDecay(0), 0.9),
            (SynthParam::EnvSustain(0), 0.6),
            (SynthParam::EnvRelease(0), 0.7),
            (SynthParam::EnvAmount, 0.4),
            (SynthParam::LfoRate(0), 0.25),
            (SynthParam::LfoDepth(0), 0.8),
            (SynthParam::ModSource(0), 1.0),
            (SynthParam::ModDest(0), 4.0),
            (SynthParam::ModAmount(0), -0.35),
            (SynthParam::ModSource(1), 5.0),
            (SynthParam::ModDest(1), 3.0),
            (SynthParam::ModAmount(1), 0.3),
            (SynthParam::Volume, 0.68),
        ],
    },
];

/// Envelope stage, mirrored from the amplitude envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Off,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// One ADSR envelope with exponential decay and release.
#[derive(Clone, Copy, Debug)]
struct Env {
    stage: Stage,
    level: f32,
    attack_inc: f32,
    decay_coef: f32,
    sustain: f32,
    release_coef: f32,
}

impl Default for Env {
    fn default() -> Self {
        Self {
            stage: Stage::Off,
            level: 0.0,
            attack_inc: 1.0,
            decay_coef: 0.999,
            sustain: 0.0,
            release_coef: 0.999,
        }
    }
}

impl Env {
    fn trigger(&mut self, params: &EnvParams, sample_rate: f32) {
        self.stage = Stage::Attack;
        self.level = 0.0;
        self.sustain = params.sustain.clamp(0.0, 1.0);
        self.attack_inc = 1.0 / (params.attack.max(0.0005) * sample_rate);
        self.decay_coef = envelope_coeff(self.sustain.max(0.0001), params.decay, sample_rate);
        self.release_coef = envelope_coeff(0.0001, params.release, sample_rate);
    }

    fn release(&mut self) {
        if self.stage != Stage::Off && self.stage != Stage::Release {
            self.stage = Stage::Release;
        }
    }

    #[inline]
    fn tick(&mut self) -> f32 {
        match self.stage {
            Stage::Attack => {
                self.level += self.attack_inc;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level *= self.decay_coef;
                if self.sustain <= 0.00011 && self.level < 0.0005 {
                    self.level = 0.0;
                    self.stage = Stage::Off;
                } else if self.level <= self.sustain {
                    self.level = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {
                self.level = self.sustain;
            }
            Stage::Release => {
                self.level *= self.release_coef;
                if self.level < 0.0001 {
                    self.level = 0.0;
                    self.stage = Stage::Off;
                }
            }
            Stage::Off => {
                self.level = 0.0;
            }
        }
        self.level
    }
}

#[inline]
fn envelope_coeff(target: f32, time_secs: f32, sample_rate: f32) -> f32 {
    let samples = (time_secs.max(0.0005) * sample_rate).max(1.0);
    (target.ln() / samples).exp()
}

/// Topology-preserving state variable filter (Zavalishin / Simper).
#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl Svf {
    fn set(&mut self, cutoff: f32, resonance: f32, sample_rate: f32) {
        let cutoff = cutoff.clamp(20.0, sample_rate * 0.45);
        self.g = (PI * cutoff / sample_rate).tan();
        self.k = 2.0 - 1.92 * resonance.clamp(0.0, 0.99);
        self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        self.a2 = self.g * self.a1;
        self.a3 = self.g * self.a2;
    }

    #[inline]
    fn process(&mut self, input: f32) -> (f32, f32, f32) {
        let v3 = input - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        if !self.ic1.is_finite() || !self.ic2.is_finite() {
            self.ic1 = 0.0;
            self.ic2 = 0.0;
        }
        (v2, v1, input - self.k * v1 - v2)
    }
}

/// One two-stage filter (12 or 24 dB depending on the shape).
#[derive(Clone, Copy, Debug, Default)]
struct FilterPair {
    stages: [Svf; 2],
}

impl FilterPair {
    fn set(&mut self, kind: FilterKind, cutoff: f32, resonance: f32, sample_rate: f32) {
        let q = match kind {
            FilterKind::LowPass24
            | FilterKind::HighPass24
            | FilterKind::LowPass12
            | FilterKind::HighPass12 => resonance,
            _ => resonance * 0.85,
        };
        self.stages[0].set(cutoff, q, sample_rate);
        self.stages[1].set(cutoff, q, sample_rate);
    }

    #[inline]
    fn process(&mut self, kind: FilterKind, input: f32) -> f32 {
        match kind {
            FilterKind::LowPass12 => self.stages[0].process(input).0,
            FilterKind::HighPass12 => self.stages[0].process(input).2,
            FilterKind::BandPass => self.stages[0].process(input).1,
            FilterKind::Notch => {
                let (lp, _, hp) = self.stages[0].process(input);
                lp + hp
            }
            FilterKind::LowPass24 => {
                let first = self.stages[0].process(input).0;
                self.stages[1].process(first).0
            }
            FilterKind::HighPass24 => {
                let first = self.stages[0].process(input).2;
                self.stages[1].process(first).2
            }
        }
    }
}

/// Control-rate interval for filter coefficients.
const CONTROL_INTERVAL: u8 = 16;

/// One synthesizer voice. Fixed-size and `Copy`, so the engine keeps voices in
/// a preallocated pool and never allocates from the audio callback.
#[derive(Clone, Copy, Debug)]
pub struct Voice {
    pub stage: Stage,
    pub track: u8,
    pub note: u8,
    velocity: f32,
    freq: f32,
    target_freq: f32,
    glide_coef: f32,
    phases: [[f32; MAX_UNISON]; OSC_COUNT],
    env: [Env; ENV_COUNT],
    lfo_phase: [f32; LFO_COUNT],
    lfo_hold: [f32; LFO_COUNT],
    filters: [FilterPair; FILTER_COUNT],
    control_left: u8,
    cutoff: [f32; FILTER_COUNT],
    resonance: [f32; FILTER_COUNT],
    /// Equal-power pan gains per oscillator and unison voice, refreshed at
    /// control rate instead of running sin/cos every sample.
    pan: [[(f32, f32); MAX_UNISON]; OSC_COUNT],
    /// Frequency multiplier per oscillator and unison voice (tuning, pitch
    /// modulation and detune), so the sample loop only multiplies.
    ratio: [[f32; MAX_UNISON]; OSC_COUNT],
    voice_pan: (f32, f32),
    drive: f32,
    keytrack: f32,
    random: f32,
    rng: u32,
    age: u64,
    gate_left: u32,
    active: bool,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            stage: Stage::Off,
            track: 0,
            note: 0,
            velocity: 0.0,
            freq: 0.0,
            target_freq: 440.0,
            glide_coef: 1.0,
            phases: [[0.0; MAX_UNISON]; OSC_COUNT],
            env: [Env::default(); ENV_COUNT],
            lfo_phase: [0.0; LFO_COUNT],
            lfo_hold: [0.0; LFO_COUNT],
            filters: [FilterPair::default(); FILTER_COUNT],
            control_left: 0,
            cutoff: [1_000.0; FILTER_COUNT],
            resonance: [0.0; FILTER_COUNT],
            pan: [[(0.707, 0.707); MAX_UNISON]; OSC_COUNT],
            ratio: [[1.0; MAX_UNISON]; OSC_COUNT],
            voice_pan: (0.707, 0.707),
            drive: 0.0,
            keytrack: 0.0,
            random: 0.0,
            rng: 0x1234_5678,
            age: 0,
            gate_left: 0,
            active: false,
        }
    }
}

impl Voice {
    #[allow(clippy::too_many_arguments)]
    pub fn trigger(
        &mut self,
        patch: &SynthXt,
        track: usize,
        note: u8,
        velocity: f32,
        gate_samples: u32,
        sample_rate: f32,
        age: u64,
    ) {
        self.stage = Stage::Attack;
        self.track = track as u8;
        self.note = note;
        self.velocity = velocity.clamp(0.0, 1.0);
        self.target_freq = midi_to_hz(note as f32);
        if self.freq <= 1.0 || patch.glide <= 0.0001 {
            self.freq = self.target_freq;
            self.glide_coef = 1.0;
        } else {
            // Time constant that reaches the target in roughly `glide`.
            self.glide_coef = 1.0 - (-1.0 / (patch.glide.max(0.0005) * sample_rate)).exp();
        }
        for osc in 0..OSC_COUNT {
            for unison in 0..MAX_UNISON {
                self.phases[osc][unison] = patch.osc[osc].phase.fract();
            }
        }
        for (index, env) in self.env.iter_mut().enumerate() {
            env.trigger(&patch.env[index], sample_rate);
        }
        self.lfo_phase = [0.0; LFO_COUNT];
        self.lfo_hold = [0.0; LFO_COUNT];
        self.filters = [FilterPair::default(); FILTER_COUNT];
        self.control_left = 0;
        self.pan = [[(0.707, 0.707); MAX_UNISON]; OSC_COUNT];
        self.ratio = [[1.0; MAX_UNISON]; OSC_COUNT];
        self.voice_pan = (0.707, 0.707);
        self.keytrack = (note as f32 - 60.0) / 48.0;
        self.age = age;
        self.gate_left = gate_samples.max(1);
        self.active = true;
        self.rng = 0x9E37_79B9
            ^ (note as u32).wrapping_mul(2_654_435_761)
            ^ (age as u32).wrapping_mul(40_503);
        self.random = self.next_random();
    }

    #[inline]
    pub fn is_active(&self) -> bool {
        self.active && self.stage != Stage::Off
    }

    /// Envelope level, used to choose the best voice to steal.
    #[inline]
    pub fn envelope_level(&self) -> f32 {
        self.env[ENV_AMP].level
    }

    #[inline]
    pub fn age(&self) -> u64 {
        self.age
    }

    #[inline]
    pub fn note(&self) -> u8 {
        self.note
    }

    pub fn release(&mut self) {
        for env in &mut self.env {
            env.release();
        }
    }

    #[inline]
    fn next_random(&mut self) -> f32 {
        // xorshift32: cheap, deterministic, good enough for noise and S&H.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Produce one stereo sample and advance the voice.
    #[inline]
    pub fn next_sample(&mut self, patch: &SynthXt, sample_rate: f32) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }

        if self.gate_left > 0 {
            self.gate_left -= 1;
            if self.gate_left == 0 {
                self.release();
            }
        }

        let amp = self.env[ENV_AMP].tick();
        self.stage = self.env[ENV_AMP].stage;
        if self.stage == Stage::Off {
            self.active = false;
            return (0.0, 0.0);
        }
        let filter_env = self.env[ENV_FILTER].tick();
        let mod_env = self.env[ENV_MOD].tick();

        // LFOs: free-running per voice, retriggered by note-on.
        let mut lfo = [0.0_f32; LFO_COUNT];
        for (index, params) in patch.lfo.iter().enumerate() {
            let phase = self.lfo_phase[index];
            let value = match params.shape {
                LfoShape::Sine => fast_sine(phase),
                LfoShape::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
                LfoShape::Saw => phase * 2.0 - 1.0,
                LfoShape::Square => {
                    if phase < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                LfoShape::Random => {
                    // A new value each cycle.
                    if phase < self.lfo_hold[index] {
                        self.lfo_hold[index] = 0.0;
                        self.random = self.next_random();
                    }
                    self.random
                }
            };
            self.lfo_hold[index] = phase;
            self.lfo_phase[index] = (phase + params.rate / sample_rate).fract();
            lfo[index] = value * params.depth;
        }

        // Modulation matrix.
        let mut mods = [0.0_f32; DEST_COUNT];
        for route in &patch.matrix {
            if route.amount.abs() < 1.0e-4 {
                continue;
            }
            let Some(slot) = route.dest.slot() else {
                continue;
            };
            let source = match route.source {
                ModSource::Off => 0.0,
                ModSource::Lfo1 => lfo[0],
                ModSource::Lfo2 => lfo[1],
                ModSource::Env2 => filter_env,
                ModSource::Env3 => mod_env,
                ModSource::Velocity => self.velocity,
                ModSource::KeyTrack => self.keytrack,
                ModSource::Random => self.random,
            };
            mods[slot] += source * route.amount;
        }

        // Glide.
        self.freq += (self.target_freq - self.freq) * self.glide_coef;

        if self.control_left == 0 {
            self.control_left = CONTROL_INTERVAL;
            for index in 0..FILTER_COUNT {
                let params = patch.filter[index];
                let octaves = mods[if index == 0 {
                    DEST_CUTOFF1
                } else {
                    DEST_CUTOFF2
                }] * 5.0
                    + self.keytrack * params.keytrack * 2.0;
                let env_octaves = if index == 0 {
                    filter_env * patch.env_amount * 4.0
                } else {
                    filter_env * patch.env_amount * 2.0
                };
                let cutoff = params.cutoff * exp2(octaves + env_octaves);
                self.cutoff[index] = cutoff.clamp(20.0, sample_rate * 0.45);
                self.resonance[index] = (params.resonance
                    + mods[if index == 0 { DEST_RES1 } else { DEST_RES2 }])
                .clamp(0.0, 0.98);
                if params.enabled {
                    self.filters[index].set(
                        params.kind,
                        self.cutoff[index],
                        self.resonance[index],
                        sample_rate,
                    );
                }
            }
            self.drive = (patch.filter[0].drive + mods[DEST_DRIVE]).clamp(0.0, 1.0);
            // Pan gains only move with the mod matrix, so control rate is
            // plenty; this keeps sin/cos out of the sample loop.
            let pitch = mods[DEST_PITCH] * 24.0;
            for (osc_index, osc) in patch.osc.iter().enumerate() {
                let unison = (osc.unison.round() as usize).clamp(1, MAX_UNISON);
                let semitones = osc.octave * 12.0
                    + osc.semitone
                    + osc.fine / 100.0
                    + pitch
                    + mods[if osc_index == 0 {
                        DEST_OSC1_PITCH
                    } else {
                        DEST_OSC2_PITCH
                    }] * 24.0;
                let base = semitone_ratio(semitones);
                for voice_index in 0..unison {
                    let spread = if unison > 1 {
                        (voice_index as f32 / (unison - 1) as f32) - 0.5
                    } else {
                        0.0
                    };
                    let pan = (osc.pan + spread * 1.2 + mods[DEST_PAN]).clamp(-1.0, 1.0);
                    self.pan[osc_index][voice_index] = crate::dsp::pan_gains(pan);
                    self.ratio[osc_index][voice_index] =
                        base * semitone_ratio(spread * 2.0 * osc.detune / 100.0);
                }
            }
            self.voice_pan = crate::dsp::pan_gains(mods[DEST_PAN].clamp(-1.0, 1.0) * 0.7);
        }
        self.control_left -= 1;

        // Oscillators, with unison and per-oscillator pan.
        let inv_rate = 1.0 / sample_rate;
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;
        for (osc_index, osc) in patch.osc.iter().enumerate() {
            let unison = (osc.unison.round() as usize).clamp(1, MAX_UNISON);
            let gain = osc.level
                + mods[if osc_index == 0 {
                    DEST_OSC1_LEVEL
                } else {
                    DEST_OSC2_LEVEL
                }];
            let gain = gain.clamp(0.0, 1.5) / (unison as f32).sqrt();
            let morph_mod = mods[if osc_index == 0 {
                DEST_OSC1_MORPH
            } else {
                DEST_OSC2_MORPH
            }];
            let effective_morph = (osc.morph + morph_mod).clamp(0.0, 1.0);
            for voice_index in 0..unison {
                let increment = self.freq * self.ratio[osc_index][voice_index] * inv_rate;
                let phase = self.phases[osc_index][voice_index];
                let value = if osc.wave == Waveform::Wavetable {
                    sample_wavetable(osc.table as usize, effective_morph, phase, increment)
                } else {
                    oscillator(osc.wave, phase, increment, &mut self.rng)
                };
                self.phases[osc_index][voice_index] = (phase + increment).fract();
                let (pan_l, pan_r) = self.pan[osc_index][voice_index];
                left += value * gain * pan_l;
                right += value * gain * pan_r;
            }
        }
        if patch.noise > 0.0 {
            let noise = self.next_random() * patch.noise;
            let (pan_l, pan_r) = self.voice_pan;
            left += noise * pan_l;
            right += noise * pan_r;
        }

        // Saturation is only paid for when the drive knob is up: `tanh` is
        // the most expensive operation in the voice.
        if self.drive > 1.0e-4 {
            let drive_gain = 1.0 + self.drive * 6.0;
            left = (left * drive_gain).tanh();
            right = (right * drive_gain).tanh();
        }

        // Filters; a bypassed filter costs nothing at all.
        let f1 = patch.filter[0];
        let f2 = patch.filter[1];
        if patch.serial_filters {
            if f1.enabled {
                left = self.filters[0].process(f1.kind, left);
                right = self.filters[0].process(f1.kind, right);
            }
            if f2.enabled {
                left = self.filters[1].process(f2.kind, left);
                right = self.filters[1].process(f2.kind, right);
            }
        } else {
            let (mut out_l, mut out_r) = (0.0, 0.0);
            let mut count = 0.0_f32;
            if f1.enabled {
                out_l += self.filters[0].process(f1.kind, left);
                out_r += self.filters[0].process(f1.kind, right);
                count += 1.0;
            }
            if f2.enabled {
                out_l += self.filters[1].process(f2.kind, left);
                out_r += self.filters[1].process(f2.kind, right);
                count += 1.0;
            }
            if count > 0.0 {
                left = out_l / count.sqrt();
                right = out_r / count.sqrt();
            }
        }

        // Amplitude, velocity and headroom.
        let envelope = amp * self.velocity * patch.volume * 0.5;
        let (pan_l, pan_r) = self.voice_pan;
        let out_l = (left * envelope * pan_l * 1.4).clamp(-4.0, 4.0);
        let out_r = (right * envelope * pan_r * 1.4).clamp(-4.0, 4.0);
        (out_l, out_r)
    }
}

/// Fast sine from a phase in `[0, 1)` using a 7th-order polynomial.
#[inline]
fn fast_sine(phase: f32) -> f32 {
    let mut x = phase.fract() * TAU;
    if x > PI {
        x -= TAU;
    }
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let mut ax = x.abs();
    if ax > FRAC_PI_2 {
        ax = PI - ax;
    }
    let x2 = ax * ax;
    sign * ax * (1.0 + x2 * (-1.0 / 6.0 + x2 * (1.0 / 120.0 - x2 / 5_040.0)))
}

/// 2^x without a libm call (musical pitch and cutoff scaling).
#[inline]
fn exp2(x: f32) -> f32 {
    // exp2(x) = 2^floor(x) * 2^frac(x). The fractional part uses the same
    // 7th-order series as `fast_sine`, which keeps the error near 1e-6.
    let x = x.clamp(-16.0, 16.0);
    let floor = x.floor();
    let frac = x - floor;
    let a = frac * std::f32::consts::LN_2;
    let a2 = a * a;
    let poly = 1.0 + a + a2 * (0.5 + a * (1.0 / 6.0 + a * (1.0 / 24.0 + a * (1.0 / 120.0))));
    poly * f32::from_bits(((floor as i32 + 127) as u32) << 23)
}

#[inline]
fn semitone_ratio(semitones: f32) -> f32 {
    exp2(semitones / 12.0)
}

/// Band-limited waveform sample. `phase` and `increment` are in `[0, 1)`.
#[inline]
fn oscillator(wave: Waveform, phase: f32, increment: f32, rng: &mut u32) -> f32 {
    let dt = increment.clamp(0.0, 0.45);
    match wave {
        Waveform::Sine => fast_sine(phase),
        Waveform::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
        Waveform::Saw => 2.0 * phase - 1.0 - poly_blep(phase, dt),
        Waveform::Square => {
            let square = if phase < 0.5 { 1.0 } else { -1.0 };
            square + poly_blep(phase, dt) - poly_blep((phase + 0.5).fract(), dt)
        }
        Waveform::Pulse => {
            let pulse = if phase < 0.25 { 1.0 } else { -1.0 };
            pulse + poly_blep(phase, dt) - poly_blep((phase + 0.75).fract(), dt)
        }
        Waveform::Noise => {
            let mut x = *rng;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *rng = x;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
        Waveform::Wavetable => sample_wavetable(0, 0.5, phase, dt),
    }
}

#[inline]
fn poly_blep(phase: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        return 0.0;
    }
    if phase < dt {
        let x = phase / dt;
        x + x - x * x - 1.0
    } else if phase > 1.0 - dt {
        let x = (phase - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sounding_voice(patch: &SynthXt, note: u8, gate: u32) -> Voice {
        let mut voice = Voice::default();
        voice.trigger(patch, 0, note, 1.0, gate, SR, 1);
        voice
    }

    #[test]
    fn every_parameter_round_trips_and_clamps() {
        let mut patch = SynthXt::default();
        for param in SynthParam::all() {
            let (min, max) = param.range();
            patch.set(param, max + 1_000.0);
            assert!((patch.get(param) - max).abs() < 1e-3, "{param:?} max");
            patch.set(param, min - 1_000.0);
            assert!((patch.get(param) - min).abs() < 1e-3, "{param:?} min");
            patch.set(param, f32::NAN);
            assert!(patch.get(param).is_finite(), "{param:?} NaN");
        }
    }

    #[test]
    fn parameter_ids_are_stable_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for param in SynthParam::all() {
            let id = param.id();
            assert!(seen.insert(id.clone()), "duplicate id {id}");
            assert_eq!(SynthParam::from_id(&id), Some(param));
        }
    }

    #[test]
    fn voice_sounds_and_releases_to_silence() {
        let patch = SynthXt::default();
        let mut voice = sounding_voice(&patch, 69, 4_800);
        let mut peak = 0.0_f32;
        for _ in 0..48_000 {
            let (l, r) = voice.next_sample(&patch, SR);
            assert!(l.is_finite() && r.is_finite());
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak > 0.05, "voice never sounded (peak {peak})");
        assert!(!voice.is_active(), "voice did not release");
    }

    #[test]
    fn every_waveform_is_finite_and_bounded() {
        for wave in Waveform::ALL {
            let mut patch = SynthXt::default();
            patch.set(SynthParam::OscWave(0), index_of(&Waveform::ALL, wave));
            patch.set(SynthParam::OscUnison(0), 3.0);
            let mut voice = sounding_voice(&patch, 60, 24_000);
            for _ in 0..24_000 {
                let (l, r) = voice.next_sample(&patch, SR);
                assert!(l.is_finite() && r.is_finite(), "{wave:?} produced {l}/{r}");
                assert!(l.abs() < 8.0 && r.abs() < 8.0, "{wave:?} ran away: {l}/{r}");
            }
        }
    }

    #[test]
    fn every_filter_stays_stable() {
        let mut patch = SynthXt::default();
        patch.set(SynthParam::FilterResonance(0), 0.98);
        patch.set(SynthParam::FilterDrive(0), 1.0);
        for kind in FilterKind::ALL {
            patch.set(SynthParam::FilterKind(0), index_of(&FilterKind::ALL, kind));
            let mut voice = sounding_voice(&patch, 45, 24_000);
            let mut peak = 0.0_f32;
            for _ in 0..24_000 {
                let (l, r) = voice.next_sample(&patch, SR);
                assert!(l.is_finite() && r.is_finite(), "{kind:?} produced {l}/{r}");
                peak = peak.max(l.abs()).max(r.abs());
            }
            assert!(peak < 8.0, "{kind:?} ran away: {peak}");
        }
    }

    #[test]
    fn oscillator_pitch_matches_the_note() {
        let mut patch = SynthXt::default();
        patch.set(SynthParam::OscWave(0), 2.0); // saw
        patch.set(SynthParam::OscUnison(0), 1.0);
        patch.set(SynthParam::OscLevel(1), 0.0);
        patch.set(SynthParam::FilterCutoff(0), 18_000.0);
        patch.set(SynthParam::FilterResonance(0), 0.0);
        patch.set(SynthParam::EnvAttack(0), 0.001);
        patch.set(SynthParam::EnvSustain(0), 1.0);
        patch.set(SynthParam::EnvDecay(0), 0.01);
        let mut voice = sounding_voice(&patch, 69, 48_000); // 440 Hz
        let mut crossings = 0_i32;
        let mut previous = 0.0;
        for _ in 0..48_000 {
            let (l, _) = voice.next_sample(&patch, SR);
            if previous <= 0.0 && l > 0.0 {
                crossings += 1;
            }
            previous = l;
        }
        assert!(
            (crossings - 440).abs() <= 3,
            "expected ~440 crossings, got {crossings}"
        );
    }

    #[test]
    fn presets_are_distinct_and_named() {
        let mut names = std::collections::HashSet::new();
        for preset in PRESETS.iter() {
            assert!(names.insert(preset.name), "duplicate preset name");
            let patch = preset.patch();
            for param in SynthParam::all() {
                assert!(patch.get(param).is_finite(), "{} {param:?}", preset.name);
            }
        }
        let lead = PRESETS[0].patch();
        let bass = PRESETS[2].patch();
        assert!(
            (lead.get(SynthParam::FilterCutoff(0)) - bass.get(SynthParam::FilterCutoff(0))).abs()
                > 100.0,
            "presets should not all be the same patch"
        );
    }

    #[test]
    fn saved_patch_round_trips() {
        let mut patch = SynthXt::default();
        patch.load_preset(5);
        patch.set(SynthParam::FilterCutoff(0), 1_234.5);
        patch.set(SynthParam::ModSource(2), 3.0);
        let saved = patch.to_saved();
        let mut restored = SynthXt::default();
        restored.apply_saved(&saved);
        assert_eq!(restored, patch);
    }

    #[test]
    fn fast_sine_is_close_to_the_real_thing() {
        let mut worst = 0.0_f32;
        for step in 0..1_000 {
            let phase = step as f32 / 1_000.0;
            let expected = (phase * TAU).sin();
            worst = worst.max((fast_sine(phase) - expected).abs());
        }
        assert!(worst < 5.0e-4, "sine approximation error {worst}");
    }

    #[test]
    fn exp2_matches_the_reference() {
        for step in -24..=24 {
            let x = step as f32 / 3.0;
            let expected = 2.0_f32.powf(x);
            let actual = exp2(x);
            assert!(
                (actual - expected).abs() / expected < 1.0e-3,
                "exp2({x}) = {actual}, expected {expected}"
            );
        }
    }
}
