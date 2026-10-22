//! A polyphonic two-operator FM voice with a sample-accurate ADSR envelope.

use std::f32::consts::TAU;

use crate::audio::messages::SynthParam;
use crate::audio::tracks::TrackSpec;
use crate::dsp::midi_to_hz;

/// Editable FM synth settings for one track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SynthParams {
    pub fm_ratio: f32,
    pub mod_index: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub level: f32,
}

impl SynthParams {
    pub fn from_spec(spec: &TrackSpec) -> Self {
        Self {
            fm_ratio: spec.fm_ratio,
            mod_index: spec.mod_index,
            attack: spec.attack,
            decay: spec.decay,
            sustain: spec.sustain,
            release: spec.release,
            level: spec.level,
        }
    }

    pub fn get(&self, param: SynthParam) -> f32 {
        match param {
            SynthParam::Ratio => self.fm_ratio,
            SynthParam::Index => self.mod_index,
            SynthParam::Attack => self.attack,
            SynthParam::Decay => self.decay,
            SynthParam::Sustain => self.sustain,
            SynthParam::Release => self.release,
            SynthParam::Level => self.level,
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
            SynthParam::Ratio => self.fm_ratio = value,
            SynthParam::Index => self.mod_index = value,
            SynthParam::Attack => self.attack = value,
            SynthParam::Decay => self.decay = value,
            SynthParam::Sustain => self.sustain = value,
            SynthParam::Release => self.release = value,
            SynthParam::Level => self.level = value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Off,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// One synthesizer voice. Fixed-size and `Copy`, so the engine keeps them
/// in a preallocated pool and never allocates from the audio callback.
#[derive(Clone, Copy, Debug)]
pub struct Voice {
    pub stage: Stage,
    pub track: u8,
    pub note: u8,
    pub velocity: f32,
    freq: f32,
    phase: f32,
    mod_phase: f32,
    ratio: f32,
    mod_index: f32,
    level: f32,
    env: f32,
    gate_left: u32,
    attack_inc: f32,
    decay_coef: f32,
    sustain: f32,
    release_coef: f32,
    age: u64,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            stage: Stage::Off,
            track: 0,
            note: 0,
            velocity: 0.0,
            freq: 440.0,
            phase: 0.0,
            mod_phase: 0.0,
            ratio: 1.0,
            mod_index: 0.0,
            level: 0.0,
            env: 0.0,
            gate_left: 0,
            attack_inc: 1.0,
            decay_coef: 0.0,
            sustain: 0.0,
            release_coef: 0.0,
            age: 0,
        }
    }
}

impl Voice {
    #[allow(clippy::too_many_arguments)]
    pub fn trigger(
        &mut self,
        params: &SynthParams,
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
        self.freq = midi_to_hz(note as f32);
        self.phase = 0.0;
        self.mod_phase = 0.0;
        self.ratio = params.fm_ratio;
        self.mod_index = params.mod_index;
        self.level = params.level;
        self.env = 0.0;
        self.gate_left = gate_samples.max(1);
        self.sustain = params.sustain.clamp(0.0, 1.0);
        self.attack_inc = 1.0 / (params.attack.max(0.0005) * sample_rate);
        self.decay_coef = envelope_coeff(self.sustain.max(0.0001), params.decay, sample_rate);
        self.release_coef = envelope_coeff(0.0001, params.release, sample_rate);
        self.age = age;
    }

    #[inline]
    pub fn is_active(&self) -> bool {
        self.stage != Stage::Off
    }

    /// Envelope level, used to choose the best voice to steal.
    #[inline]
    pub fn envelope_level(&self) -> f32 {
        self.env
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
        if self.stage != Stage::Off && self.stage != Stage::Release {
            self.stage = Stage::Release;
        }
    }

    /// Produce one sample and advance the envelope and oscillators.
    #[inline]
    pub fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.stage == Stage::Off {
            return 0.0;
        }

        if self.gate_left > 0 {
            self.gate_left -= 1;
            if self.gate_left == 0 {
                self.release();
            }
        }

