//! Lock-free audio runtime: a cpal 48 kHz output stream plus a silent
//! fallback clock for machines without an audio device.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

use super::engine::{Engine, EngineIo, SAMPLE_RATE};
use super::messages::{Command, SampleUpload, Telemetry};

const COMMAND_CAPACITY: usize = 1024;
const TELEMETRY_CAPACITY: usize = 256;
const SCOPE_CAPACITY: usize = 32_768;
const SAMPLE_CAPACITY: usize = 256;

pub struct AudioRuntime {
    pub command_tx: Producer<Command>,
    pub telemetry_rx: Consumer<Telemetry>,
    pub scope_rx: Consumer<f32>,
    /// Rendered samples assigned to clip slots and the editor preview.
    pub sample_tx: Producer<SampleUpload>,
    /// Human-readable host summary, e.g. `ALSA / Built-in Audio`.
    pub backend: String,
    pub device_name: String,
    pub sample_rate: u32,
    pub error: Option<String>,
    _stream: Option<Stream>,
    stop: Option<Arc<AtomicBool>>,
    worker: Option<JoinHandle<()>>,
}

impl AudioRuntime {
    pub fn start() -> Self {
        Self::start_with_io(EngineIo::default())
    }

    /// Start with extra engine side channels (MIDI, recording, plugins).
    pub fn start_with_io(io: EngineIo) -> Self {
        if std::env::var_os("REPLICANT_DAW_NO_AUDIO").is_some() {
            return Self::start_silent_with_io(
                Some("audio disabled by REPLICANT_DAW_NO_AUDIO".into()),
                io,
            );
        }
        match start_cpal(io) {
            Ok(runtime) => runtime,
            Err((error, io)) => {
                log::warn!("Audio device unavailable ({error}); using the silent clock");
                Self::start_silent_with_io(Some(error), io)
            }
        }
    }

    /// A real-time paced engine with no device, used for headless runs and as
    /// a graceful fallback. The UI and FFT stay alive; only sound is missing.
    pub fn start_silent(error: Option<String>) -> Self {
        Self::start_silent_with_io(error, EngineIo::default())
    }

    pub fn start_silent_with_io(error: Option<String>, mut io: EngineIo) -> Self {
        let (command_tx, command_rx) = RingBuffer::new(COMMAND_CAPACITY);
        let (telemetry_tx, telemetry_rx) = RingBuffer::new(TELEMETRY_CAPACITY);
        let (scope_tx, scope_rx) = RingBuffer::new(SCOPE_CAPACITY);
        let (sample_tx, sample_rx) = RingBuffer::new(SAMPLE_CAPACITY);

        io.commands = Some(command_rx);
        io.telemetry = Some(telemetry_tx);
        io.scope = Some(scope_tx);
        io.samples = Some(sample_rx);

        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let mut engine = Engine::new_with_io(SAMPLE_RATE as f32, io);

        let worker = thread::Builder::new()
            .name("replicant-silent-clock".into())
            .spawn(move || {
                const BLOCK: usize = 480; // 10 ms at 48 kHz
                let mut buffer = vec![0.0_f32; BLOCK * 2];
                let mut next = Instant::now();
                while !stop_flag.load(Ordering::Relaxed) {
                    let started = Instant::now();
                    engine.process(&mut buffer);
                    let load = started.elapsed().as_secs_f32()
                        / (BLOCK as f32 / SAMPLE_RATE as f32)
                        * 100.0;
                    engine.set_cpu_load(load);

                    next += Duration::from_secs_f64(BLOCK as f64 / SAMPLE_RATE as f64);
                    let now = Instant::now();
                    if next > now {
                        thread::sleep(next - now);
                    } else {
                        next = now;
                    }
                }
            })
            .expect("spawn silent audio thread");

        Self {
            command_tx,
            telemetry_rx,
            scope_rx,
            sample_tx,
            backend: "SILENT CLOCK".into(),
            device_name: "No output device".into(),
            sample_rate: SAMPLE_RATE,
            error,
            _stream: None,
            stop: Some(stop),
            worker: Some(worker),
        }
    }

    pub fn is_silent(&self) -> bool {
        self.error.is_some() || self._stream.is_none()
    }
}

