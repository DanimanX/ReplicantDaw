//! Master-chain effect modules.
//!
//! Each module is self contained, fixed-size state and allocation free while
//! processing. Modules can be added, removed, reordered and bypassed at any
//! time; parameter changes are smoothed to avoid zipper noise.

use crate::audio::messages::EffectKind;
use crate::dsp::{OnePole, Smoothed, Svf};

const MAX_DELAY_SECONDS: f32 = 1.05;

pub struct EffectSlot {
    kind: Option<EffectKind>,
    params: [f32; 3],
    smooth: [Smoothed; 3],
    bypass: bool,
    svf_l: Svf,
    svf_r: Svf,
    tone_l: OnePole,
    tone_r: OnePole,
    delay: Vec<f32>,
    delay_frames: usize,
    write: usize,
    sample_rate: f32,
}

impl EffectSlot {
    pub fn new(sample_rate: f32) -> Self {
        let frames = (sample_rate * MAX_DELAY_SECONDS).ceil() as usize + 2;
        Self {
            kind: None,
            params: [0.0; 3],
            smooth: [Smoothed::new(0.0, sample_rate, 0.02); 3],
            bypass: false,
            svf_l: Svf::default(),
            svf_r: Svf::default(),
            tone_l: OnePole::default(),
            tone_r: OnePole::default(),
            delay: vec![0.0; frames * 2],
            delay_frames: frames,
            write: 0,
            sample_rate,
        }
    }

    pub fn kind(&self) -> Option<EffectKind> {
        self.kind
    }

    pub fn bypassed(&self) -> bool {
        self.bypass
    }

    pub fn set_bypass(&mut self, bypass: bool) {
        self.bypass = bypass;
    }

    pub fn set_kind(&mut self, kind: Option<EffectKind>) {
        self.kind = kind;
        self.bypass = false;
        self.svf_l.reset();
        self.svf_r.reset();
        self.tone_l.reset();
        self.tone_r.reset();
        self.delay.fill(0.0);
        self.write = 0;
        if let Some(kind) = kind {
            let defaults = kind.defaults();
            for (index, value) in defaults.into_iter().enumerate() {
                self.params[index] = value;
                self.smooth[index].set_now(value);
            }
        }
    }

    pub fn parameter(&self, knob: usize) -> f32 {
        self.params.get(knob).copied().unwrap_or(0.0)
    }

    pub fn set_parameter(&mut self, knob: usize, value: f32) {
        let Some(kind) = self.kind else {
            return;
        };
        let Some(spec) = kind.knobs().get(knob) else {
            return;
        };
        if !value.is_finite() {
            return;
        }
        let value = value.clamp(spec.min, spec.max);
        self.params[knob] = value;
        self.smooth[knob].set(value);
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let Some(kind) = self.kind else {
            return (l, r);
        };
        if self.bypass {
            return (l, r);
        }

        let p0 = self.smooth[0].tick();
        let p1 = self.smooth[1].tick();
        let p2 = self.smooth[2].tick();
        let dry_l = l;
        let dry_r = r;

        let (wet_l, wet_r, mix) = match kind {
            EffectKind::Drive => {
                let pre = 1.0 + p0 * 8.0;
                let gain = p2 / (1.0 + p0 * 3.0);
                ((dry_l * pre).tanh() * gain, (dry_r * pre).tanh() * gain, p1)
            }
            EffectKind::Filter => {
                let (g, k) = Svf::coefficients(p0, p1, self.sample_rate);
                (
                    self.svf_l.process(dry_l, g, k),
                    self.svf_r.process(dry_r, g, k),
                    p2,
                )
            }
            EffectKind::Tone => (
                self.tone_l.process(dry_l, p0, self.sample_rate) * p2,
                self.tone_r.process(dry_r, p0, self.sample_rate) * p2,
                p1,
            ),
            EffectKind::Width => {
                let mid = (dry_l + dry_r) * 0.5;
                let side = (dry_l - dry_r) * 0.5 * p0;
                ((mid + side) * p2, (mid - side) * p2, p1)
            }
            EffectKind::Delay => {
                let (wl, wr) = self.delay_tick(dry_l, dry_r, p0, p1);
                (wl, wr, p2)
            }
        };

        let mix = mix.clamp(0.0, 1.0);
        (dry_l + (wet_l - dry_l) * mix, dry_r + (wet_r - dry_r) * mix)
    }

