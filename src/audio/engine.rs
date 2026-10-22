//! The audio engine: sequencer, FM voices, stereo mix bus and master FX.
//!
//! `Engine` is deliberately free of locks, allocations and syscalls in
//! [`Engine::process`]. It is driven either by the cpal output callback or by
//! the silent fallback clock when no audio device is available.

use rtrb::{Consumer, Producer};

use super::effects::EffectSlot;
use super::messages::{
    db_to_gain, Command, EffectKind, RecordEvent, RecordedClip, RecordedNote, Telemetry,
    LOOP_STEPS, MAX_EFFECTS, STEPS_PER_BAR, TRACK_COUNT,
};
use super::midi::MidiMessage;
use super::synth::{SynthParams, Voice};
use super::tracks::{note_at, TrackSpec, TRACKS};
use crate::dsp::{soft_clip, DcBlocker, Smoothed};
use crate::plugins::{PluginBlock, PLUGIN_BLOCK_FRAMES};

pub const MAX_VOICES: usize = 32;
pub const LOOP_BARS: u64 = 16;
pub const DEFAULT_BPM: f32 = 142.0;
pub const SAMPLE_RATE: u32 = 48_000;

struct TrackState {
    gain: Smoothed,
    pan: Smoothed,
    clip_gain: Smoothed,
    fade_in: f32,
    fade_out: f32,
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
            clip_gain: Smoothed::new(1.0, sample_rate, 0.015),
            fade_in: 0.0,
            fade_out: 0.0,
            mute: false,
            solo: false,
            active: true,
            scene: super::tracks::DEFAULT_SCENES[index],
            peak: 0.0,
        }
    }
}

/// Optional ring-buffer endpoints and side channels for the engine.
#[derive(Default)]
pub struct EngineIo {
    pub commands: Option<Consumer<Command>>,
    pub telemetry: Option<Producer<Telemetry>>,
    pub scope: Option<Producer<f32>>,
    pub midi: Option<Consumer<MidiMessage>>,
    pub record_events: Option<Producer<RecordEvent>>,
    /// Dry audio sent to the plugin worker.
    pub plugin_input: Option<Producer<PluginBlock>>,
    /// Processed audio returned by the plugin worker.
    pub plugin_output: Option<Consumer<PluginBlock>>,
}

