//! The audio engine: sequencer, FM voices, stereo mix bus and master FX.
//!
//! `Engine` is deliberately free of locks, allocations and syscalls in
//! [`Engine::process`]. It is driven either by the cpal output callback or by
//! the silent fallback clock when no audio device is available.

use rtrb::{Consumer, Producer};

use super::messages::{db_to_gain, Command, FxParam, Telemetry, STEPS_PER_BAR, TRACK_COUNT};
use super::synth::Voice;
use super::tracks::{note_at, TrackSpec, TRACKS};
use crate::dsp::{soft_clip, DcBlocker, OnePole, Smoothed, Svf};

pub const MAX_VOICES: usize = 32;
pub const LOOP_BARS: u64 = 16;
pub const DEFAULT_BPM: f32 = 142.0;
pub const SAMPLE_RATE: u32 = 48_000;

struct TrackState {
    gain: Smoothed,
    pan: Smoothed,
    mute: bool,
    solo: bool,
    active: bool,
    scene: usize,
    peak: f32,
}

impl TrackState {
    fn new(index: usize, sample_rate: f32) -> Self {
        let spec = &TRACKS[index];
        Self {
            gain: Smoothed::new(db_to_gain(spec.gain_db), sample_rate, 0.015),
            pan: Smoothed::new(spec.pan, sample_rate, 0.015),
            mute: false,
            solo: false,
            active: true,
            scene: super::tracks::DEFAULT_SCENES[index],
            peak: 0.0,
        }
    }
}

struct FxState {
    gain: Smoothed,
    tone: Smoothed,
    cutoff: Smoothed,
    resonance: Smoothed,
    drive: Smoothed,
    width: Smoothed,
}

impl FxState {
    fn new(sample_rate: f32) -> Self {
        Self {
            gain: Smoothed::new(0.90, sample_rate, 0.02),
            tone: Smoothed::new(16_000.0, sample_rate, 0.05),
            cutoff: Smoothed::new(5_200.0, sample_rate, 0.04),
            resonance: Smoothed::new(0.18, sample_rate, 0.04),
            drive: Smoothed::new(0.12, sample_rate, 0.03),
            width: Smoothed::new(1.0, sample_rate, 0.03),
        }
    }
}

pub struct Engine {
    sample_rate: f32,
    bpm: f32,
    playing: bool,
    position: u64,
    last_step: u64,
    tracks: [TrackState; TRACK_COUNT],
    voices: [Voice; MAX_VOICES],
    voice_age: u64,
    fx: FxState,
    filter_l: Svf,
    filter_r: Svf,
    tone_l: OnePole,
    tone_r: OnePole,
    dc_l: DcBlocker,
    dc_r: DcBlocker,
    command_rx: Option<Consumer<Command>>,
    telemetry_tx: Option<Producer<Telemetry>>,
    scope_tx: Option<Producer<f32>>,
    telemetry_interval: u32,
    telemetry_countdown: u32,
    cpu_load: f32,
}

