//! The audio engine: sequencer, synth voices, stereo mix bus and master FX.
//!
//! `Engine` is deliberately free of locks, allocations and syscalls in
//! [`Engine::process`]. It is driven either by the cpal output callback or by
//! the silent fallback clock when no audio device is available.

use rtrb::{Consumer, Producer};

use super::effects::EffectSlot;
use std::sync::Arc;

use super::messages::{
    db_to_gain, ArrangementClip, Command, EffectKind, RecordEvent, RecordedClip, RecordedNote,
    SampleBuffer, SampleUpload, Telemetry, LOOP_STEPS, MAX_EFFECTS, PREVIEW_TRACK, SCENE_COUNT,
    STEPS_PER_BAR, TRACK_COUNT,
};
use super::midi::MidiMessage;
#[cfg(test)]
use super::synth_xt::{SynthXt, Voice};
#[cfg(test)]
use super::tracks::TrackSpec;
use super::tracks::{note_at, TRACKS};
use crate::dsp::{soft_clip, DcBlocker, Smoothed};
use crate::plugins::{PluginBlock, PLUGIN_BLOCK_FRAMES};

pub const MAX_VOICES: usize = 32;
pub const MAX_ARRANGEMENT_CLIPS: usize = 64;
pub const LOOP_BARS: u64 = 16;
pub const DEFAULT_BPM: f32 = 142.0;
pub const SAMPLE_RATE: u32 = 48_000;

/// Flat index of one session clip inside [`Engine::recorded`].
#[inline]
fn clip_index(track: usize, scene: usize) -> usize {
    track.min(TRACK_COUNT - 1) * SCENE_COUNT + scene.min(SCENE_COUNT - 1)
}

/// Heap-allocated grid of empty session clips.
///
/// Built through `Vec` so the big array is never materialised on the stack,
/// which matters on Windows' 1 MiB main-thread stack.
fn empty_clip_grid() -> Box<[RecordedClip]> {
    let mut clips = Vec::with_capacity(TRACK_COUNT * SCENE_COUNT);
    clips.resize_with(TRACK_COUNT * SCENE_COUNT, RecordedClip::default);
    clips.into_boxed_slice()
}

/// Heap-allocated sample slots, one per session clip.
fn empty_sample_grid() -> Box<[Option<Arc<SampleBuffer>>]> {
    let mut samples = Vec::with_capacity(TRACK_COUNT * SCENE_COUNT);
    samples.resize_with(TRACK_COUNT * SCENE_COUNT, || None);
    samples.into_boxed_slice()
}

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
    /// Playhead inside the launched clip's loop region.
    clip_step: u16,
    /// Playback position of the slot's audio sample, in frames.
    sample_pos: usize,
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
            clip_step: 0,
            sample_pos: 0,
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
    /// Rendered samples assigned to clip slots and the editor preview.
    pub samples: Option<Consumer<SampleUpload>>,
}

pub struct Engine {
    sample_rate: f32,
    bpm: f32,
    playing: bool,
    position: u64,
    last_step: u64,
    tracks: [TrackState; TRACK_COUNT],
    #[cfg(test)]
    synth: [SynthXt; TRACK_COUNT],
    #[cfg(test)]
    voices: [Voice; MAX_VOICES],
    #[cfg(test)]
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
    /// Loop region of each clip, as `(start, end)` in 16th notes.
    clip_loop: [[(u16, u16); SCENE_COUNT]; TRACK_COUNT],
    /// One MIDI clip per (track, session slot), flattened on the heap.
    ///
    /// An inline `[[RecordedClip; 8]; 8]` is ~190 KiB, which made `Engine`
    /// exceed the 1 MiB main-thread stack Windows gives a process as soon as
    /// the selftest created a few engines. `clip_index` keeps the addressing.
    recorded: Box<[RecordedClip]>,
    recorded_active: [[bool; SCENE_COUNT]; TRACK_COUNT],
    recording: bool,
    record_track: u8,
    record_scene: u8,
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
    /// Whether the insert on that track generates its own audio (instrument)
    /// or just processes the bus the internal synth feeds (effect).
    track_plugin_instrument: [bool; TRACK_COUNT],
    /// Internal synth audible per track (test-only legacy path).
    #[cfg(test)]
    synth_enabled: [bool; TRACK_COUNT],
    /// Note-offs scheduled for plugins at a given loop step.
    plugin_offs: [(u8, u8, u16); 128],
    plugin_off_count: usize,
    last_midi_note: u8,
    sample_rx: Option<Consumer<SampleUpload>>,
    /// One audio sample per (track, scene) clip.
    clip_samples: Box<[Option<Arc<SampleBuffer>>]>,
    /// Audio clips placed on the arrangement timeline.
    arrangement: [Option<ArrangementClip>; MAX_ARRANGEMENT_CLIPS],
    arrangement_len: usize,
    /// Repeat each slot's sample to fill its clip loop.
    sample_loop: [[bool; SCENE_COUNT]; TRACK_COUNT],
    /// Linear trim per slot, precomputed from the dB command.
    sample_gain: [[f32; SCENE_COUNT]; TRACK_COUNT],
    /// One-shot sample preview from the editor.
    preview: Option<Arc<SampleBuffer>>,
    preview_pos: usize,
    preview_playing: bool,
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
            #[cfg(test)]
            synth: std::array::from_fn(SynthXt::from_track),
            #[cfg(test)]
            voices: [Voice::default(); MAX_VOICES],
            #[cfg(test)]
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
            clip_loop: [[(0, LOOP_STEPS as u16); SCENE_COUNT]; TRACK_COUNT],
            recorded: empty_clip_grid(),
            recorded_active: [[true; SCENE_COUNT]; TRACK_COUNT],
            recording: false,
            record_track: 0,
            record_scene: 0,
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
            track_plugin_instrument: [false; TRACK_COUNT],
            #[cfg(test)]
            synth_enabled: [true; TRACK_COUNT],
            plugin_offs: [(0, 0, 0); 128],
            plugin_off_count: 0,
            last_midi_note: 60,
            sample_rx: io.samples,
            clip_samples: empty_sample_grid(),
            arrangement: std::array::from_fn(|_| None),
            arrangement_len: 0,
            sample_loop: [[true; SCENE_COUNT]; TRACK_COUNT],
            sample_gain: [[1.0; SCENE_COUNT]; TRACK_COUNT],
            preview: None,
            preview_pos: 0,
            preview_playing: false,
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