impl Drop for AudioRuntime {
    fn drop(&mut self) {
        if let Some(stream) = self._stream.take() {
            drop(stream);
        }
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[allow(clippy::result_large_err)]
fn start_cpal(mut io: EngineIo) -> Result<AudioRuntime, (String, EngineIo)> {
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        return Err(("no default output device".to_string(), io));
    };
    let device_name = device.to_string();
    let supported = match choose_config(&device) {
        Ok(config) => config,
        Err(error) => return Err((error, io)),
    };
    let sample_format = supported.sample_format();
    let sample_rate = supported.sample_rate();
    let config: StreamConfig = supported.config();

    log::info!(
        "Audio: {device_name} | {:?} | {} Hz | {} ch",
        sample_format,
        sample_rate,
        config.channels
    );

    let (command_tx, command_rx) = RingBuffer::new(COMMAND_CAPACITY);
    let (telemetry_tx, telemetry_rx) = RingBuffer::new(TELEMETRY_CAPACITY);
    let (scope_tx, scope_rx) = RingBuffer::new(SCOPE_CAPACITY);
    let (sample_tx, sample_rx) = RingBuffer::new(SAMPLE_CAPACITY);

    io.commands = Some(command_rx);
    io.telemetry = Some(telemetry_tx);
    io.scope = Some(scope_tx);
    io.samples = Some(sample_rx);
    let engine = Engine::new_with_io(sample_rate as f32, io);

    let stream = build_stream(&device, config, sample_format, engine)
        .map_err(|error| (error, EngineIo::default()))?;
    stream.play().map_err(|error| {
        (
            format!("could not start stream: {error}"),
            EngineIo::default(),
        )
    })?;

    let backend = format!("{:?}", host.id());
    Ok(AudioRuntime {
        command_tx,
        telemetry_rx,
        scope_rx,
        sample_tx,
        backend,
        device_name,
        sample_rate,
        error: None,
        _stream: Some(stream),
        stop: None,
        worker: None,
    })
}

/// Prefer a 48 kHz stereo float stream. Fall back to the device default on
/// hardware that cannot provide it, converting samples as needed.
fn choose_config(device: &cpal::Device) -> Result<SupportedStreamConfig, String> {
    let configs: Vec<_> = device
        .supported_output_configs()
        .map_err(|error| format!("could not query output configs: {error}"))?
        .collect();

    let has_48k = |c: &&cpal::SupportedStreamConfigRange| {
        c.min_sample_rate() <= SAMPLE_RATE && c.max_sample_rate() >= SAMPLE_RATE
    };

    let preferred = configs
        .iter()
        .find(|c| c.sample_format() == SampleFormat::F32 && c.channels() == 2 && has_48k(c))
        .map(|c| c.with_sample_rate(SAMPLE_RATE))
        .or_else(|| {
            configs
                .iter()
                .find(|c| c.channels() == 2 && has_48k(c))
                .map(|c| c.with_sample_rate(SAMPLE_RATE))
        });

    match preferred {
        Some(config) => Ok(config),
        None => device
            .default_output_config()
            .map_err(|error| format!("no usable output config: {error}")),
    }
}

fn build_stream(
    device: &cpal::Device,
    config: StreamConfig,
    format: SampleFormat,
    engine: Engine,
) -> Result<Stream, String> {
    macro_rules! build {
        ($sample:ty) => {
            build_typed::<$sample>(device, config, engine)
        };
    }
    match format {
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I24 => build!(cpal::I24),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U24 => build!(cpal::U24),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U64 => build!(u64),
        other => Err(format!("unsupported sample format: {other:?}")),
    }
}

fn build_typed<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut engine: Engine,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32> + 'static,
{
    let channels = config.channels as usize;
    let sample_rate = engine.sample_rate();
    // Allocated once, reused by every callback.
    let mut scratch = vec![0.0_f32; 8192 * 2];

    let error_callback = |error| log::error!("Audio stream error: {error}");

    device
        .build_output_stream(
            config,
            move |data: &mut [T], _info: &cpal::OutputCallbackInfo| {
                let started = Instant::now();
                let frames = data.len() / channels.max(1);
                let mut rendered = 0;

                while rendered < frames {
                    let chunk = (frames - rendered).min(scratch.len() / 2);
                    let slice = &mut scratch[..chunk * 2];
                    engine.process(slice);

                    for (frame, source) in data[rendered * channels..(rendered + chunk) * channels]
                        .chunks_exact_mut(channels)
                        .zip(slice.chunks_exact(2))
                    {
                        for (channel, out) in frame.iter_mut().enumerate() {
                            let value = match channel {
                                0 => source[0],
                                1 => source[1],
                                _ => 0.0,
                            };
                            *out = T::from_sample(value);
                        }
                    }
                    rendered += chunk;
                }

                let budget = frames.max(1) as f32 / sample_rate;
                engine.set_cpu_load(started.elapsed().as_secs_f32() / budget * 100.0);
            },
            error_callback,
            None,
        )
        .map_err(|error| format!("could not build output stream: {error}"))
}

/// Exercise the fallback clock in `--selftest` without opening cpal.
pub fn probe_silent_clock() -> Result<(f32, bool), String> {
    let mut runtime = AudioRuntime::start_silent(None);
    runtime
        .command_tx
        .push(Command::Play)
        .map_err(|error| error.to_string())?;
    thread::sleep(Duration::from_millis(160));

    let mut peak = 0.0_f32;
    let mut playing = false;
    let mut received = false;
    while let Ok(frame) = runtime.telemetry_rx.pop() {
        received = true;
        peak = peak.max(frame.master_peak[0]).max(frame.master_peak[1]);
        playing = frame.playing;
    }
    if !received {
        return Err("fallback clock produced no telemetry".into());
    }
    Ok((peak, playing))
}