impl Engine {
    pub fn new(
        sample_rate: f32,
        command_rx: Option<Consumer<Command>>,
        telemetry_tx: Option<Producer<Telemetry>>,
        scope_tx: Option<Producer<f32>>,
    ) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate >= 8_000.0 {
            sample_rate
        } else {
            SAMPLE_RATE as f32
        };
        let telemetry_interval = (sample_rate / 60.0).round().max(1.0) as u32;
        Self {
            sample_rate,
            bpm: DEFAULT_BPM,
            playing: false,
            position: 0,
            last_step: u64::MAX,
            tracks: std::array::from_fn(|i| TrackState::new(i, sample_rate)),
            voices: [Voice::default(); MAX_VOICES],
            voice_age: 0,
            fx: FxState::new(sample_rate),
            filter_l: Svf::default(),
            filter_r: Svf::default(),
            tone_l: OnePole::default(),
            tone_r: OnePole::default(),
            dc_l: DcBlocker::default(),
            dc_r: DcBlocker::default(),
            command_rx,
            telemetry_tx,
            scope_tx,
            telemetry_interval,
            telemetry_countdown: telemetry_interval,
            cpu_load: 0.0,
        }
    }

    /// Engine without any ring-buffer endpoints; used by tests and `--selftest`.
    pub fn offline(sample_rate: f32) -> Self {
        Self::new(sample_rate, None, None, None)
    }

    #[inline]
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    #[inline]
    pub fn bpm(&self) -> f32 {
        self.bpm
    }

    #[inline]
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    #[inline]
    pub fn position_frames(&self) -> u64 {
        self.position
    }

    /// Emitted telemetry carries the callback load measured by the host.
    pub fn set_cpu_load(&mut self, percent: f32) {
        self.cpu_load = percent.clamp(0.0, 999.0);
    }

    pub fn active_voices(&self) -> u8 {
        self.voices.iter().filter(|v| v.is_active()).count() as u8
    }

    /// Render interleaved stereo audio. The slice length must be even.
    pub fn process(&mut self, out: &mut [f32]) {
        self.drain_commands();

        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }

        let any_solo = self.tracks.iter().any(|t| t.solo);
        let mut block_peaks = [0.0_f32; TRACK_COUNT];
        let mut block_master = [0.0_f32; 2];

        for frame in out.chunks_exact_mut(2) {
            if self.playing {
                self.advance_sequencer();
            }

            let mut track_l = [0.0_f32; TRACK_COUNT];
            let mut track_r = [0.0_f32; TRACK_COUNT];

            // Render voices into their track buses.
            for voice in &mut self.voices {
                if !voice.is_active() {
                    continue;
                }
                let sample = voice.next_sample(self.sample_rate);
                let track = voice.track as usize;
                if track < TRACK_COUNT {
                    track_l[track] += sample;
                    track_r[track] += sample;
                }
            }

            let mut l = 0.0_f32;
            let mut r = 0.0_f32;
            for (index, track) in self.tracks.iter_mut().enumerate() {
                let gain = track.gain.tick();
                let pan = track.pan.tick();
                let audible = !track.mute && (!any_solo || track.solo);
                if !audible {
                    track.peak = 0.0;
                    continue;
                }
                let (pan_l, pan_r) = crate::dsp::pan_gains(pan);
                let tl = track_l[index] * gain * pan_l;
                let tr = track_r[index] * gain * pan_r;
                l += tl;
                r += tr;
                let peak = tl.abs().max(tr.abs());
                if peak > block_peaks[index] {
                    block_peaks[index] = peak;
                }
                if peak > track.peak {
                    track.peak = peak;
                }
            }

            // ---- Master FX chain ----------------------------------------
            let drive = self.fx.drive.tick();
            if drive > 0.001 {
                let pre = 1.0 + drive * 6.0;
                let post = 1.0 / (1.0 + drive * 1.1);
                l = (l * pre).tanh() * post;
                r = (r * pre).tanh() * post;
            }

            let tone = self.fx.tone.tick();
            l = self.tone_l.process(l, tone, self.sample_rate);
            r = self.tone_r.process(r, tone, self.sample_rate);

            let cutoff = self.fx.cutoff.tick();
            let resonance = self.fx.resonance.tick();
            let (g, k) = Svf::coefficients(cutoff, resonance, self.sample_rate);
            l = self.filter_l.process(l, g, k);
            r = self.filter_r.process(r, g, k);

            let width = self.fx.width.tick();
            let mid = (l + r) * 0.5;
            let side = (l - r) * 0.5 * width;
            l = mid + side;
            r = mid - side;

            let gain = self.fx.gain.tick();
            l = soft_clip(l * gain);
            r = soft_clip(r * gain);

            l = self.dc_l.process(l);
            r = self.dc_r.process(r);

            // Speaker protection: the prototype should never emit NaN or
            // full-scale digital clipping, even with extreme knob positions.
            l = if l.is_finite() {
                l.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            r = if r.is_finite() {
                r.clamp(-1.0, 1.0)
            } else {
                0.0
            };

            block_master[0] = block_master[0].max(l.abs());
            block_master[1] = block_master[1].max(r.abs());

            frame[0] = l;
            frame[1] = r;

            self.telemetry_countdown -= 1;
            if self.telemetry_countdown == 0 {
                self.telemetry_countdown = self.telemetry_interval;
                self.publish_telemetry(block_peaks, block_master);
                block_peaks = [0.0; TRACK_COUNT];
                block_master = [0.0; 2];
            }

            // Feed the post-FX bus into the scope ring at the full sample
            // rate; the FFT frequency axis depends on it.
            if let Some(tx) = &mut self.scope_tx {
                let _ = tx.push((l + r) * 0.5);
            }
        }
    }

    fn publish_telemetry(&mut self, peaks: [f32; TRACK_COUNT], master: [f32; 2]) {
        let frame = Telemetry {
            position_frames: self.position,
            playing: self.playing,
            bpm: self.bpm,
            track_peaks: peaks,
            master_peak: master,
            cpu_load: self.cpu_load,
            active_voices: self.active_voices(),
            sample_rate: self.sample_rate as u32,
        };
        if let Some(tx) = &mut self.telemetry_tx {
            // The UI drains at display rate. If it ever falls behind, dropping
            // a telemetry frame is preferable to blocking the audio thread.
            let _ = tx.push(frame);
        }
    }

    fn drain_commands(&mut self) {
        let Some(mut rx) = self.command_rx.take() else {
            return;
        };
        // Bound the work per block so a flood of UI events cannot stall audio.
        for _ in 0..256 {
            match rx.pop() {
                Ok(command) => self.apply(command),
                Err(_) => break,
            }
        }
        self.command_rx = Some(rx);
    }

    pub fn apply(&mut self, command: Command) {
        match command {
            Command::Play => {
                if !self.playing {
                    self.playing = true;
                    if self.position == 0 {
                        self.last_step = u64::MAX;
                    }
                }
            }
            Command::Stop => {
                self.playing = false;
                self.position = 0;
                self.last_step = u64::MAX;
                for voice in &mut self.voices {
                    voice.release();
                }
            }
            Command::SetBpm(bpm) => {
                if bpm.is_finite() {
                    self.bpm = bpm.clamp(30.0, 300.0);
                }
            }
            Command::SetTrackGain { track, gain } => {
                if let Some(t) = self.tracks.get_mut(track as usize) {
                    if gain.is_finite() {
                        t.gain.set(gain.clamp(0.0, 2.0));
                    }
                }
            }
            Command::SetTrackPan { track, pan } => {
                if let Some(t) = self.tracks.get_mut(track as usize) {
                    if pan.is_finite() {
                        t.pan.set(pan.clamp(-1.0, 1.0));
                    }
                }
            }
            Command::SetTrackMute { track, mute } => {
                if let Some(t) = self.tracks.get_mut(track as usize) {
                    t.mute = mute;
                }
            }
            Command::SetTrackSolo { track, solo } => {
                if let Some(t) = self.tracks.get_mut(track as usize) {
                    t.solo = solo;
                }
            }
            Command::LaunchClip { track, scene } => {
                if let Some(t) = self.tracks.get_mut(track as usize) {
                    if (scene as usize) < super::messages::SCENE_COUNT {
                        t.scene = scene as usize;
                        t.active = true;
                    }
                }
            }
            Command::StopClip { track } => {
                if let Some(t) = self.tracks.get_mut(track as usize) {
                    t.active = false;
                    for voice in &mut self.voices {
                        if voice.is_active() && voice.track == track {
                            voice.release();
                        }
                    }
                }
            }
            Command::SetFx { param, value } => {
                if !value.is_finite() {
                    return;
                }
                match param {
                    FxParam::Gain => self.fx.gain.set(value.clamp(0.0, 1.4)),
                    FxParam::Tone => self.fx.tone.set(value.clamp(300.0, 18_000.0)),
                    FxParam::Cutoff => self.fx.cutoff.set(value.clamp(40.0, 18_000.0)),
                    FxParam::Resonance => self.fx.resonance.set(value.clamp(0.0, 1.0)),
                    FxParam::Drive => self.fx.drive.set(value.clamp(0.0, 1.0)),
                    FxParam::Width => self.fx.width.set(value.clamp(0.0, 1.5)),
                }
            }
        }
    }

    #[inline]
    fn samples_per_beat(&self) -> f64 {
        self.sample_rate as f64 * 60.0 / self.bpm as f64
    }

    #[inline]
    fn samples_per_step(&self) -> f64 {
        self.samples_per_beat() / 4.0
    }

    fn advance_sequencer(&mut self) {
        self.position += 1;

        // 16 bars × 4 beats. STEPS_PER_BAR counts 16th notes, not beats.
        let loop_samples = (self.samples_per_beat() * 4.0 * LOOP_BARS as f64)
            .round()
            .max(1.0) as u64;
        if self.position >= loop_samples {
            self.position = 0;
            self.last_step = u64::MAX;
        }

        let step = (self.position as f64 / self.samples_per_step()) as u64;
        if step != self.last_step {
            self.last_step = step;
            let step_in_bar = (step % STEPS_PER_BAR as u64) as usize;
            self.trigger_step(step_in_bar);
        }
    }

    fn trigger_step(&mut self, step_in_bar: usize) {
        let step_samples = self.samples_per_step();
        for (track, spec) in TRACKS.iter().enumerate() {
            if !self.tracks[track].active {
                continue;
            }
            let scene = self.tracks[track].scene;
            let Some(note) = note_at(track, scene, step_in_bar) else {
                continue;
            };
            let gate = (step_samples * spec.gate as f64).round().max(1.0) as u32;
            let velocity = if step_in_bar % 4 == 0 { 1.0 } else { 0.82 };
            self.trigger_note(track, note, velocity, gate, spec);
        }
    }

    fn trigger_note(&mut self, track: usize, note: u8, velocity: f32, gate: u32, spec: &TrackSpec) {
        // Prefer a free voice; otherwise steal the quietest, then oldest.
        let slot = self.voices.iter().position(|v| !v.is_active()).or_else(|| {
            self.voices
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    a.envelope_level()
                        .total_cmp(&b.envelope_level())
                        .then_with(|| a.age().cmp(&b.age()))
                })
                .map(|(i, _)| i)
        });

        if let Some(index) = slot {
            self.voice_age = self.voice_age.wrapping_add(1);
            self.voices[index].trigger(
                spec,
                track,
                note,
                velocity,
                gate,
                self.sample_rate,
                self.voice_age,
            );
        }
    }

    /// Length of the session loop in samples at the current tempo.
    pub fn loop_length_samples(&self) -> u64 {
        (self.samples_per_beat() * 4.0 * LOOP_BARS as f64)
            .round()
            .max(1.0) as u64
    }

    /// Current musical position as (bar, beat, sixteenth), 1-based bar/beat.
    pub fn musical_position(&self) -> (u32, u32, u32) {
        let beat = self.position as f64 / self.samples_per_beat();
        let bar = (beat / 4.0).floor() as u32 + 1;
        let beat_in_bar = (beat.floor() as u32) % 4 + 1;
        let sixteenth = ((beat * 4.0).floor() as u32) % 4 + 1;
        (bar, beat_in_bar, sixteenth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::messages::SCENE_COUNT;
    use rtrb::RingBuffer;

    fn render(engine: &mut Engine, frames: usize) -> Vec<f32> {
        let mut buffer = vec![0.0; frames * 2];
        engine.process(&mut buffer);
        buffer
    }

    #[test]
    fn playing_engine_produces_finite_audio() {
        let mut engine = Engine::offline(48_000.0);
        engine.apply(Command::Play);
        let out = render(&mut engine, 48_000 * 2);
        let peak = out.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.05, "peak too low: {peak}");
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn stopped_engine_is_silent() {
        let mut engine = Engine::offline(48_000.0);
        engine.apply(Command::Play);
        let _ = render(&mut engine, 4800);
        engine.apply(Command::Stop);
        let _ = render(&mut engine, 48_000 * 4);
        let out = render(&mut engine, 4800);
        let peak = out.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak < 1e-4, "engine did not fall silent: {peak}");
    }

    #[test]
    fn bpm_changes_advance_rate() {
        let mut slow = Engine::offline(48_000.0);
        slow.apply(Command::SetBpm(60.0));
        slow.apply(Command::Play);
        let _ = render(&mut slow, 48_000);
        assert_eq!(slow.musical_position().0, 1);
        assert_eq!(slow.musical_position().1, 2);

        let mut fast = Engine::offline(48_000.0);
        fast.apply(Command::SetBpm(240.0));
        fast.apply(Command::Play);
        let _ = render(&mut fast, 48_000);
        assert_eq!(fast.musical_position().0, 2);
        assert_eq!(fast.musical_position().1, 1);
    }

    #[test]
    fn loop_is_sixteen_bars() {
        let mut engine = Engine::offline(48_000.0);
        engine.apply(Command::SetBpm(120.0));
        // 120 BPM: 0.5 s per beat, 4 beats per bar, 16 bars.
        assert_eq!(engine.loop_length_samples(), 48_000 * 32);
        // Rendering a full loop wraps back to the top.
        let mut buffer = vec![0.0; 512 * 2];
        for _ in 0..(engine.loop_length_samples() / 512 + 2) {
            engine.process(&mut buffer);
        }
        assert!(
            engine.position_frames() < 48_000,
            "position did not wrap: {}",
            engine.position_frames()
        );
    }

    #[test]
    fn clips_are_updated_from_commands() {
        let mut engine = Engine::offline(48_000.0);
        engine.apply(Command::LaunchClip { track: 0, scene: 5 });
        assert_eq!(engine.tracks[0].scene, 5);
        engine.apply(Command::StopClip { track: 0 });
        assert!(!engine.tracks[0].active);
        engine.apply(Command::LaunchClip {
            track: 0,
            scene: (SCENE_COUNT + 3) as u8,
        });
        assert_eq!(engine.tracks[0].scene, 5, "invalid scene must be ignored");
    }

    #[test]
    fn ring_buffer_commands_and_telemetry_work() {
        let (mut cmd_tx, cmd_rx) = RingBuffer::new(64);
        let (tele_tx, mut tele_rx) = RingBuffer::new(64);
        let (scope_tx, scope_rx) = RingBuffer::new(16_384);
        let mut engine = Engine::new(48_000.0, Some(cmd_rx), Some(tele_tx), Some(scope_tx));
        cmd_tx.push(Command::Play).unwrap();
        cmd_tx.push(Command::SetBpm(128.0)).unwrap();
        cmd_tx
            .push(Command::SetFx {
                param: FxParam::Cutoff,
                value: 900.0,
            })
            .unwrap();

        let out = render(&mut engine, 48_000);
        assert!(engine.is_playing());
        assert!((engine.bpm() - 128.0).abs() < 1e-6);
        assert!(!tele_rx.is_empty(), "no telemetry emitted");
        assert!(!scope_rx.is_empty(), "no scope samples emitted");

        let mut latest = None;
        while let Ok(frame) = tele_rx.pop() {
            latest = Some(frame);
        }
        let latest = latest.expect("telemetry frame");
        assert!(latest.playing);
        assert!(latest.track_peaks.iter().any(|p| *p > 0.0));
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn mute_silences_a_track_and_master_limits() {
        let mut engine = Engine::offline(48_000.0);
        engine.apply(Command::Play);
        for track in 0..TRACK_COUNT {
            if track != 2 {
                engine.apply(Command::SetTrackMute {
                    track: track as u8,
                    mute: true,
                });
            }
        }
        let out = render(&mut engine, 24_000);
        assert!(out.iter().all(|s| s.abs() <= 1.0));
        assert!(out.iter().any(|s| s.abs() > 0.005));
    }

    #[test]
    fn extreme_fx_parameters_stay_stable() {
        let mut engine = Engine::offline(48_000.0);
        engine.apply(Command::Play);
        engine.apply(Command::SetFx {
            param: FxParam::Resonance,
            value: 1.0,
        });
        engine.apply(Command::SetFx {
            param: FxParam::Drive,
            value: 1.0,
        });
        engine.apply(Command::SetFx {
            param: FxParam::Gain,
            value: 1.4,
        });
        engine.apply(Command::SetFx {
            param: FxParam::Cutoff,
            value: 40.0,
        });
        let out = render(&mut engine, 96_000);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    }
}