    /// Editable synth parameters for a track (test-only legacy path).
    #[cfg(test)]
    pub fn synth_params(&self, track: usize) -> SynthXt {
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
        #[cfg(test)]
        {
            self.voices.iter().filter(|v| v.is_active()).count() as u8
        }
        #[cfg(not(test))]
        {
            0
        }
    }

    /// Render interleaved stereo audio. The slice length must be even.
    pub fn process(&mut self, out: &mut [f32]) {
        self.drain_commands();
        self.drain_samples();
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
        #[cfg(test)]
        // Patches are copyable and small; taking one snapshot per block keeps
        // the per-sample loop free of borrow juggling and cache misses.
        let patches = self.synth;

        for frame in out.chunks_exact_mut(2) {
            if self.playing {
                self.advance_sequencer();
            }

            let mut track_l = [0.0_f32; TRACK_COUNT];
            let mut track_r = [0.0_f32; TRACK_COUNT];

            // Render voices into their track buses (test-only legacy path).
            #[cfg(test)]
            for voice in &mut self.voices {
                if !voice.is_active() {
                    continue;
                }
                let track = voice.track as usize;
                if track >= TRACK_COUNT {
                    continue;
                }
                let (left, right) = voice.next_sample(&patches[track], self.sample_rate);
                track_l[track] += left;
                track_r[track] += right;
            }

            // Audio samples play from their clip slot, one-shot per launch
            // (and retriggered when the 16-bar session wraps).
            if self.playing {
                for index in 0..TRACK_COUNT {
                    if !self.tracks[index].active {
                        continue;
                    }
                    let scene = self.tracks[index].scene.min(SCENE_COUNT - 1);
                    if !self.recorded_active[index][scene] {
                        continue;
                    }
                    let slot = clip_index(index, scene);
                    let Some(sample) = self.clip_samples[slot].as_ref() else {
                        continue;
                    };
                    let frames = sample.frame_count();
                    if frames == 0 {
                        continue;
                    }
                    let mut position = self.tracks[index].sample_pos;
                    if position >= frames {
                        if !self.sample_loop[index][scene] {
                            continue;
                        }
                        // Loop: the sample repeats inside the clip region
                        // until the region wraps and retriggers it.
                        position %= frames;
                    }
                    let gain = self.sample_gain[index][scene];
                    track_l[index] += sample.frames[position * 2] * gain;
                    track_r[index] += sample.frames[position * 2 + 1] * gain;
                    self.tracks[index].sample_pos = position + 1;
                }

                // Arrangement clips play at their absolute timeline position,
                // independent of which session slot a track has launched.
                let step_frames = self.samples_per_step();
                let step_pos = self.position as f64 / step_frames;
                for index in 0..TRACK_COUNT {
                    let (left, right) = self.arrangement_sample(index, step_pos, step_frames);
                    track_l[index] += left;
                    track_r[index] += right;
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

            // The editor's preview is independent of the session: it is not
            // routed through a track and is added after the master chain.
            if self.preview_playing {
                let frame = self.preview.as_ref().and_then(|sample| {
                    let position = self.preview_pos;
                    if position < sample.frame_count() {
                        Some((sample.frames[position * 2], sample.frames[position * 2 + 1]))
                    } else {
                        None
                    }
                });
                match frame {
                    Some((preview_l, preview_r)) => {
                        l += preview_l;
                        r += preview_r;
                        self.preview_pos += 1;
                    }
                    None => {
                        self.preview_playing = false;
                        self.preview_pos = 0;
                    }
                }
            }

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

        // Speaker protection: the engine should never emit NaN or
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
            recorded_notes: if self.recording {
                self.clip(self.record_track as usize, self.record_scene as usize)
                    .len
            } else {
                self.recorded.iter().map(|clip| clip.len).sum()
            },
            plugin_latency: self.plugin_latency,
            clip_steps: std::array::from_fn(|track| self.tracks[track].clip_step),
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

    /// Receive rendered sample buffers from the UI.
    ///
    /// Replacing a slot drops the engine's handle to the previous buffer on
    /// the audio thread, but only for an explicit UI edit.
    fn drain_samples(&mut self) {
        let Some(mut rx) = self.sample_rx.take() else {
            return;
        };
        for _ in 0..64 {
            match rx.pop() {
                Ok(upload) => {
                    if upload.track == PREVIEW_TRACK {
                        self.preview = upload.sample;
                        self.preview_pos = 0;
                        self.preview_playing = self.preview.is_some();
                    } else if (upload.track as usize) < TRACK_COUNT {
                        let track = upload.track as usize;
                        let scene = (upload.scene as usize).min(SCENE_COUNT - 1);
                        self.clip_samples[clip_index(track, scene)] = upload.sample;
                        self.tracks[track].sample_pos = 0;
                    }
                }
                Err(_) => break,
            }
        }
        self.sample_rx = Some(rx);
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
                for track in 0..TRACK_COUNT {
                    let scene = self.tracks[track].scene.min(SCENE_COUNT - 1);
                    self.tracks[track].clip_step = self.clip_loop[track][scene].0;
                    self.tracks[track].sample_pos = 0;
                }
                #[cfg(test)]
                for voice in &mut self.voices {
                    voice.release();
                }
                self.preview_playing = false;
                self.preview_pos = 0;
                // Release any notes still held by a hosted instrument.
                for index in 0..self.plugin_off_count {
                    let (bus, note, _) = self.plugin_offs[index];
                    self.plugin_pending.push_event(bus, 0, [0x80, note, 0], 3);
                }
                self.plugin_off_count = 0;
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
                    if (scene as usize) < SCENE_COUNT {
                        t.scene = scene as usize;
                        t.active = true;
                        // A launched clip restarts at the beginning of its
                        // loop region, like a clip launch in any DAW.
                        t.clip_step = self.clip_loop[track as usize][scene as usize].0;
                        t.sample_pos = 0;
                    }
                }
            }
            Command::StopClip { track } => {
                if let Some(t) = self.tracks.get_mut(track as usize) {
                    t.active = false;
                    #[cfg(test)]
                    for voice in &mut self.voices {
                        if voice.is_active() && voice.track == track {
                            voice.release();
                        }
                    }
                }
            }
            Command::RecordStart { track, scene } => {
                let track = (track as usize).min(TRACK_COUNT - 1);
                let scene = (scene as usize).min(SCENE_COUNT - 1);
                self.record_track = track as u8;
                self.record_scene = scene as u8;
                // Recording goes into the slot that is playing on this track.
                self.tracks[track].scene = scene;
                self.tracks[track].active = true;
                self.clip_mut(track, scene).clear();
                self.recording = true;
                self.record_pending = None;
                if !self.playing {
                    self.playing = true;
                }
                self.emit_record_reset(track as u8, scene as u8);
            }
            Command::RecordStop => {
                self.finalize_pending_note(self.loop_step);
                self.recording = false;
            }
            Command::ClearRecorded { track, scene } => {
                let track = (track as usize).min(TRACK_COUNT - 1);
                let scene = (scene as usize).min(SCENE_COUNT - 1);
                self.clip_mut(track, scene).clear();
                self.emit_record_reset(track as u8, scene as u8);
            }
            Command::SetRecordedClip { track, scene, clip } => {
                let track = (track as usize).min(TRACK_COUNT - 1);
                let scene = (scene as usize).min(SCENE_COUNT - 1);
                *self.clip_mut(track, scene) = clip;
            }
            Command::SetClipLoop {
                track,
                scene,
                start,
                end,
            } => {
                let track = (track as usize).min(TRACK_COUNT - 1);
                let scene = (scene as usize).min(SCENE_COUNT - 1);
                // Order and clamp: the UI drags both handles, so either one
                // can arrive first.
                let total = LOOP_STEPS as u16;
                let (lo, hi) = if start <= end {
                    (start, end)
                } else {
                    (end, start)
                };
                let start = lo.min(total - 1);
                let end = hi.clamp(start + 1, total);
                self.clip_loop[track][scene] = (start, end);
                self.tracks[track].clip_step = self.tracks[track].clip_step.clamp(start, end - 1);
            }
            Command::StopPreview => {
                self.preview_playing = false;
                self.preview_pos = 0;
            }
            Command::SetSampleLoop {
                track,
                scene,
                enabled,
            } => {
                let track = (track as usize).min(TRACK_COUNT - 1);
                let scene = (scene as usize).min(SCENE_COUNT - 1);
                self.sample_loop[track][scene] = enabled;
            }
            Command::SetSampleGain {
                track,
                scene,
                gain_db,
            } => {
                let track = (track as usize).min(TRACK_COUNT - 1);
                let scene = (scene as usize).min(SCENE_COUNT - 1);
                if gain_db.is_finite() {
                    self.sample_gain[track][scene] = db_to_gain(gain_db.clamp(-48.0, 12.0));
                }
            }
            Command::SetPluginMidi { enabled } => {
                self.plugin_midi = enabled;
            }
            Command::SetInputTrack { track } => {
                self.record_track = (track as usize).min(TRACK_COUNT - 1) as u8;
            }
            Command::SetTrackPlugin {
                track,
                active,
                instrument,
            } => {
                let track = (track as usize).min(TRACK_COUNT - 1);
                if let Some(flag) = self.track_plugin.get_mut(track) {
                    *flag = active;
                    self.track_plugin_instrument[track] = instrument;
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
                // One instrument per track: an active instrument insert owns
                // the sound source, so the internal synth steps aside.
                #[cfg(test)]
                if active && instrument {
                    self.synth_enabled[track] = false;
                    self.stop_synth_voices(track);
                }
                // Dropping the insert must not leave notes held inside the
                // instance: they would resume the moment it is routed again.
                if !active && self.plugin_midi && self.track_plugin.iter().any(|enabled| *enabled) {
                    self.plugin_pending
                        .push_event(track as u8, 0, [0xB0, 123, 0], 3);
                }
            }
            #[cfg(test)]
            Command::SetTrackSynth { track, enabled } => {
                let track = (track as usize).min(TRACK_COUNT - 1);
                if let Some(flag) = self.synth_enabled.get_mut(track) {
                    *flag = enabled;
                }
                if !enabled {
                    // Muting the source must also stop what is already ringing.
                    self.stop_synth_voices(track);
                } else if self.track_plugin[track] && self.track_plugin_instrument[track] {
                    // Choosing the synth drops an instrument insert. An effect
                    // insert keeps processing the bus the synth feeds.
                    self.track_plugin[track] = false;
                    if !self.track_plugin.iter().any(|enabled| *enabled) {
                        self.plugin_pending.frames = 0;
                        self.plugin_pending.clear_events();
                        self.plugin_ready_len = 0;
                        self.plugin_ready_pos = 0;
                        self.plugin_latency = 0;
                    }
                }
            }
            Command::SetArrangement { clips } => {
                let count = clips.len().min(MAX_ARRANGEMENT_CLIPS);
                let mut incoming = clips.into_iter();
                for slot in self.arrangement.iter_mut().take(count) {
                    *slot = incoming.next();
                }
                for slot in self.arrangement.iter_mut().skip(count) {
                    *slot = None;
                }
                self.arrangement_len = count;
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
            #[cfg(test)]
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
            Command::SetRecordedActive {
                track,
                scene,
                active,
            } => {
                if let Some(slots) = self.recorded_active.get_mut(track as usize) {
                    if let Some(flag) = slots.get_mut(scene as usize) {
                        *flag = active;
                    }
                }
            }
        }
    }

    /// Audio from arrangement clips whose timeline range contains `step_pos`.
    #[inline]
    fn arrangement_sample(&self, track: usize, step_pos: f64, step_frames: f64) -> (f32, f32) {
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;
        for clip in self.arrangement[..self.arrangement_len].iter().flatten() {
            if clip.track as usize != track {
                continue;
            }
            let start = clip.start_step as f64;
            let end = start + clip.length_steps.max(1) as f64;
            if step_pos < start || step_pos >= end {
                continue;
            }
            let frames = clip.sample.frame_count();
            if frames == 0 {
                continue;
            }
            let offset = ((step_pos - start) * step_frames).round().max(0.0) as usize;
            let position = if offset >= frames {
                if clip.loop_enabled {
                    offset % frames
                } else {
                    continue;
                }
            } else {
                offset
            };
            let gain = clip.gain.clamp(0.0, 2.0);
            left += clip.sample.frames[position * 2] * gain;
            right += clip.sample.frames[position * 2 + 1] * gain;
        }
        (left, right)
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
            // A new pass over the 16 bars retriggers sample clips, matching
            // the MIDI clips that restart inside their loop regions.
            for track in &mut self.tracks {
                track.sample_pos = 0;
            }
            for index in 0..self.plugin_off_count {
                let (bus, note, _) = self.plugin_offs[index];
                let offset = self.plugin_pending.frames.saturating_sub(1);
                self.plugin_pending
                    .push_event(bus, offset, [0x80, note, 0], 3);
            }
            self.plugin_off_count = 0;
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

        // Release plugin notes whose gate ended on this step.
        self.release_plugin_notes(loop_step);

        for (track, spec) in TRACKS.iter().enumerate() {
            let _ = spec;
            if !self.tracks[track].active {
                continue;
            }

            // Recorded MIDI replaces the generative pattern for this slot, and
            // plays inside its loop region on its own playhead.
            let scene = self.tracks[track].scene.min(SCENE_COUNT - 1);
            let clip = *self.clip(track, scene);
            let (start, end) = self.clip_loop[track][scene];
            let region = (end - start).max(1);
            let phase = self.tracks[track].clip_step.clamp(start, end - 1);
            if !clip.is_empty() {
                let muted = !self.recorded_active[track][scene];

                if !muted {
                    for note in clip.as_slice() {
                        if note.start_step != phase {
                            continue;
                        }
                        let gate = (step_samples * note.length_steps.max(1) as f64)
                            .round()
                            .max(1.0) as u32;
                        #[cfg(test)]
                        {
                            let velocity = (note.velocity as f32 / 127.0).clamp(0.05, 1.0);
                            self.trigger_note(track, note.note, velocity, gate, spec);
                        }
                        let _ = gate;
                        self.forward_note_on(track, note.note, note.velocity);
                        // When the note ends, expressed on the global step grid.
                        let delta = (note.end_step().saturating_sub(phase)) % region;
                        self.schedule_plugin_off(track, note.note, loop_step + delta);
                    }
                }

                // Advance the clip playhead, wrapping inside the region.
                let next = phase + 1;
                self.tracks[track].clip_step = if next >= end { start } else { next };
                continue;
            }

            // Empty slots can still hold an audio sample. Their playhead has
            // to advance too, so the waveform cursor follows the audio, and
            // the sample restarts when the clip's own region wraps.
            let next = phase + 1;
            self.tracks[track].clip_step = if next >= end { start } else { next };
            if next >= end {
                self.tracks[track].sample_pos = 0;
            }

            // A sample slot replaces the generative pattern: otherwise the
            // internal synth would keep playing under the dragged sample.
            if self.clip_samples[clip_index(track, scene)].is_some() {
                continue;
            }

            let Some(note) = note_at(track, scene, step_in_bar) else {
                continue;
            };
            #[cfg(test)]
            {
                let gate = (step_samples * spec.gate as f64).round().max(1.0) as u32;
                let velocity = if step_in_bar % 4 == 0 { 1.0 } else { 0.82 };
                self.trigger_note(track, note, velocity, gate, spec);
            }
            let velocity_byte = if step_in_bar % 4 == 0 { 100 } else { 82 };
            self.forward_note_on(track, note, velocity_byte);
            self.schedule_plugin_off(track, note, loop_step + 1);
        }
    }

    /// Send a sequencer note to the track's plugin insert.
    fn forward_note_on(&mut self, track: usize, note: u8, velocity: u8) {
        if self.plugin_input.is_none() || !self.plugin_midi {
            return;
        }
        // A track whose insert is not routed must not collect notes: they
        // would otherwise fire the moment the insert is routed again.
        if !self.track_plugin.get(track).copied().unwrap_or(false) {
            return;
        }
        let offset = self.plugin_pending.frames.saturating_sub(1);
        self.plugin_pending.push_event(
            track as u8,
            offset,
            [0x90, note, velocity.clamp(1, 127)],
            3,
        );
    }

    /// Queue a note-off for the plugin at `end_step` of the 16-bar loop.
    fn schedule_plugin_off(&mut self, track: usize, note: u8, end_step: u16) {
        if self.plugin_input.is_none() || !self.plugin_midi || self.plugin_off_count >= 128 {
            return;
        }
        if !self.track_plugin.get(track).copied().unwrap_or(false) {
            return;
        }
        let end_step = end_step % LOOP_STEPS as u16;
        self.plugin_offs[self.plugin_off_count] = (track as u8, note, end_step);
        self.plugin_off_count += 1;
    }

    /// Emit pending plugin note-offs and drop them from the schedule.
    fn release_plugin_notes(&mut self, loop_step: u16) {
        let mut index = 0;
        while index < self.plugin_off_count {
            if self.plugin_offs[index].2 == loop_step {
                let (bus, note, _) = self.plugin_offs[index];
                let offset = self.plugin_pending.frames.saturating_sub(1);
                self.plugin_pending
                    .push_event(bus, offset, [0x80, note, 0], 3);
                self.plugin_off_count -= 1;
                self.plugin_offs[index] = self.plugin_offs[self.plugin_off_count];
            } else {
                index += 1;
            }
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
                #[cfg(test)]
                {
                    let _ = spec;
                    let velocity_norm = (velocity as f32 / 127.0).clamp(0.05, 1.0);
                    // Live notes stay open until the matching note-off.
                    self.trigger_note(track, note, velocity_norm, u32::MAX / 4, spec);
                }
                let _ = track;
                let _ = spec;
                self.forward_midi([0x90, note, velocity], 3);
            }
            MidiMessage::NoteOff { note, channel: _ } => {
                if let Some(pending) = self.record_pending {
                    if pending.note == note {
                        self.finalize_pending_note(self.loop_step);
                    }
                }
                #[cfg(test)]
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
        let scene = self.record_scene as usize;

        self.clip_mut(track, scene).push(pending);
        self.emit_record_note(track as u8, self.record_scene, pending, false);
    }

    fn emit_record_reset(&mut self, track: u8, scene: u8) {
        if let Some(tx) = &mut self.record_tx {
            let _ = tx.push(RecordEvent {
                track,
                scene,
                note: RecordedNote::default(),
                reset: true,
            });
        }
    }

    fn emit_record_note(&mut self, track: u8, scene: u8, note: RecordedNote, reset: bool) {
        if let Some(tx) = &mut self.record_tx {
            let _ = tx.push(RecordEvent {
                track,
                scene,
                note,
                reset,
            });
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

    /// Loop region of one clip (tests and diagnostics).
    pub fn clip_loop(&self, track: usize, scene: usize) -> (u16, u16) {
        self.clip_loop[track.min(TRACK_COUNT - 1)][scene.min(SCENE_COUNT - 1)]
    }

    /// Currently recorded clip for one session slot (tests and diagnostics).
    pub fn recorded_clip(&self, track: usize, scene: usize) -> &RecordedClip {
        self.clip(track, scene)
    }

    /// Address one session clip. Out-of-range indices clamp to the last slot.
    #[inline]
    fn clip(&self, track: usize, scene: usize) -> &RecordedClip {
        &self.recorded[clip_index(track, scene)]
    }

    #[inline]
    fn clip_mut(&mut self, track: usize, scene: usize) -> &mut RecordedClip {
        let index = clip_index(track, scene);
        &mut self.recorded[index]
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// Fade out every sounding voice of `track` and free their slots.
    ///
    /// Called when the track's sound source changes, so the internal synth
    /// cannot keep ringing under a newly selected instrument plugin.
    #[cfg(test)]
    fn stop_synth_voices(&mut self, track: usize) {
        for voice in &mut self.voices {
            if voice.is_active() && voice.track as usize == track {
                voice.stop(self.sample_rate);
            }
        }
        // Notes scheduled from before the switch would re-open silent voices.
        let mut index = 0;
        while index < self.plugin_off_count {
            if self.plugin_offs[index].0 as usize == track {
                self.plugin_off_count -= 1;
                self.plugin_offs[index] = self.plugin_offs[self.plugin_off_count];
            } else {
                index += 1;
            }
        }
    }

    #[cfg(test)]
    fn trigger_note(
        &mut self,
        track: usize,
        note: u8,
        velocity: f32,
        gate: u32,
        _spec: &TrackSpec,
    ) {
        let track = track.min(TRACK_COUNT - 1);
        if !self.synth_enabled[track] {
            return;
        }
        let params = self.synth[track];
        let limit = (params.polyphony.round() as usize).clamp(1, MAX_VOICES);
        let sounding = self
            .voices
            .iter()
            .filter(|voice| voice.is_active() && voice.track as usize == track)
            .count();
        // A free slot, unless this track already reached its polyphony limit.
        let slot = if sounding < limit {
            self.voices.iter().position(|v| !v.is_active())
        } else {
            None
        }
        .or_else(|| {
            // Steal the quietest voice of this track, then the oldest.
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, voice)| voice.is_active() && voice.track as usize == track)
                .min_by(|(_, a), (_, b)| {
                    a.envelope_level()
                        .total_cmp(&b.envelope_level())
                        .then_with(|| a.age().cmp(&b.age()))
                })
                .map(|(i, _)| i)
        })
        .or_else(|| {
            // Fallback for a full pool: steal the quietest anywhere.
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
    use crate::audio::messages::{ArrangementClip, SynthParam, SCENE_COUNT};
    use rtrb::RingBuffer;

    fn render(engine: &mut Engine, frames: usize) -> Vec<f32> {
        let mut buffer = vec![0.0; frames * 2];
        engine.process(&mut buffer);
        buffer
    }

    /// The Windows main thread gets a 1 MiB stack, and the selftest plus
    /// project load keep several engines alive at once.
    #[test]
    fn engine_fits_a_windows_sized_stack() {
        let size = std::mem::size_of::<Engine>();
        assert!(
            size < 128 * 1024,
            "Engine grew to {size} bytes; keep large state on the heap"
        );

        let handle = std::thread::Builder::new()
            .name("engine-stack-check".into())
            .stack_size(1024 * 1024)
            .spawn(|| {
                // Four *stack* locals, mirroring `selftest` and project load.
                let mut a = Engine::offline(48_000.0);
                let mut b = Engine::offline(48_000.0);
                let mut c = Engine::offline(48_000.0);
                let mut d = Engine::offline(48_000.0);
                let mut buffer = vec![0.0_f32; 4_096];
                let mut engines = [&mut a, &mut b, &mut c, &mut d];
                let mut ok = true;
                for engine in engines.iter_mut() {
                    engine.apply(Command::Play);
                    engine.process(&mut buffer);
                    ok &= engine.is_playing() && engine.effect_count() == 4;
                }
                ok
            })
            .expect("spawn stack-check thread");
        assert!(handle.join().expect("engine thread panicked"));
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
    fn sample_clip_plays_when_its_slot_is_launched() {
        let (mut sample_tx, sample_rx) = RingBuffer::new(4);
        let mut engine = Engine::new_with_io(
            48_000.0,
            EngineIo {
                samples: Some(sample_rx),
                ..EngineIo::default()
            },
        );
        // Silence the internal synths: only the sample should make sound.
        for track in 0..TRACK_COUNT {
            engine.apply(Command::SetTrackSynth {
                track: track as u8,
                enabled: false,
            });
        }
        engine.apply(Command::Play);

        let frames = 9_600usize;
        let mut data = Vec::with_capacity(frames * 2);
        for index in 0..frames {
            let value = (std::f32::consts::TAU * 440.0 * index as f32 / 48_000.0).sin() * 0.5;
            data.push(value);
            data.push(value);
        }
        sample_tx
            .push(SampleUpload {
                track: 0,
                scene: 2,
                sample: Some(Arc::new(SampleBuffer {
                    frames: data,
                    sample_rate: 48_000,
                })),
            })
            .expect("queue sample");
        engine.apply(Command::LaunchClip { track: 0, scene: 2 });

        let out = render(&mut engine, 1024);
        let peak = out
            .iter()
            .fold(0.0_f32, |max, sample| max.max(sample.abs()));
        assert!(peak > 0.05, "sample produced no audio: {peak}");
        assert!(engine.tracks[0].sample_pos > 0);
    }

    #[test]
    fn arrangement_clip_plays_at_its_timeline_position() {
        let (mut sample_tx, sample_rx) = RingBuffer::new(4);
        let mut engine = Engine::new_with_io(
            48_000.0,
            EngineIo {
                samples: Some(sample_rx),
                ..EngineIo::default()
            },
        );
        // Silence the test-only synth path and the generative patterns.
        for track in 0..TRACK_COUNT {
            engine.apply(Command::SetTrackSynth {
                track: track as u8,
                enabled: false,
            });
        }
        engine.apply(Command::SetBpm(120.0));
        engine.apply(Command::Play);

        let frames = 48_000usize;
        let mut data = Vec::with_capacity(frames * 2);
        for index in 0..frames {
            let value = (std::f32::consts::TAU * 330.0 * index as f32 / 48_000.0).sin() * 0.5;
            data.push(value);
            data.push(value);
        }
        // At 120 BPM a 16th note is 0.125 s: step 16 is second 2.
        engine.apply(Command::SetArrangement {
            clips: vec![ArrangementClip {
                track: 0,
                start_step: 16,
                length_steps: 16,
                sample: Arc::new(SampleBuffer {
                    frames: data,
                    sample_rate: 48_000,
                }),
                loop_enabled: true,
                gain: 1.0,
            }],
        });
        let _ = &mut sample_tx;

        let before = render(&mut engine, 72_000);
        assert!(peak(&before) < 1e-4, "arrangement clip started early");

        let during = render(&mut engine, 48_000);
        assert!(peak(&during) > 0.05, "arrangement clip did not sound");

        let _ = render(&mut engine, 96_000);
        let after = render(&mut engine, 24_000);
        assert!(
            peak(&after) < 1e-4,
            "arrangement clip kept sounding after its end"
        );
    }

    #[test]
    fn preview_plays_without_a_session_clip() {
        let (mut sample_tx, sample_rx) = RingBuffer::new(4);
        let mut engine = Engine::new_with_io(
            48_000.0,
            EngineIo {
                samples: Some(sample_rx),
                ..EngineIo::default()
            },
        );
        for track in 0..TRACK_COUNT {
            engine.apply(Command::SetTrackSynth {
                track: track as u8,
                enabled: false,
            });
        }
        // Not playing: the preview must sound on its own.
        let frames = 4_800usize;
        let mut data = Vec::with_capacity(frames * 2);
        for index in 0..frames {
            let value = (std::f32::consts::TAU * 220.0 * index as f32 / 48_000.0).sin() * 0.5;
            data.push(value);
            data.push(value);
        }
        sample_tx
            .push(SampleUpload {
                track: PREVIEW_TRACK,
                scene: 0,
                sample: Some(Arc::new(SampleBuffer {
                    frames: data,
                    sample_rate: 48_000,
                })),
            })
            .expect("queue preview");
        let out = render(&mut engine, 512);
        let peak = out
            .iter()
            .fold(0.0_f32, |max, sample| max.max(sample.abs()));
        assert!(peak > 0.05, "preview produced no audio: {peak}");
        engine.apply(Command::StopPreview);
        let out = render(&mut engine, 512);
        let peak = out
            .iter()
            .fold(0.0_f32, |max, sample| max.max(sample.abs()));
        assert!(peak < 1e-4, "preview did not stop: {peak}");
    }

    #[test]
    fn sample_loops_inside_the_clip_or_plays_once() {
        let (mut sample_tx, sample_rx) = RingBuffer::new(4);
        let mut engine = Engine::new_with_io(
            48_000.0,
            EngineIo {
                samples: Some(sample_rx),
                ..EngineIo::default()
            },
        );
        for track in 0..TRACK_COUNT {
            engine.apply(Command::SetTrackSynth {
                track: track as u8,
                enabled: false,
            });
            engine.apply(Command::StopClip { track: track as u8 });
        }
        engine.apply(Command::Play);

        // 0.1 s of a steady tone.
        let frames = 4_800usize;
        let mut data = Vec::with_capacity(frames * 2);
        for index in 0..frames {
            let value = (std::f32::consts::TAU * 220.0 * index as f32 / 48_000.0).sin() * 0.5;
            data.push(value);
            data.push(value);
        }
        sample_tx
            .push(SampleUpload {
                track: 0,
                scene: 0,
                sample: Some(Arc::new(SampleBuffer {
                    frames: data,
                    sample_rate: 48_000,
                })),
            })
            .expect("queue sample");
        engine.apply(Command::LaunchClip { track: 0, scene: 0 });

        // After the sample's own length the loop keeps it audible.
        let _ = render(&mut engine, 9_600);
        let tail = render(&mut engine, 4_800);
        let peak = tail
            .iter()
            .fold(0.0_f32, |max, sample| max.max(sample.abs()));
        assert!(peak > 0.05, "looping sample fell silent: {peak}");

        // Turning the loop off leaves the region silent after one pass.
        engine.apply(Command::SetSampleLoop {
            track: 0,
            scene: 0,
            enabled: false,
        });
        engine.apply(Command::LaunchClip { track: 0, scene: 0 });
        let _ = render(&mut engine, 7_200);
        let tail = render(&mut engine, 4_800);
        let peak = tail
            .iter()
            .fold(0.0_f32, |max, sample| max.max(sample.abs()));
        assert!(peak < 1e-4, "one-shot sample kept repeating: {peak}");
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

    /// Solo track 0 so a source switch is measured on its own bus.
    fn solo_track_zero(engine: &mut Engine) {
        for track in 1..TRACK_COUNT {
            engine.apply(Command::SetTrackMute {
                track: track as u8,
                mute: true,
            });
        }
    }

    /// Voices the internal synth still has sounding on one track.
    fn voices_on(engine: &Engine, track: u8) -> usize {
        engine
            .voices
            .iter()
            .filter(|voice| voice.is_active() && voice.track == track)
            .count()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0_f32, |max, s| max.max(s.abs()))
    }

    #[test]
    fn an_instrument_insert_replaces_the_internal_synth() {
        let mut engine = Engine::offline(48_000.0);
        solo_track_zero(&mut engine);
        engine.apply(Command::Play);
        let before = render(&mut engine, 24_000);
        assert!(peak(&before) > 0.01, "the synth should be sounding");
        assert!(voices_on(&engine, 0) > 0);

        engine.apply(Command::SetTrackPlugin {
            track: 0,
            active: true,
            instrument: true,
        });
        // The fade-out is 12 ms; give it room, then the track must be silent.
        let _ = render(&mut engine, 4_800);
        assert_eq!(
            voices_on(&engine, 0),
            0,
            "the synth kept voices alive under an instrument plugin"
        );
        let after = render(&mut engine, 24_000);
        assert!(
            peak(&after) < 1.0e-6,
            "the internal synth is still audible under the plugin (peak {})",
            peak(&after)
        );
    }

    #[test]
    fn an_effect_insert_keeps_the_internal_synth_audible() {
        let mut engine = Engine::offline(48_000.0);
        solo_track_zero(&mut engine);
        engine.apply(Command::Play);
        engine.apply(Command::SetTrackPlugin {
            track: 0,
            active: true,
            instrument: false,
        });
        let out = render(&mut engine, 24_000);
        assert!(
            peak(&out) > 0.01,
            "an effect insert must still receive the synth bus"
        );
        assert!(voices_on(&engine, 0) > 0);
    }

    #[test]
    fn choosing_the_synth_drops_the_instrument_insert() {
        let mut engine = Engine::offline(48_000.0);
        solo_track_zero(&mut engine);
        engine.apply(Command::Play);
        engine.apply(Command::SetTrackPlugin {
            track: 0,
            active: true,
            instrument: true,
        });
        let silent = render(&mut engine, 24_000);
        assert!(peak(&silent) < 1.0e-6, "the plugin path should be silent");

        engine.apply(Command::SetTrackSynth {
            track: 0,
            enabled: true,
        });
        let out = render(&mut engine, 24_000);
        assert!(
            peak(&out) > 0.01,
            "selecting the synth must bring it back and drop the insert"
        );
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
        engine.apply(Command::RecordStart { track: 2, scene: 0 });
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

        let clip = engine.recorded_clip(2, 0);
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
    fn recording_targets_only_the_requested_slot() {
        let mut engine = Engine::offline(48_000.0);
        engine.apply(Command::RecordStart { track: 3, scene: 5 });
        assert_eq!(engine.record_track, 3);
        assert_eq!(engine.record_scene, 5);
        assert_eq!(engine.tracks[3].scene, 5);

        let mut clip = RecordedClip::default();
        clip.push(RecordedNote {
            note: 50,
            velocity: 100,
            start_step: 0,
            length_steps: 2,
        });
        engine.apply(Command::SetRecordedClip {
            track: 3,
            scene: 5,
            clip,
        });
        assert_eq!(engine.recorded_clip(3, 5).len, 1);
        for scene in 0..SCENE_COUNT {
            if scene != 5 {
                assert!(
                    engine.recorded_clip(3, scene).is_empty(),
                    "slot {scene} should be empty"
                );
            }
        }
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
        engine.apply(Command::SetRecordedClip {
            track: 0,
            scene: 0,
            clip,
        });
        engine.apply(Command::LaunchClip { track: 0, scene: 0 });
        engine.apply(Command::SetBpm(120.0));
        engine.apply(Command::Play);
        let out = render(&mut engine, 12_000);
        let peak = out
            .iter()
            .fold(0.0_f32, |max, sample| max.max(sample.abs()));
        assert!(peak > 0.01, "recorded clip did not sound (peak {peak})");
        assert_eq!(engine.recorded_clip(0, 0).len, 1);
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
            // An effect insert: it processes the bus the synth feeds.
            instrument: false,
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
            param: SynthParam::FilterCutoff(0),
            value: 14_000.0,
        });
        bright.apply(Command::Play);
        let bright_out = render(&mut bright, 24_000);
        let bright_energy: f32 = bright_out.iter().map(|s| s * s).sum();

        let mut mellow = Engine::offline(48_000.0);
        mellow.apply(Command::SetSynthParam {
            track: 0,
            param: SynthParam::FilterCutoff(0),
            value: 120.0,
        });
        mellow.apply(Command::Play);
        let mellow_out = render(&mut mellow, 24_000);
        let mellow_energy: f32 = mellow_out.iter().map(|s| s * s).sum();

        assert!(bright_energy.is_finite() && mellow_energy.is_finite());
        assert!(
            (bright_energy - mellow_energy).abs() > 1.0,
            "synth parameter had no audible effect ({bright_energy} vs {mellow_energy})"
        );
        assert!((bright.synth_params(0).get(SynthParam::FilterCutoff(0)) - 14_000.0).abs() < 1e-3);
    }

    #[test]
    fn every_preset_sounds_on_a_launched_track() {
        use crate::audio::PRESETS;
        for preset in PRESETS.iter() {
            let mut engine = Engine::offline(48_000.0);
            for track in 0..TRACK_COUNT {
                engine.apply(Command::StopClip { track: track as u8 });
            }
            let patch = preset.patch();
            for param in SynthParam::all() {
                engine.apply(Command::SetSynthParam {
                    track: 0,
                    param,
                    value: patch.get(param),
                });
            }
            engine.apply(Command::Play);
            // A long gate so slow pads reach their body before release.
            engine.trigger_note(0, 60, 1.0, 96_000, &TRACKS[0]);
            // Long enough for the slowest attack in the factory bank.
            let out = render(&mut engine, 72_000);
            let peak = out
                .iter()
                .fold(0.0_f32, |max, sample| max.max(sample.abs()));
            let rms =
                (out.iter().map(|sample| sample * sample).sum::<f32>() / out.len() as f32).sqrt();
            assert!(
                peak > 0.002 && rms > 5.0e-5,
                "preset {} produced no audio (peak {peak}, rms {rms})",
                preset.name
            );
            assert!(
                out.iter().all(|sample| sample.is_finite()),
                "preset {} produced non-finite samples",
                preset.name
            );
        }
    }

    #[test]
    fn polyphony_limit_steals_within_the_track() {
        let mut engine = Engine::offline(48_000.0);
        engine.apply(Command::SetSynthParam {
            track: 0,
            param: SynthParam::Polyphony,
            value: 2.0,
        });
        engine.apply(Command::Play);
        for note in 60..66 {
            engine.trigger_note(0, note, 1.0, 48_000, &TRACKS[0]);
        }
        let sounding = engine
            .voices
            .iter()
            .filter(|voice| voice.is_active() && voice.track == 0)
            .count();
        assert!(
            sounding <= 2,
            "polyphony limit ignored: {sounding} voices sounding"
        );
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
        engine.apply(Command::SetRecordedClip {
            track: 0,
            scene: 0,
            clip,
        });
        engine.apply(Command::SetRecordedActive {
            track: 0,
            scene: 0,
            active: false,
        });
        // Point the track at the muted slot so nothing else sounds.
        engine.apply(Command::LaunchClip { track: 0, scene: 0 });
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
    fn clip_loop_repeats_only_its_region() {
        let mut engine = Engine::offline(48_000.0);
        // Two notes per bar: one inside the region, one outside.
        let mut clip = RecordedClip::default();
        clip.push(RecordedNote {
            note: 60,
            velocity: 100,
            start_step: 16,
            length_steps: 2,
        });
        clip.push(RecordedNote {
            note: 72,
            velocity: 100,
            start_step: 64,
            length_steps: 2,
        });
        engine.apply(Command::SetRecordedClip {
            track: 0,
            scene: 0,
            clip,
        });
        engine.apply(Command::SetClipLoop {
            track: 0,
            scene: 0,
            start: 16,
            end: 32,
        });
        engine.apply(Command::LaunchClip { track: 0, scene: 0 });
        engine.apply(Command::SetBpm(120.0));
        engine.apply(Command::Play);

        // Bar 1 of the region plays its note; the note outside never fires.
        let mut buffer = vec![0.0; 512 * 2];
        let mut triggered_notes = Vec::new();
        for _ in 0..(4 * 48_000 / 512) {
            engine.process(&mut buffer);
            for voice in &engine.voices {
                // Only the clip's track matters here; the other seven play
                // their generative patterns.
                if voice.is_active() && voice.track == 0 && !triggered_notes.contains(&voice.note())
                {
                    triggered_notes.push(voice.note());
                }
            }
        }
        assert!(
            triggered_notes.contains(&60),
            "note inside the loop region did not play: {triggered_notes:?}"
        );
        assert!(
            !triggered_notes.contains(&72),
            "note outside the loop region played: {triggered_notes:?}"
        );
        // The playhead stays inside the region across many loops.
        assert!((16..32).contains(&engine.tracks[0].clip_step));
    }

    #[test]
    fn launching_a_clip_restarts_its_loop() {
        let mut engine = Engine::offline(48_000.0);
        engine.apply(Command::SetClipLoop {
            track: 0,
            scene: 2,
            start: 40,
            end: 48,
        });
        engine.apply(Command::LaunchClip { track: 0, scene: 2 });
        assert_eq!(engine.tracks[0].clip_step, 40);

        let mut clip = RecordedClip::default();
        clip.push(RecordedNote {
            note: 55,
            velocity: 100,
            start_step: 40,
            length_steps: 1,
        });
        engine.apply(Command::SetRecordedClip {
            track: 0,
            scene: 2,
            clip,
        });
        engine.apply(Command::Stop);
        assert_eq!(engine.tracks[0].clip_step, 40);
        // Out-of-range regions are ordered and clamped.
        engine.apply(Command::SetClipLoop {
            track: 0,
            scene: 2,
            start: 90,
            end: 10,
        });
        assert_eq!(engine.clip_loop(0, 2), (10, 90));
    }

    #[test]
    fn session_slots_play_their_own_clip() {
        let mut engine = Engine::offline(48_000.0);
        let mut clip = RecordedClip::default();
        clip.push(RecordedNote {
            note: 48,
            velocity: 110,
            start_step: 0,
            length_steps: 8,
        });
        engine.apply(Command::SetRecordedClip {
            track: 0,
            scene: 3,
            clip,
        });
        // Track 0 defaults to scene 1, which has no clip: the generative
        // pattern still plays there.
        engine.apply(Command::Play);
        render(&mut engine, 2_400);
        assert!(engine.tracks[0].active);
        assert_eq!(engine.recorded_clip(0, 3).len, 1);
        assert!(engine.recorded_clip(0, 4).is_empty());

        // Launching the slot with the clip uses it.
        engine.apply(Command::LaunchClip { track: 0, scene: 3 });
        let out = render(&mut engine, 24_000);
        assert!(out.iter().any(|sample| sample.abs() > 0.005));
    }

    #[test]
    fn muting_the_synth_silences_the_track() {
        let mut engine = Engine::offline(48_000.0);
        engine.apply(Command::SetTrackSynth {
            track: 0,
            enabled: false,
        });
        for track in 1..TRACK_COUNT {
            engine.apply(Command::SetTrackMute {
                track: track as u8,
                mute: true,
            });
        }
        engine.apply(Command::Play);
        let out = render(&mut engine, 24_000);
        let peak = out.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak < 1e-4, "muted synth still sounded ({peak})");

        let mut restored = Engine::offline(48_000.0);
        restored.apply(Command::Play);
        let out = render(&mut restored, 24_000);
        let peak = out.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.01);
    }

    #[test]
    fn sequencer_notes_reach_the_plugin_insert() {
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
        engine.apply(Command::SetTrackPlugin {
            track: 0,
            active: true,
            instrument: true,
        });
        engine.apply(Command::Play);

        let mut note_on = 0;
        let mut note_off = 0;
        let mut buffer = vec![0.0; PLUGIN_BLOCK_FRAMES * 2];
        for _ in 0..40 {
            engine.process(&mut buffer);
            while let Ok(block) = dry_rx.pop() {
                for event in &block.events[..block.event_count as usize] {
                    if event.bus == 0 && event.bytes[0] == 0x90 {
                        note_on += 1;
                    }
                    if event.bus == 0 && event.bytes[0] == 0x80 {
                        note_off += 1;
                    }
                }
                let _ = wet_tx.push(block);
            }
        }
        assert!(note_on > 0, "no note-on events reached the plugin");
        assert!(note_off > 0, "no note-off events reached the plugin");
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