    /// Read the delay line and push the new input. Advances the write cursor
    /// exactly once per stereo frame.
    fn delay_tick(&mut self, dry_l: f32, dry_r: f32, time: f32, feedback: f32) -> (f32, f32) {
        if self.delay_frames < 4 {
            return (0.0, 0.0);
        }
        let max = self.delay_frames - 2;
        let offset =
            ((time.clamp(0.02, MAX_DELAY_SECONDS) * self.sample_rate) as usize).clamp(2, max);
        let read = (self.write + self.delay_frames - offset) % self.delay_frames;
        let wet_l = self.delay[read * 2];
        let wet_r = self.delay[read * 2 + 1];

        self.delay[self.write * 2] = (dry_l + wet_l * feedback).clamp(-4.0, 4.0);
        self.delay[self.write * 2 + 1] = (dry_r + wet_r * feedback).clamp(-4.0, 4.0);
        self.write = (self.write + 1) % self.delay_frames;
        (wet_l, wet_r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    /// Let parameter smoothing settle so tests measure the steady state.
    fn settle(slot: &mut EffectSlot) {
        for _ in 0..(SR as usize / 10) {
            slot.process(0.0, 0.0);
        }
    }

    fn run_slot(slot: &mut EffectSlot, samples: usize, freq: f32) -> f32 {
        let mut peak = 0.0_f32;
        for n in 0..samples {
            let x = (TAU * freq * n as f32 / SR).sin() * 0.5;
            let (l, r) = slot.process(x, x);
            peak = peak.max(l.abs()).max(r.abs());
        }
        peak
    }

    #[test]
    fn bypass_is_exact_passthrough() {
        let mut slot = EffectSlot::new(SR);
        slot.set_kind(Some(EffectKind::Drive));
        slot.set_bypass(true);
        let (l, r) = slot.process(0.3, -0.2);
        assert_eq!((l, r), (0.3, -0.2));
    }

    #[test]
    fn each_kind_stays_finite_and_bounded() {
        for kind in EffectKind::ALL {
            let mut slot = EffectSlot::new(SR);
            slot.set_kind(Some(kind));
            for knob in 0..3 {
                slot.set_parameter(knob, kind.knobs()[knob].max);
            }
            let peak = run_slot(&mut slot, SR as usize, 220.0);
            assert!(peak.is_finite(), "{kind:?} produced non-finite output");
            assert!(peak < 20.0, "{kind:?} ran away: {peak}");
        }
    }

    #[test]
    fn filter_reduces_high_frequency_energy() {
        let mut open = EffectSlot::new(SR);
        open.set_kind(Some(EffectKind::Filter));
        open.set_parameter(0, 12_000.0);
        open.set_parameter(1, 0.0);
        settle(&mut open);
        let bright = run_slot(&mut open, 24_000, 6_000.0);

        let mut closed = EffectSlot::new(SR);
        closed.set_kind(Some(EffectKind::Filter));
        closed.set_parameter(0, 300.0);
        closed.set_parameter(1, 0.0);
        settle(&mut closed);
        let dark = run_slot(&mut closed, 24_000, 6_000.0);

        assert!(dark < bright * 0.5, "bright {bright}, dark {dark}");
    }

    #[test]
    fn delay_returns_echo_after_the_set_time() {
        let mut slot = EffectSlot::new(SR);
        slot.set_kind(Some(EffectKind::Delay));
        slot.set_parameter(0, 0.1);
        slot.set_parameter(1, 0.0);
        slot.set_parameter(2, 1.0);
        settle(&mut slot);

        // Impulse, then silence: the echo must appear near 100 ms.
        let (l, _) = slot.process(1.0, 1.0);
        assert!(l.abs() < 0.1, "dry impulse leaked into the wet output");
        let mut peak = 0.0_f32;
        let mut peak_at = 0;
        for n in 1..(SR as usize / 5) {
            let (l, _) = slot.process(0.0, 0.0);
            if l.abs() > peak {
                peak = l.abs();
                peak_at = n;
            }
        }
        let expected = (0.1 * SR) as usize;
        assert!(peak > 0.1, "no echo detected");
        assert!(
            peak_at.abs_diff(expected) < 32,
            "echo at {peak_at}, expected {expected}"
        );
    }

    #[test]
    fn knob_ranges_are_clamped() {
        let mut slot = EffectSlot::new(SR);
        slot.set_kind(Some(EffectKind::Tone));
        let specs = EffectKind::Tone.knobs();
        slot.set_parameter(0, 1.0e9);
        assert!((slot.parameter(0) - specs[0].max).abs() < 1e-3);
        slot.set_parameter(0, -1.0e9);
        assert!((slot.parameter(0) - specs[0].min).abs() < 1e-3);
    }

    #[test]
    fn unused_slot_is_transparent() {
        let mut slot = EffectSlot::new(SR);
        assert_eq!(slot.process(0.5, -0.5), (0.5, -0.5));
    }
}