pub struct Engine {
    sample_rate: f32,
    bpm: f32,
    playing: bool,
    position: u64,
    last_step: u64,
    tracks: [TrackState; TRACK_COUNT],
    synth: [SynthParams; TRACK_COUNT],
    voices: [Voice; MAX_VOICES],
    voice_age: u64,
    effects: [EffectSlot; MAX_EFFECTS],
    effect_count: u8,
    master_gain: Smoothed,
    dc_l: DcBlocker,
    dc_r: DcBlocker,
    command_rx: Option<Consumer<Command>>,
    telemetry_tx: Option<Producer<Telemetry>>,
    scope_tx: Option<Producer<f32>>,
    telemetry_interval: u32,
    telemetry_countdown: u32,
    cpu_load: f32,
    midi_rx: Option<Consumer<MidiMessage>>,
    record_tx: Option<Producer<RecordEvent>>,
    recorded: [RecordedClip; TRACK_COUNT],
    recorded_active: [bool; TRACK_COUNT],
    recording: bool,
    record_track: u8,
    record_pending: Option<RecordedNote>,
    loop_step: u16,
    plugin_input: Option<Producer<PluginBlock>>,
    plugin_output: Option<Consumer<PluginBlock>>,
    plugin_pending: PluginBlock,
    plugin_ready: PluginBlock,
    plugin_ready_pos: u16,
    plugin_ready_len: u16,
    plugin_midi: bool,
    plugin_latency: u16,
    track_plugin: [bool; TRACK_COUNT],
    last_midi_note: u8,
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
        Self::new_with_io(
            sample_rate,
            EngineIo {
                commands: command_rx,
                telemetry: telemetry_tx,
                scope: scope_tx,
                ..EngineIo::default()
            },
        )
    }

    /// Construct an engine with every optional side channel.
    pub fn new_with_io(sample_rate: f32, io: EngineIo) -> Self {
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
            synth: std::array::from_fn(|i| SynthParams::from_spec(&TRACKS[i])),
            voices: [Voice::default(); MAX_VOICES],
            voice_age: 0,
            effects: std::array::from_fn(|_| EffectSlot::new(sample_rate)),
            effect_count: 0,
            master_gain: Smoothed::new(0.90, sample_rate, 0.02),
            dc_l: DcBlocker::default(),
            dc_r: DcBlocker::default(),
            command_rx: io.commands,
            telemetry_tx: io.telemetry,
            scope_tx: io.scope,
            telemetry_interval,
            telemetry_countdown: telemetry_interval,
            cpu_load: 0.0,
            midi_rx: io.midi,
            record_tx: io.record_events,
            recorded: [RecordedClip::default(); TRACK_COUNT],
            recorded_active: [true; TRACK_COUNT],
            recording: false,
            record_track: 0,
            record_pending: None,
            loop_step: 0,
            plugin_input: io.plugin_input,
            plugin_output: io.plugin_output,
            plugin_pending: PluginBlock::default(),
            plugin_ready: PluginBlock::default(),
            plugin_ready_pos: 0,
            plugin_ready_len: 0,
            plugin_midi: true,
            plugin_latency: 0,
            track_plugin: [false; TRACK_COUNT],
            last_midi_note: 60,
        }
        .with_default_chain()
    }

    /// Default master chain: the three "signature" devices from the original
    /// design, now as removable modules.
    fn with_default_chain(mut self) -> Self {
        for kind in [
            EffectKind::Drive,
            EffectKind::Tone,
            EffectKind::Filter,
            EffectKind::Width,
        ] {
            self.add_effect(kind);
        }
        self
    }

    fn add_effect(&mut self, kind: EffectKind) {
        let index = self.effect_count as usize;
        if index < MAX_EFFECTS {
            self.effects[index].set_kind(Some(kind));
            self.effect_count += 1;
        }
    }

    /// Number of effect modules currently in the chain.
    pub fn effect_count(&self) -> usize {
        self.effect_count as usize
    }

    /// Kind of the effect at `index`, if any.
    pub fn effect_kind(&self, index: usize) -> Option<EffectKind> {
        if index < self.effect_count as usize {
            self.effects[index].kind()
        } else {
            None
        }
    }

    /// Current knob value of an effect module.
    pub fn effect_parameter(&self, index: usize, knob: usize) -> f32 {
        self.effects
            .get(index)
            .map(|slot| slot.parameter(knob))
            .unwrap_or(0.0)
    }

    /// Editable synth parameters for a track.
    pub fn synth_params(&self, track: usize) -> SynthParams {
        self.synth[track.min(TRACK_COUNT - 1)]
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
        self.drain_midi();

        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }

        let any_solo = self.tracks.iter().any(|t| t.solo);
        let loop_samples = (self.samples_per_beat() * 4.0 * LOOP_BARS as f64).max(1.0) as f32;
        let track_plugins = self.plugin_input.is_some()
            && self.plugin_output.is_some()
            && self.track_plugin.iter().any(|active| *active);
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

            for (index, track) in self.tracks.iter_mut().enumerate() {
                let gain = track.gain.tick();
                let pan = track.pan.tick();
                let clip_gain = track.clip_gain.tick();
                let audible = !track.mute && (!any_solo || track.solo);
                if !audible {
                    track_l[index] = 0.0;
                    track_r[index] = 0.0;
                    track.peak = 0.0;
                    continue;
                }

                // Audio-clip envelope: level plus loop-normalized fades.
                let clip_envelope = if self.playing {
                    let progress = (self.position as f32 / loop_samples).clamp(0.0, 1.0);
                    let fade_in = if track.fade_in > 0.0 {
                        (progress / track.fade_in).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    let fade_out = if track.fade_out > 0.0 {
                        ((1.0 - progress) / track.fade_out).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    fade_in.min(fade_out)
                } else {
                    1.0
                };
                let envelope = clip_gain * clip_envelope;

                let (pan_l, pan_r) = crate::dsp::pan_gains(pan);
                let tl = track_l[index] * gain * pan_l * envelope;
                let tr = track_r[index] * gain * pan_r * envelope;
                track_l[index] = tl;
                track_r[index] = tr;

                let peak = tl.abs().max(tr.abs());
                if peak > block_peaks[index] {
                    block_peaks[index] = peak;
                }
                if peak > track.peak {
                    track.peak = peak;
                }
            }

            // Track plugins process their own bus. When any track insert is
            // active the output is delayed by one plugin block so the worker
            // can process each bus in isolation.
            let (mut l, mut r) = if track_plugins {
                self.run_track_plugin_block(&track_l, &track_r)
            } else {
                self.mix_master(&track_l, &track_r)
            };

            // Third-party plugins can overshoot; keep the output guard last.
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

    /// Sum the per-track buses through the master effect chain.
    fn mix_master(
        &mut self,
        track_l: &[f32; TRACK_COUNT],
        track_r: &[f32; TRACK_COUNT],
    ) -> (f32, f32) {
        let mut l = track_l.iter().sum::<f32>();
        let mut r = track_r.iter().sum::<f32>();

        let effect_count = self.effect_count as usize;
        for slot in &mut self.effects[..effect_count] {
            let (wet_l, wet_r) = slot.process(l, r);
            l = wet_l;
            r = wet_r;
        }

        let gain = self.master_gain.tick();
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
        (l, r)
    }

    /// Queue one frame of per-track buses and return the matching frame from
    /// the previous plugin block.
    fn run_track_plugin_block(
        &mut self,
        track_l: &[f32; TRACK_COUNT],
        track_r: &[f32; TRACK_COUNT],
    ) -> (f32, f32) {
        let index = self.plugin_pending.frames as usize;
        if index < PLUGIN_BLOCK_FRAMES {
            for track in 0..TRACK_COUNT {
                self.plugin_pending.left[track][index] = track_l[track];
                self.plugin_pending.right[track][index] = track_r[track];
            }
            self.plugin_pending.frames += 1;
        }

        if self.plugin_pending.frames as usize >= PLUGIN_BLOCK_FRAMES {
            if let Some(tx) = &mut self.plugin_input {
                let mut block = std::mem::take(&mut self.plugin_pending);
                block.frames = PLUGIN_BLOCK_FRAMES as u16;
                let _ = tx.push(block);
            }
            if let Some(rx) = &mut self.plugin_output {
                if let Ok(block) = rx.pop() {
                    self.plugin_latency = PLUGIN_BLOCK_FRAMES as u16;
                    self.plugin_ready = block;
                    self.plugin_ready_len = PLUGIN_BLOCK_FRAMES as u16;
                    self.plugin_ready_pos = 0;
                }
            }
        }

        if self.plugin_ready_pos < self.plugin_ready_len {
            let position = self.plugin_ready_pos as usize;
            let mut ready_l = [0.0_f32; TRACK_COUNT];
            let mut ready_r = [0.0_f32; TRACK_COUNT];
            for track in 0..TRACK_COUNT {
                ready_l[track] = self.plugin_ready.left[track][position];
                ready_r[track] = self.plugin_ready.right[track][position];
            }
            self.plugin_ready_pos += 1;
            if self.plugin_ready_pos >= self.plugin_ready_len {
                self.plugin_ready_len = 0;
                self.plugin_ready_pos = 0;
            }
            self.mix_master(&ready_l, &ready_r)
        } else {
            (0.0, 0.0)
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
            recording: self.recording,
            record_track: self.record_track,
            recorded_notes: self.recorded.iter().map(|clip| clip.len).sum(),
            plugin_latency: self.plugin_latency,
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
            Command::RecordStart { track } => {
                let track = (track as usize).min(TRACK_COUNT - 1);
                self.record_track = track as u8;
                self.recorded[track].clear();
                self.recording = true;
                // A recorded clip must be audible even if the clip slot was
                // previously stopped.
                self.tracks[track].active = true;
                self.record_pending = None;
                if !self.playing {
                    self.playing = true;
                }
                self.emit_record_reset(track as u8);
            }
            Command::RecordStop => {
                self.finalize_pending_note(self.loop_step);
                self.recording = false;
            }
            Command::ClearRecorded { track } => {
                if let Some(clip) = self.recorded.get_mut(track as usize) {
                    clip.clear();
                    self.emit_record_reset(track);
                }
            }
            Command::SetRecordedClip { track, clip } => {
                if let Some(slot) = self.recorded.get_mut(track as usize) {
                    *slot = clip;
                }
            }
            Command::SetPluginMidi { enabled } => {
                self.plugin_midi = enabled;
            }
            Command::SetInputTrack { track } => {
                self.record_track = (track as usize).min(TRACK_COUNT - 1) as u8;
            }
            Command::SetTrackPlugin { track, active } => {
                if let Some(flag) = self.track_plugin.get_mut(track as usize) {
                    *flag = active;
                    if !self.track_plugin.iter().any(|enabled| *enabled) {
                        // Leaving the plugin path: drop the one-block delay.
                        self.plugin_pending.frames = 0;
                        self.plugin_pending.clear_events();
                        self.plugin_ready_len = 0;
                        self.plugin_ready_pos = 0;
                        self.plugin_latency = 0;
                    } else if active {
                        // Avoid feeding stale events into a freshly inserted
                        // plugin and restart the block alignment cleanly.
                        self.plugin_pending.frames = 0;
                        self.plugin_pending.clear_events();
                        self.plugin_ready_len = 0;
                        self.plugin_ready_pos = 0;
                    }
                }
            }
            Command::SetTrackClip {
                track,
                gain,
                fade_in,
                fade_out,
            } => {
                if let Some(state) = self.tracks.get_mut(track as usize) {
                    if gain.is_finite() {
                        state.clip_gain.set(gain.clamp(0.0, 4.0));
                    }
                    if fade_in.is_finite() {
                        state.fade_in = fade_in.clamp(0.0, 1.0);
                    }
                    if fade_out.is_finite() {
                        state.fade_out = fade_out.clamp(0.0, 1.0);
                    }
                }
            }
            Command::SetSynthParam {
                track,
                param,
                value,
            } => {
                if let Some(params) = self.synth.get_mut(track as usize) {
                    params.set(param, value);
                }
            }
            Command::AddEffect { kind } => self.add_effect(kind),
            Command::RemoveEffect { index } => {
                let index = index as usize;
                let count = self.effect_count as usize;
                if index < count {
                    self.effects[index..count].rotate_left(1);
                    self.effects[count - 1].set_kind(None);
                    self.effect_count -= 1;
                }
            }
            Command::MoveEffect { from, to } => {
                let count = self.effect_count as usize;
                let from = from as usize;
                let to = (to as usize).min(count.saturating_sub(1));
                if count > 1 && from < count && from != to {
                    if from < to {
                        self.effects[from..=to].rotate_left(1);
                    } else {
                        self.effects[to..=from].rotate_right(1);
                    }
                }
            }
            Command::SetEffectParam { index, knob, value } => {
                if let Some(slot) = self.effects.get_mut(index as usize) {
                    if (index as usize) < self.effect_count as usize {
                        slot.set_parameter(knob as usize, value);
                    }
                }
            }
            Command::SetEffectBypass { index, bypass } => {
                if (index as usize) < self.effect_count as usize {
                    if let Some(slot) = self.effects.get_mut(index as usize) {
                        slot.set_bypass(bypass);
                    }
                }
            }
            Command::ClearEffects => {
                for slot in &mut self.effects {
                    slot.set_kind(None);
                }
                self.effect_count = 0;
            }
            Command::SetMasterGain { gain } => {
                if gain.is_finite() {
                    self.master_gain.set(gain.clamp(0.0, 2.0));
                }
            }
            Command::SetRecordedActive { track, active } => {
                if let Some(flag) = self.recorded_active.get_mut(track as usize) {
                    *flag = active;
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
            if self.recording {
                self.finalize_pending_note(LOOP_STEPS as u16);
            }
            self.position = 0;
            self.last_step = u64::MAX;
            self.loop_step = 0;
        }

        let step = (self.position as f64 / self.samples_per_step()) as u64;
        self.loop_step = (step % LOOP_STEPS as u64) as u16;
        if step != self.last_step {
            self.last_step = step;
            let step_in_bar = (step % STEPS_PER_BAR as u64) as usize;
            self.trigger_step(step_in_bar);
        }
    }

    fn trigger_step(&mut self, step_in_bar: usize) {
        let step_samples = self.samples_per_step();
        let loop_step = self.loop_step;

        for (track, spec) in TRACKS.iter().enumerate() {
            if !self.tracks[track].active {
                continue;
            }

            // Recorded MIDI takes precedence over the generative pattern.
            let clip = self.recorded[track];
            if !clip.is_empty() {
                if !self.recorded_active[track] {
                    continue;
                }
                for note in clip.as_slice() {
                    if note.start_step != loop_step {
                        continue;
                    }
                    let gate = (step_samples * note.length_steps.max(1) as f64)
                        .round()
                        .max(1.0) as u32;
                    let velocity = (note.velocity as f32 / 127.0).clamp(0.05, 1.0);
                    self.trigger_note(track, note.note, velocity, gate, spec);
                }
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

    /// Drain hardware/virtual MIDI without blocking or allocating.
    fn drain_midi(&mut self) {
        let Some(mut rx) = self.midi_rx.take() else {
            return;
        };
        for _ in 0..64 {
            match rx.pop() {
                Ok(message) => self.handle_midi(message),
                Err(_) => break,
            }
        }
        self.midi_rx = Some(rx);
    }

    fn handle_midi(&mut self, message: MidiMessage) {
        match message {
            MidiMessage::NoteOn {
                note,
                velocity,
                channel: _,
            } => {
                self.last_midi_note = note;
                if self.recording {
                    let pending = RecordedNote {
                        note,
                        velocity,
                        start_step: self.loop_step,
                        length_steps: 1,
                    };
                    self.record_pending = Some(pending);
                }
                let track = self.record_track as usize;
                let spec = &TRACKS[track.min(TRACK_COUNT - 1)];
                let velocity_norm = (velocity as f32 / 127.0).clamp(0.05, 1.0);
                // Live notes stay open until the matching note-off.
                self.trigger_note(track, note, velocity_norm, u32::MAX / 4, spec);
                self.forward_midi([0x90, note, velocity], 3);
            }
            MidiMessage::NoteOff { note, channel: _ } => {
                if let Some(pending) = self.record_pending {
                    if pending.note == note {
                        self.finalize_pending_note(self.loop_step);
                    }
                }
                for voice in &mut self.voices {
                    if voice.is_active() && voice.track == self.record_track && voice.note() == note
                    {
                        voice.release();
                    }
                }
                self.forward_midi([0x80, note, 0], 3);
            }
            MidiMessage::ControlChange {
                controller,
                value,
                channel,
            } => {
                self.forward_midi([0xb0 | (channel & 0x0f), controller, value], 3);
            }
            MidiMessage::PitchBend { value, channel } => {
                self.forward_midi(
                    [
                        0xe0 | (channel & 0x0f),
                        (value & 0x7f) as u8,
                        (value >> 7) as u8,
                    ],
                    3,
                );
            }
        }
    }

    fn finalize_pending_note(&mut self, end_step: u16) {
        let Some(mut pending) = self.record_pending.take() else {
            return;
        };
        let length = if end_step >= pending.start_step {
            end_step - pending.start_step
        } else {
            LOOP_STEPS as u16 - pending.start_step + end_step
        };
        pending.length_steps = length.max(1);
        let track = self.record_track as usize;

        if let Some(clip) = self.recorded.get_mut(track) {
            clip.push(pending);
        }
        self.emit_record_note(track as u8, pending, false);
    }

    fn emit_record_reset(&mut self, track: u8) {
        if let Some(tx) = &mut self.record_tx {
            let _ = tx.push(RecordEvent {
                track,
                note: RecordedNote::default(),
                reset: true,
            });
        }
    }

    fn emit_record_note(&mut self, track: u8, note: RecordedNote, reset: bool) {
        if let Some(tx) = &mut self.record_tx {
            let _ = tx.push(RecordEvent { track, note, reset });
        }
    }

    fn forward_midi(&mut self, bytes: [u8; 3], len: u8) {
        if !self.plugin_midi || self.plugin_input.is_none() {
            return;
        }
        let offset = self.plugin_pending.frames.saturating_sub(1);
        let bus = self.record_track;
        self.plugin_pending.push_event(bus, offset, bytes, len);
    }

    /// Currently recorded clip for a track (used by tests and diagnostics).
    pub fn recorded_clip(&self, track: usize) -> &RecordedClip {
        &self.recorded[track.min(TRACK_COUNT - 1)]
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    fn trigger_note(
        &mut self,
        track: usize,
        note: u8,
        velocity: f32,
        gate: u32,
        _spec: &TrackSpec,
    ) {
        let params = self.synth[track.min(TRACK_COUNT - 1)];
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
                &params,
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
    use crate::audio::messages::{SynthParam, SCENE_COUNT};
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
            .push(Command::SetEffectParam {
                index: 0,
                knob: 0,
                value: 0.4,
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
    fn midi_input_is_recorded_into_a_clip() {
        let (mut midi_tx, midi_rx) = RingBuffer::new(64);
        let (record_tx, mut record_rx) = RingBuffer::new(64);
        let mut engine = Engine::new_with_io(
            48_000.0,
            EngineIo {
                midi: Some(midi_rx),
                record_events: Some(record_tx),
                ..EngineIo::default()
            },
        );
        engine.apply(Command::SetBpm(120.0));
        engine.apply(Command::RecordStart { track: 2 });
        assert!(engine.is_playing(), "recording should start the transport");

        midi_tx
            .push(MidiMessage::NoteOn {
                channel: 0,
                note: 60,
                velocity: 100,
            })
            .unwrap();
        let mut buffer = vec![0.0; 512 * 2];
        for _ in 0..(48_000 / 512) {
            engine.process(&mut buffer);
        }
        midi_tx
            .push(MidiMessage::NoteOff {
                channel: 0,
                note: 60,
            })
            .unwrap();
        for _ in 0..(48_000 / 512) {
            engine.process(&mut buffer);
        }
        engine.apply(Command::RecordStop);

        let clip = engine.recorded_clip(2);
        assert_eq!(clip.len, 1, "expected exactly one recorded note");
        let note = clip.as_slice()[0];
        assert_eq!(note.note, 60);
        assert!(note.length_steps >= 1, "note length {}", note.length_steps);

        let mut reset_seen = false;
        let mut note_seen = false;
        while let Ok(event) = record_rx.pop() {
            reset_seen |= event.reset;
            note_seen |= !event.reset && event.note.note == 60;
        }
        assert!(reset_seen, "no reset event emitted");
        assert!(note_seen, "no note event mirrored to the UI");
    }

    #[test]
    fn recorded_clip_plays_back() {
        let mut engine = Engine::offline(48_000.0);
        let mut clip = RecordedClip::default();
        clip.push(RecordedNote {
            note: 64,
            velocity: 110,
            start_step: 0,
            length_steps: 4,
        });
        engine.apply(Command::SetRecordedClip { track: 0, clip });
        engine.apply(Command::SetBpm(120.0));
        engine.apply(Command::Play);
        let out = render(&mut engine, 12_000);
        let peak = out
            .iter()
            .fold(0.0_f32, |max, sample| max.max(sample.abs()));
        assert!(peak > 0.01, "recorded clip did not sound (peak {peak})");
        assert_eq!(engine.recorded_clip(0).len, 1);
    }

    #[test]
    fn plugin_chain_round_trip() {
        let (dry_tx, mut dry_rx) = RingBuffer::new(8);
        let (mut wet_tx, wet_rx) = RingBuffer::new(8);
        let mut engine = Engine::new_with_io(
            48_000.0,
            EngineIo {
                plugin_input: Some(dry_tx),
                plugin_output: Some(wet_rx),
                ..EngineIo::default()
            },
        );
        engine.apply(Command::Play);
        engine.apply(Command::SetTrackPlugin {
            track: 0,
            active: true,
        });

        let frames = PLUGIN_BLOCK_FRAMES;
        let mut out = vec![0.0; frames * 2];
        engine.process(&mut out);
        let block = dry_rx.pop().expect("engine did not send a dry block");
        assert_eq!(block.frames as usize, frames);
        assert!(block.left.iter().flatten().any(|sample| *sample != 0.0));

        // A "plugin" that inverts the signal on every track bus.
        let mut processed = block;
        for bus in processed.left.iter_mut() {
            for sample in bus.iter_mut() {
                *sample = -*sample;
            }
        }
        for bus in processed.right.iter_mut() {
            for sample in bus.iter_mut() {
                *sample = -*sample;
            }
        }
        wet_tx.push(processed).unwrap();

        let mut delayed = vec![0.0; frames * 2];
        engine.process(&mut delayed);
        // The block becomes ready on the last frame of this call; render one
        // more block to hear the processed audio.
        engine.process(&mut delayed);
        // The engine outputs the processed block while rendering the next one.
        let sent = dry_rx.pop().expect("second dry block");
        assert_eq!(sent.frames as usize, frames);
        let wet_frames: Vec<f32> = delayed
            .chunks_exact(2)
            .map(|frame| frame[0])
            .filter(|sample| sample.abs() > 1e-9)
            .collect();
        assert!(
            !wet_frames.is_empty(),
            "processed audio never reached the output"
        );
    }

    #[test]
    fn default_chain_is_present_and_removable() {
        let mut engine = Engine::offline(48_000.0);
        assert_eq!(engine.effect_count(), 4);
        assert_eq!(engine.effect_kind(0), Some(crate::audio::EffectKind::Drive));

        engine.apply(Command::RemoveEffect { index: 1 });
        assert_eq!(engine.effect_count(), 3);
        assert_eq!(
            engine.effect_kind(1),
            Some(crate::audio::EffectKind::Filter)
        );

        engine.apply(Command::MoveEffect { from: 2, to: 0 });
        assert_eq!(engine.effect_kind(0), Some(crate::audio::EffectKind::Width));

        engine.apply(Command::ClearEffects);
        assert_eq!(engine.effect_count(), 0);

        for kind in crate::audio::EffectKind::ALL {
            engine.apply(Command::AddEffect { kind });
        }
        assert_eq!(engine.effect_count(), crate::audio::EffectKind::ALL.len());
    }

    #[test]
    fn chain_can_exceed_the_slot_limit_without_panicking() {
        let mut engine = Engine::offline(48_000.0);
        for _ in 0..MAX_EFFECTS * 3 {
            engine.apply(Command::AddEffect {
                kind: EffectKind::Delay,
            });
        }
        assert_eq!(engine.effect_count(), MAX_EFFECTS);
        let out = render(&mut engine, 4_800);
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn extreme_chain_parameters_stay_stable() {
        let mut engine = Engine::offline(48_000.0);
        engine.apply(Command::Play);
        engine.apply(Command::ClearEffects);
        for kind in crate::audio::EffectKind::ALL {
            engine.apply(Command::AddEffect { kind });
        }
        for index in 0..engine.effect_count() {
            for knob in 0..3 {
                engine.apply(Command::SetEffectParam {
                    index: index as u8,
                    knob,
                    value: 1.0e6,
                });
            }
        }
        engine.apply(Command::SetMasterGain { gain: 1.6 });
        let out = render(&mut engine, 96_000);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    }

    #[test]
    fn synth_parameters_change_the_voice() {
        let mut bright = Engine::offline(48_000.0);
        bright.apply(Command::SetSynthParam {
            track: 0,
            param: SynthParam::Index,
            value: 6.0,
        });
        bright.apply(Command::Play);
        let bright_out = render(&mut bright, 24_000);
        let bright_energy: f32 = bright_out.iter().map(|s| s * s).sum();

        let mut mellow = Engine::offline(48_000.0);
        mellow.apply(Command::SetSynthParam {
            track: 0,
            param: SynthParam::Index,
            value: 0.0,
        });
        mellow.apply(Command::Play);
        let mellow_out = render(&mut mellow, 24_000);
        let mellow_energy: f32 = mellow_out.iter().map(|s| s * s).sum();

        assert!(bright_energy.is_finite() && mellow_energy.is_finite());
        assert!(
            (bright_energy - mellow_energy).abs() > 1.0,
            "synth parameter had no audible effect ({bright_energy} vs {mellow_energy})"
        );
        assert!((bright.synth_params(0).mod_index - 6.0).abs() < 1e-6);
    }

    #[test]
    fn disabled_recorded_clip_is_silent() {
        let mut engine = Engine::offline(48_000.0);
        let mut clip = RecordedClip::default();
        clip.push(RecordedNote {
            note: 72,
            velocity: 120,
            start_step: 0,
            length_steps: 8,
        });
        engine.apply(Command::SetRecordedClip { track: 0, clip });
        engine.apply(Command::SetRecordedActive {
            track: 0,
            active: false,
        });
        for track in 1..TRACK_COUNT {
            engine.apply(Command::SetTrackMute {
                track: track as u8,
                mute: true,
            });
        }
        engine.apply(Command::Play);
        let out = render(&mut engine, 48_000);
        let peak = out.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak < 1e-3, "disabled MIDI clip still sounded ({peak})");
    }

    #[test]
    fn clip_envelope_shapes_the_track() {
        let mut full = Engine::offline(48_000.0);
        for track in 1..TRACK_COUNT {
            full.apply(Command::SetTrackMute {
                track: track as u8,
                mute: true,
            });
        }
        full.apply(Command::Play);
        let full_energy: f32 = render(&mut full, 48_000).iter().map(|s| s * s).sum();

        let mut faded = Engine::offline(48_000.0);
        for track in 1..TRACK_COUNT {
            faded.apply(Command::SetTrackMute {
                track: track as u8,
                mute: true,
            });
        }
        faded.apply(Command::SetTrackClip {
            track: 0,
            gain: 1.0,
            fade_in: 1.0,
            fade_out: 0.0,
        });
        faded.apply(Command::Play);
        let faded_energy: f32 = render(&mut faded, 48_000).iter().map(|s| s * s).sum();

        assert!(full_energy > 0.1);
        assert!(
            faded_energy < full_energy * 0.25,
            "fade-in did not attenuate the clip ({full_energy} vs {faded_energy})"
        );
    }
}
