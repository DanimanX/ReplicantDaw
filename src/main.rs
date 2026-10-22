//! Replicant DAW entry point.
//!
//! Normal launch opens the egui/wgpu interface. `--selftest` runs an offline
//! DSP smoke test and `--devices` lists available output devices.

use std::time::Instant;

use replicant_daw::app::ReplicantApp;
use replicant_daw::audio::{self, Command, Engine};
use replicant_daw::APP_VERSION;

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("replicant_daw=info,warn"),
    )
    .init();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|arg| arg == "--selftest") {
        match selftest() {
            Ok(()) => return,
            Err(error) => {
                eprintln!("SELFTEST FAILED: {error}");
                std::process::exit(2);
            }
        }
    }

    if args.iter().any(|arg| arg == "--devices") {
        list_devices();
        return;
    }

    if args.iter().any(|arg| arg == "--audio-check") {
        match audio_check() {
            Ok(()) => return,
            Err(error) => {
                eprintln!("AUDIO CHECK FAILED: {error}");
                std::process::exit(2);
            }
        }
    }

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Replicant DAW {}", APP_VERSION);
        println!("  replicant-daw              Open the DAW");
        println!("  replicant-daw --selftest   Run an offline engine smoke test");
        println!("  replicant-daw --devices    List audio output devices");
        println!(
            "  replicant-daw --audio-check Open the default device silently and verify telemetry"
        );
        println!();
        println!("Environment:");
        println!("  REPLICANT_DAW_NO_AUDIO=1          Use the silent fallback clock");
        println!("  REPLICANT_DAW_SMOKE=docs/shot.png Capture a screenshot and exit");
        println!("  REPLICANT_DAW_SMOKE_SECS=3        Delay before the screenshot");
        println!("  REPLICANT_DAW_SIZE=1660x1000      Initial window size");
        println!("  WGPU_BACKEND=gl|vulkan|metal|dx12 Select the wgpu backend");
        return;
    }

    if let Err(error) = run_gui() {
        eprintln!("Could not start Replicant DAW: {error}");
        std::process::exit(1);
    }
}

fn run_gui() -> Result<(), eframe::Error> {
    // Allow headless runners and users to override the initial window size.
    let size = std::env::var("REPLICANT_DAW_SIZE")
        .ok()
        .and_then(|value| {
            let (w, h) = value.split_once('x')?;
            Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?])
        })
        .map(|[w, h]| [w.clamp(1200.0, 3840.0), h.clamp(760.0, 2160.0)])
        .unwrap_or([1660.0, 1000.0]);

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(size)
        .with_min_inner_size([1200.0, 760.0])
        .with_title("Replicant DAW // 2035");

    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        multisampling: 0,
        depth_buffer: 0,
        stencil_buffer: 0,
        run_and_return: true,
        ..Default::default()
    };

    eframe::run_native(
        "Replicant DAW",
        options,
        Box::new(|cc| Ok(Box::new(ReplicantApp::new(cc)))),
    )
}