        match self.stage {
            Stage::Attack => {
                self.env += self.attack_inc;
                if self.env >= 1.0 {
                    self.env = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.env *= self.decay_coef;
                if self.sustain <= 0.00011 && self.env < 0.0005 {
                    self.env = 0.0;
                    self.stage = Stage::Off;
                    return 0.0;
                }
                if self.env <= self.sustain {
                    self.env = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {
                self.env = self.sustain;
            }
            Stage::Release => {
                self.env *= self.release_coef;
                if self.env < 0.0001 {
                    self.env = 0.0;
                    self.stage = Stage::Off;
                    return 0.0;
                }
            }
            Stage::Off => return 0.0,
        }

        let modulator = (self.mod_phase).sin() * self.mod_index * self.env;
        let out = (self.phase + modulator).sin() * self.env * self.level * self.velocity;

        let inc = TAU * self.freq / sample_rate;
        self.phase += inc;
        if self.phase >= TAU {
            self.phase -= TAU;
        }
        // The modulator tracks the carrier frequency ratio.
        self.mod_phase += inc * self.ratio;
        if self.mod_phase >= TAU {
            self.mod_phase %= TAU;
        }

        out
    }
}

#[inline]
fn envelope_coeff(target: f32, time_secs: f32, sample_rate: f32) -> f32 {
    let samples = (time_secs.max(0.0005) * sample_rate).max(1.0);
    (target.ln() / samples).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::tracks::TRACKS;

    const SR: f32 = 48_000.0;

    #[test]
    fn voice_attacks_and_releases_to_silence() {
        let mut voice = Voice::default();
        let params = SynthParams::from_spec(&TRACKS[0]);
        voice.trigger(&params, 0, 69, 1.0, 4_800, SR, 1);
        let mut peak = 0.0_f32;
        for _ in 0..24_000 {
            peak = peak.max(voice.next_sample(SR).abs());
        }
        assert!(peak > 0.1, "voice never sounded (peak {peak})");
        assert!(!voice.is_active(), "voice did not release");
    }

    #[test]
    fn voice_is_finite() {
        let mut voice = Voice::default();
        let params = SynthParams::from_spec(&TRACKS[4]);
        voice.trigger(&params, 4, 88, 1.0, 24_000, SR, 1);
        for _ in 0..48_000 {
            assert!(voice.next_sample(SR).is_finite());
        }
    }

    #[test]
    fn oscillator_frequency_is_accurate() {
        let mut voice = Voice::default();
        let mut params = SynthParams::from_spec(&TRACKS[2]);
        params.mod_index = 0.0;
        let note = 69; // 440 Hz
        voice.trigger(&params, 2, note, 1.0, 48_000, SR, 1);
        let mut crossings = 0_i32;
        let mut prev = 0.0;
        for _ in 0..48_000 {
            let s = voice.next_sample(SR);
            if prev <= 0.0 && s > 0.0 {
                crossings += 1;
            }
            prev = s;
        }
        // The envelope decays, but the positive-going crossings remain 440 Hz.
        assert!(
            (crossings - 440).abs() <= 2,
            "expected ~440 crossings, got {crossings}"
        );
    }

    #[test]
    fn synth_params_round_trip_and_clamp() {
        let mut params = SynthParams::from_spec(&TRACKS[0]);
        for param in SynthParam::ALL {
            let (min, max) = param.range();
            params.set(param, max + 100.0);
            assert!((params.get(param) - max).abs() < 1e-5, "{param:?} max");
            params.set(param, min - 100.0);
            assert!((params.get(param) - min).abs() < 1e-5, "{param:?} min");
        }
        params.set(SynthParam::Ratio, f32::NAN);
        assert!(params.get(SynthParam::Ratio).is_finite());
    }

    #[test]
    fn stolen_voice_reuses_pool_slot() {
        // The engine handles stealing; here we only verify that retriggering
        // resets the envelope and phase deterministically.
        let mut voice = Voice::default();
        voice.trigger(&SynthParams::from_spec(&TRACKS[0]), 0, 60, 1.0, 480, SR, 1);
        for _ in 0..200 {
            voice.next_sample(SR);
        }
        voice.trigger(&SynthParams::from_spec(&TRACKS[1]), 1, 72, 0.5, 480, SR, 2);
        assert_eq!(voice.stage, Stage::Attack);
        assert_eq!(voice.env, 0.0);
        assert_eq!(voice.note, 72);
    }
}
