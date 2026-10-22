//! Small real-time-safe DSP building blocks.

pub mod filter;
pub mod spectrum;

pub use filter::{OnePole, Svf};

use std::f32::consts::TAU;

/// Exponential smoother used to remove zipper noise from parameter changes.
#[derive(Clone, Copy, Debug)]
pub struct Smoothed {
    current: f32,
    target: f32,
    coeff: f32,
}

impl Smoothed {
    pub fn new(value: f32, sample_rate: f32, time_secs: f32) -> Self {
        Self {
            current: value,
            target: value,
            coeff: smoothing_coeff(sample_rate, time_secs),
        }
    }

    #[inline]
    pub fn set(&mut self, value: f32) {
        self.target = value;
    }

    #[inline]
    pub fn set_now(&mut self, value: f32) {
        self.current = value;
        self.target = value;
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        self.current += (self.target - self.current) * self.coeff;
        self.current
    }

    #[inline]
    pub fn current(&self) -> f32 {
        self.current
    }

    #[inline]
    pub fn target(&self) -> f32 {
        self.target
    }
}

#[inline]
pub fn smoothing_coeff(sample_rate: f32, time_secs: f32) -> f32 {
    if time_secs <= 0.0 {
        1.0
    } else {
        1.0 - (-1.0 / (sample_rate * time_secs)).exp()
    }
}

/// Musical equal-tempered frequency for a MIDI note.
#[inline]
pub fn midi_to_hz(note: f32) -> f32 {
    440.0 * ((note - 69.0) / 12.0).exp2()
}

/// Equal-power pan gains for a position in -1..=1.
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * std::f32::consts::PI;
    (angle.cos(), angle.sin())
}

/// Smooth saturation with unity gain around zero.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.72;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        let t = (a - KNEE) / (1.0 - KNEE);
        x.signum() * (KNEE + (1.0 - KNEE) * t.tanh())
    }
}

/// Cheap one-pole high-pass state, used by the speaker-protection stage.
#[derive(Clone, Copy, Debug, Default)]
pub struct DcBlocker {
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + 0.995 * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// White-ish noise from a deterministic LCG (used for visualisers only).
pub fn hash_noise(mut x: u32) -> f32 {
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// Two-operator FM sample, shared by the engine and offline tests.
#[inline]
pub fn fm_sample(phase: f32, mod_phase: f32, mod_index: f32) -> f32 {
    let modulator = (mod_phase.rem_euclid(TAU)).sin() * mod_index;
    (phase + modulator).sin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_is_equal_power() {
        for pan in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            let (l, r) = pan_gains(pan);
            let power = l * l + r * r;
            assert!((power - 1.0).abs() < 1e-5, "power at {pan} = {power}");
        }
    }

    #[test]
    fn midi_reference_is_440() {
        assert!((midi_to_hz(69.0) - 440.0).abs() < 0.001);
        assert!((midi_to_hz(81.0) - 880.0).abs() < 0.01);
    }

    #[test]
    fn smoother_converges() {
        let mut s = Smoothed::new(0.0, 48_000.0, 0.01);
        s.set(1.0);
        for _ in 0..48_000 {
            s.tick();
        }
        assert!((s.current() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn soft_clip_stays_bounded() {
        for i in -200..=200 {
            let x = i as f32 * 0.05;
            let y = soft_clip(x);
            assert!(y.abs() <= 1.0);
            assert!(y.is_finite());
        }
    }
}