fn selftest() -> Result<(), String> {
    println!("Replicant DAW {} · engine self-test", APP_VERSION);
    println!("- sample rate: 48000 Hz, stereo f32, 16-bar loop");

    let mut engine = Engine::offline(48_000.0);
    engine.apply(Command::Play);

    let frames = 48_000 * 8;
    let block = 512;
    let mut buffer = vec![0.0_f32; block * 2];
    let mut peak = 0.0_f32;
    let mut energy = 0.0_f64;
    let mut samples = 0u64;
    let mut nan_seen = false;

    let started = Instant::now();
    for _ in 0..frames / block {
        engine.process(&mut buffer);
        for sample in &buffer {
            if !sample.is_finite() {
                nan_seen = true;
            }
            peak = peak.max(sample.abs());
            energy += (*sample as f64) * (*sample as f64);
            samples += 1;
        }
    }
    let elapsed = started.elapsed().as_secs_f32();
    let rms = (energy / samples.max(1) as f64).sqrt();

    println!(
        "- rendered 8.0 s of audio in {elapsed:.3} s ({:.1}x realtime)",
        8.0 / elapsed
    );
    println!(
        "- peak {peak:.3} | rms {rms:.4} | voices {}",
        engine.active_voices()
    );
    println!(
        "- position {}/{} frames",
        engine.position_frames(),
        48_000 * 8
    );

    if nan_seen {
        return Err("engine emitted non-finite samples".into());
    }
    if !(0.02..=1.0).contains(&peak) {
        return Err(format!("unexpected peak level: {peak}"));
    }
    if rms < 0.005 {
        return Err(format!("engine is effectively silent: rms {rms}"));
    }

    // Verify the command path and FX knobs through the real ring buffers.
    let (mut command_tx, command_rx) = rtrb::RingBuffer::new(128);
    let (telemetry_tx, mut telemetry_rx) = rtrb::RingBuffer::new(64);
    let (scope_tx, scope_rx) = rtrb::RingBuffer::new(32_768);
    let mut online = Engine::new(
        48_000.0,
        Some(command_rx),
        Some(telemetry_tx),
        Some(scope_tx),
    );
    command_tx.push(Command::Play).map_err(|e| e.to_string())?;
    command_tx
        .push(Command::SetFx {
            param: audio::FxParam::Cutoff,
            value: 900.0,
        })
        .map_err(|e| e.to_string())?;
    command_tx
        .push(Command::SetTrackGain {
            track: 2,
            gain: 0.5,
        })
        .map_err(|e| e.to_string())?;
    for _ in 0..48_000 / block {
        online.process(&mut buffer);
    }
    let telemetry = telemetry_rx.pop().map_err(|_| "no telemetry frames")?;
    let scope_frames = scope_rx.slots();
    println!(
        "- lock-free path: telemetry playing={} peaks={:.3} | scope frames={scope_frames}",
        telemetry.playing,
        telemetry.track_peaks.iter().copied().fold(0.0, f32::max)
    );
    if !telemetry.playing || scope_frames == 0 {
        return Err("rtrb telemetry/scope path did not deliver data".into());
    }

    // The silent clock must produce paced audio for headless verification.
    let (fallback_peak, fallback_playing) = audio::stream::probe_silent_clock()?;
    println!("- silent clock: peak {fallback_peak:.3}, playing={fallback_playing}");
    if !fallback_playing {
        return Err("silent fallback clock did not play".into());
    }

    println!("SELFTEST OK");
    Ok(())
}

fn audio_check() -> Result<(), String> {
    let mut runtime = audio::AudioRuntime::start();
    if let Some(error) = &runtime.error {
        return Err(error.clone());
    }
    std::thread::sleep(std::time::Duration::from_millis(350));
    let mut frames = 0usize;
    while runtime.telemetry_rx.pop().is_ok() {
        frames += 1;
    }
    if frames == 0 {
        return Err("stream opened but produced no telemetry".into());
    }
    println!(
        "AUDIO OK: {} | {} | {} Hz | {} telemetry frames | stopped (silent)",
        runtime.backend, runtime.device_name, runtime.sample_rate, frames
    );
    Ok(())
}

fn list_devices() {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    println!("Default host: {:?}", host.id());
    match host.default_output_device() {
        Some(device) => println!("Default output: {}", device),
        None => println!("Default output: none"),
    }
    match host.output_devices() {
        Ok(devices) => {
            println!("Output devices:");
            for device in devices {
                match device.default_output_config() {
                    Ok(config) => println!(
                        "  {device} · {:?} · {} Hz · {} ch",
                        config.sample_format(),
                        config.sample_rate(),
                        config.channels()
                    ),
                    Err(error) => println!("  {device} · unavailable ({error})"),
                }
            }
        }
        Err(error) => println!("Could not enumerate devices: {error}"),
    }
}
