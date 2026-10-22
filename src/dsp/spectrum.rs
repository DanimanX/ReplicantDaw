//! FFT analyser for the spectral waterfall display.
//!
//! The audio thread only pushes mono samples into an `rtrb` ring. The UI
//! drains that ring and performs the FFT on the UI thread, so the audio
//! callback stays cheap.

use std::collections::VecDeque;
use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

pub const FFT_SIZE: usize = 1024;
/// Log-spaced display bins from 30 Hz to 16 kHz.
pub const BINS: usize = 56;
/// Number of spectra kept for the waterfall.
pub const HISTORY: usize = 48;
pub const MIN_HZ: f32 = 30.0;
pub const MAX_HZ: f32 = 16_000.0;

pub struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    ring: Vec<f32>,
    ring_pos: usize,
    filled: usize,
    fft_buf: Vec<Complex32>,
    raw: Vec<f32>,
    display: Vec<f32>,
    history: VecDeque<[f32; BINS]>,
    sample_rate: f32,
    frames: u64,
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: f32) -> Self {
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        // Hann window, precomputed.
        let window = (0..FFT_SIZE)
            .map(|n| {
                let x = std::f32::consts::TAU * n as f32 / (FFT_SIZE - 1) as f32;
                0.5 - 0.5 * x.cos()
            })
            .collect();

        let mut history = VecDeque::with_capacity(HISTORY + 1);
        for _ in 0..HISTORY {
            history.push_back([0.0; BINS]);
        }

        Self {
            fft,
            window,
            ring: vec![0.0; FFT_SIZE],
            ring_pos: 0,
            filled: 0,
            fft_buf: vec![Complex32::default(); FFT_SIZE],
            raw: vec![0.0; FFT_SIZE / 2],
            display: vec![0.0; BINS],
            history,
            sample_rate,
            frames: 0,
        }
    }

    pub fn push_samples(&mut self, samples: &[f32]) {
        for &sample in samples {
            self.ring[self.ring_pos] = sample;
            self.ring_pos = (self.ring_pos + 1) % FFT_SIZE;
            self.filled = (self.filled + 1).min(FFT_SIZE);
            self.frames += 1;
        }
    }

    /// Compute the current spectrum. Cheap enough to run once per frame.
    pub fn compute(&mut self) {
        // Unwrap the ring into chronological order (oldest first).
        for i in 0..FFT_SIZE {
            let source = if self.filled < FFT_SIZE {
                // Pad the start with silence until the ring has data.
                let offset = FFT_SIZE - self.filled;
                if i < offset {
                    self.fft_buf[i] = Complex32::default();
                    continue;
                }
                (self.ring_pos + i - offset) % FFT_SIZE
            } else {
                (self.ring_pos + i) % FFT_SIZE
            };
            let value = self.ring[source] * self.window[i];
            self.fft_buf[i] = Complex32::new(value, 0.0);
        }

        self.fft.process(&mut self.fft_buf);
        for (bin, complex) in self.fft_buf.iter().take(FFT_SIZE / 2).enumerate() {
            // Hann coherent gain = 0.5, two-sided -> scale to a 0..1 amplitude.
            self.raw[bin] = complex.norm() / (FFT_SIZE as f32 * 0.25);
        }

        let bin_hz = self.sample_rate / FFT_SIZE as f32;
        let log_ratio = (MAX_HZ / MIN_HZ).ln();
        for i in 0..BINS {
            let f0 = MIN_HZ * (log_ratio * i as f32 / BINS as f32).exp();
            let f1 = MIN_HZ * (log_ratio * (i + 1) as f32 / BINS as f32).exp();
            let b0 = (f0 / bin_hz).floor() as usize;
            let b1 = ((f1 / bin_hz).ceil() as usize).min(self.raw.len() - 1);
            let mut mag = 0.0_f32;
            for bin in b0..=b1.max(b0) {
                if let Some(value) = self.raw.get(bin) {
                    mag = mag.max(*value);
                }
            }
            // -84 dB .. 0 dB mapped to 0..1. The power curve keeps the
            // waterfall from saturating into a flat wall of colour.
            let db = 20.0 * (mag + 1e-9).log10();
            let norm = ((db + 84.0) / 84.0).clamp(0.0, 1.0);
            // Lift the noise floor out and keep a little headroom so the
            // waterfall has dynamics instead of a saturated plateau.
            let shaped = ((norm - 0.12) / 0.88).clamp(0.0, 1.0);
            self.display[i] = shaped.powf(2.4);
        }
    }

    pub fn display(&self) -> &[f32] {
        &self.display
    }

    pub fn history(&self) -> &VecDeque<[f32; BINS]> {
        &self.history
    }

    pub fn push_history(&mut self) {
        let mut frame = [0.0_f32; BINS];
        frame.copy_from_slice(&self.display);
        if self.history.len() >= HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(frame);
    }

    pub fn seed_history(&mut self) {
        self.history.clear();
        for _ in 0..HISTORY {
            self.history.push_back([0.0; BINS]);
        }
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Frequency represented by a display bin index (for labels/tests).
    pub fn bin_hz(index: usize) -> f32 {
        let log_ratio = (MAX_HZ / MIN_HZ).ln();
        MIN_HZ * (log_ratio * (index as f32 + 0.5) / BINS as f32).exp()
    }

    /// Convenience wrapper for the UI peak readout.
    pub fn peak_hz(&self, index: usize) -> f32 {
        Self::bin_hz(index)
    }

    /// Match the analyser to the host sample rate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if (self.sample_rate - sample_rate).abs() > 0.5 {
            self.sample_rate = sample_rate;
            self.ring.fill(0.0);
            self.ring_pos = 0;
            self.filled = 0;
            self.compute();
            self.seed_history();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn feed_sine(analyzer: &mut SpectrumAnalyzer, hz: f32, amplitude: f32) {
        let sr = 48_000.0;
        for n in 0..FFT_SIZE * 4 {
            let sample = (TAU * hz * n as f32 / sr).sin() * amplitude;
            analyzer.push_samples(&[sample]);
        }
        analyzer.compute();
    }

    fn dominant_hz(analyzer: &SpectrumAnalyzer) -> f32 {
        let (index, _) = analyzer
            .display()
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        SpectrumAnalyzer::bin_hz(index)
    }

    #[test]
    fn detects_a_sine_frequency() {
        let mut analyzer = SpectrumAnalyzer::new(48_000.0);
        feed_sine(&mut analyzer, 1_000.0, 0.7);
        let peak = dominant_hz(&analyzer);
        assert!((peak - 1_000.0).abs() < 120.0, "peak at {peak} Hz");
        assert!(analyzer.display().iter().cloned().fold(0.0, f32::max) > 0.6);
    }

    #[test]
    fn silence_stays_dark() {
        let mut analyzer = SpectrumAnalyzer::new(48_000.0);
        analyzer.compute();
        assert!(analyzer.display().iter().all(|v| *v < 0.01));
    }

    #[test]
    fn quieter_sine_is_lower() {
        let mut loud = SpectrumAnalyzer::new(48_000.0);
        feed_sine(&mut loud, 440.0, 0.8);
        let mut quiet = SpectrumAnalyzer::new(48_000.0);
        feed_sine(&mut quiet, 440.0, 0.08);
        let loud_peak = loud.display().iter().cloned().fold(0.0, f32::max);
        let quiet_peak = quiet.display().iter().cloned().fold(0.0, f32::max);
        assert!(loud_peak > quiet_peak);
    }

    #[test]
    fn history_has_fixed_length() {
        let mut analyzer = SpectrumAnalyzer::new(48_000.0);
        for _ in 0..HISTORY * 2 {
            analyzer.compute();
            analyzer.push_history();
        }
        assert_eq!(analyzer.history().len(), HISTORY);
    }
}
