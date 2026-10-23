//! Filters used by the Quantum Resonator effect and the offline synth tests.

use std::f32::consts::PI;

/// Topology-preserving state variable filter (Zavalishin / Simper form).
///
/// Stable under fast cutoff modulation, which makes it suitable for a
/// knob that can be dragged while audio is running.
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    ic1eq: f32,
    ic2eq: f32,
}

impl Svf {
    #[inline]
    pub fn reset(&mut self) {
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
    }

    /// Process one sample. `g = tan(pi * cutoff / sample_rate)`, `k = 1/Q`.
    #[inline]
    pub fn process(&mut self, x: f32, g: f32, k: f32) -> f32 {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2eq;
        let v1 = a1 * self.ic1eq + a2 * v3;
        let v2 = self.ic2eq + a2 * self.ic1eq + a3 * v3;
        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;
        v2
    }

    /// Coefficients for a cutoff in Hz and a resonance in 0..=1.
    #[inline]
    pub fn coefficients(cutoff_hz: f32, resonance: f32, sample_rate: f32) -> (f32, f32) {
        let nyquist = sample_rate * 0.5;
        let cutoff = cutoff_hz.clamp(20.0, nyquist * 0.92);
        let g = (PI * cutoff / sample_rate).tan();
        // k = 1/Q. 2.0 is gentle, 0.12 is a screaming resonance.
        let k = 2.0 - 1.88 * resonance.clamp(0.0, 1.0);
        (g, k.max(0.12))
    }
}

/// One-pole low-pass used for the high-cut "tone" control.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    z: f32,
}

impl OnePole {
    #[inline]
    pub fn reset(&mut self) {
        self.z = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32, cutoff_hz: f32, sample_rate: f32) -> f32 {
        let cutoff = cutoff_hz.clamp(20.0, sample_rate * 0.45);
        let a = 1.0 - (-2.0 * PI * cutoff / sample_rate).exp();
        self.z += a * (x - self.z);
        self.z
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn run_sine(freq: f32, cutoff: f32, resonance: f32) -> f32 {
        let sr = 48_000.0;
        let mut filter = Svf::default();
        let (g, k) = Svf::coefficients(cutoff, resonance, sr);
        let mut peak = 0.0_f32;
        for n in 0..48_000 {
            let x = (TAU * freq * n as f32 / sr).sin();
            let y = filter.process(x, g, k);
            if n > 4_800 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn lowpass_passes_below_cutoff() {
        let peak = run_sine(200.0, 2_000.0, 0.1);
        assert!(peak > 0.8, "peak was {peak}");
    }

    #[test]
    fn lowpass_removes_above_cutoff() {
        let peak = run_sine(8_000.0, 500.0, 0.0);
        assert!(peak < 0.2, "peak was {peak}");
    }

    #[test]
    fn resonance_boosts_near_cutoff() {
        let flat = run_sine(1_000.0, 1_000.0, 0.0);
        let resonant = run_sine(1_000.0, 1_000.0, 0.9);
        assert!(resonant > flat * 1.5, "flat {flat}, resonant {resonant}");
    }

    #[test]
    fn one_pole_is_stable() {
        let mut lp = OnePole::default();
        let mut y = 0.0;
        for _ in 0..10_000 {
            y = lp.process(1.0, 500.0, 48_000.0);
        }
        assert!((y - 1.0).abs() < 0.01);
    }
}
