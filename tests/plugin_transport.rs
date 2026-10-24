//! Live plugin behaviour that needs an installed CLAP/VST3 instrument.
//!
//! These tests skip silently when the referenced plugin is not present, so CI
//! runners without third-party plugins still pass.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use replicant_daw::audio::{Command, Engine, EngineIo, MidiMessage};
use replicant_daw::plugins::{
    PluginCommand, PluginDescriptor, PluginEvent, PluginFormat, PluginRuntime,
};

/// Third-party instruments are not guaranteed to be safe to instantiate
/// twice at once, so the live-plugin tests run one at a time.
static PLUGIN_LOCK: Mutex<()> = Mutex::new(());

fn find_plugin(candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
}

/// Load a plugin and run the engine for `frames`, returning the peak.
fn render(engine: &mut Engine, frames: usize) -> f32 {
    let block = replicant_daw::plugins::PLUGIN_BLOCK_FRAMES;
    let mut buffer = vec![0.0_f32; block * 2];
    let mut peak = 0.0_f32;
    let mut done = 0;
    while done < frames {
        engine.process(&mut buffer);
        for sample in &buffer {
            peak = peak.max(sample.abs());
        }
        done += block;
    }
    peak
}

/// Load `name` at `path` and wait until the worker reports the slot.
fn load(
    runtime: &mut PluginRuntime,
    engine: &mut Engine,
    path: PathBuf,
    format: PluginFormat,
) -> Vec<replicant_daw::plugins::ParamInfo> {
    runtime.send(PluginCommand::Load {
        slot: 0,
        descriptor: PluginDescriptor {
            name: "Test".into(),
            path,
            format,
        },
        state: None,
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut params: Option<Vec<_>> = None;
    while Instant::now() < deadline {
        engine.process(&mut vec![0.0_f32; 1024]);
        while let Ok(event) = runtime.event_rx.pop() {
            if let PluginEvent::Slots(slots) = event {
                if let Some(slot) = slots.iter().find(|slot| slot.slot == 0) {
                    params = Some(slot.params.clone());
                }
            }
        }
        if params.is_some() {
            return params.unwrap_or_default();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the plugin did not load");
}

/// Wait for the next slot report, refreshing parameters once.
fn refreshed_params(
    runtime: &mut PluginRuntime,
    engine: &mut Engine,
) -> Vec<replicant_daw::plugins::ParamInfo> {
    runtime.send(PluginCommand::RefreshParams);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        engine.process(&mut vec![0.0_f32; 1024]);
        while let Ok(event) = runtime.event_rx.pop() {
            if let PluginEvent::Slots(slots) = event {
                if let Some(slot) = slots.iter().find(|slot| slot.slot == 0) {
                    return slot.params.clone();
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the worker did not report the slot")
}

#[test]
fn a_host_knob_change_reaches_the_plugin() {
    let _guard = PLUGIN_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(path) = find_plugin(&[
        "/usr/lib/clap/Dexed.clap",
        "/usr/local/lib/clap/Dexed.clap",
        "/home/danimanx/.clap/Dexed.clap",
    ]) else {
        eprintln!("skipping: no Dexed installation");
        return;
    };
    let (mut runtime, input, output) = PluginRuntime::start(48_000.0);
    let mut engine = Engine::new_with_io(
        48_000.0,
        EngineIo {
            plugin_input: Some(input),
            plugin_output: Some(output),
            ..EngineIo::default()
        },
    );
    let params = load(&mut runtime, &mut engine, path, PluginFormat::Clap);
    let Some(first) = params.first().cloned() else {
        eprintln!("skipping: the plugin exposes no parameters");
        return;
    };
    engine.apply(Command::SetTrackPlugin {
        track: 0,
        active: true,
        instrument: true,
    });

    // Pick a target clearly away from the current value.
    let target = if first.value > 0.5 { 0.1 } else { 0.9 };
    runtime.send(PluginCommand::SetParam {
        slot: 0,
        id: first.id,
        value: target,
    });
    // Deliver the parameter event, then let the worker re-read the plugin.
    render(&mut engine, 48_000);

    let after = refreshed_params(&mut runtime, &mut engine);
    let value = after
        .iter()
        .find(|param| param.id == first.id)
        .map(|param| param.value)
        .unwrap_or(f32::NAN);
    eprintln!(
        "param {} ({}) {:.3} -> {:.3}",
        first.id, first.name, first.value, value
    );
    assert!(
        (value - target).abs() < 0.05,
        "the plugin ignored the host knob change: {value} vs {target}"
    );
}

/// A held live note must not survive Stop, for both hosted formats.
fn held_note_is_released(path: Option<PathBuf>, format: PluginFormat) {
    let Some(path) = path else {
        eprintln!("skipping: plugin not installed");
        return;
    };
    let (mut midi_tx, midi_rx) = rtrb::RingBuffer::new(64);
    let (mut runtime, input, output) = PluginRuntime::start(48_000.0);
    let mut engine = Engine::new_with_io(
        48_000.0,
        EngineIo {
            midi: Some(midi_rx),
            plugin_input: Some(input),
            plugin_output: Some(output),
            ..EngineIo::default()
        },
    );
    load(&mut runtime, &mut engine, path, format);
    engine.apply(Command::SetTrackPlugin {
        track: 0,
        active: true,
        instrument: true,
    });
    engine.apply(Command::Play);

    // A key held down with no note-off, like a controller or the on-screen
    // keyboard when the pointer leaves the key.
    midi_tx
        .push(MidiMessage::NoteOn {
            channel: 0,
            note: 72,
            velocity: 100,
        })
        .expect("queue note-on");
    let playing = render(&mut engine, 48_000);
    eprintln!("held note peak {playing:.4}");

    engine.apply(Command::Stop);
    render(&mut engine, 9_600);
    let stopped = render(&mut engine, 96_000);
    eprintln!("after stop peak {stopped:.6}");
    assert!(
        stopped < 1e-3,
        "the held note keeps sounding after Stop (peak {stopped})"
    );
}

#[test]
fn stopping_the_transport_releases_a_held_note_in_clap() {
    let _guard = PLUGIN_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    held_note_is_released(
        find_plugin(&[
            "/usr/lib/clap/Dexed.clap",
            "/usr/local/lib/clap/Dexed.clap",
            "/home/danimanx/.clap/Dexed.clap",
        ]),
        PluginFormat::Clap,
    );
}

#[test]
fn stopping_the_transport_releases_a_held_note_in_vst3() {
    let _guard = PLUGIN_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    held_note_is_released(
        find_plugin(&[
            "/usr/lib/vst3/Dexed.vst3",
            "/usr/local/lib/vst3/Dexed.vst3",
            "/home/danimanx/.vst3/Dexed.vst3",
        ]),
        PluginFormat::Vst3,
    );
}

#[test]
fn a_stopped_transport_silences_the_hosted_plugin() {
    let _guard = PLUGIN_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(path) = find_plugin(&[
        "/usr/lib/clap/Dexed.clap",
        "/usr/lib/vst3/Dexed.vst3",
        "/usr/local/lib/clap/Dexed.clap",
    ]) else {
        eprintln!("skipping: no Dexed installation");
        return;
    };
    let format = if path.extension().is_some_and(|ext| ext == "clap") {
        PluginFormat::Clap
    } else {
        PluginFormat::Vst3
    };
    let (mut runtime, input, output) = PluginRuntime::start(48_000.0);
    let mut engine = Engine::new_with_io(
        48_000.0,
        EngineIo {
            plugin_input: Some(input),
            plugin_output: Some(output),
            ..EngineIo::default()
        },
    );
    runtime.send(PluginCommand::Load {
        slot: 0,
        descriptor: PluginDescriptor {
            name: "Dexed".into(),
            path,
            format,
        },
        state: None,
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut loaded = false;
    while Instant::now() < deadline {
        engine.process(&mut vec![0.0_f32; 1024]);
        while let Ok(event) = runtime.event_rx.pop() {
            if let PluginEvent::Slots(slots) = event {
                loaded |= slots.iter().any(|slot| slot.slot == 0);
            }
        }
        if loaded {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(loaded, "the plugin did not load");

    engine.apply(Command::SetTrackPlugin {
        track: 0,
        active: true,
        instrument: true,
    });
    engine.apply(Command::Play);
    let playing = render(&mut engine, 96_000);
    eprintln!("playing peak {playing:.4}");
    assert!(playing > 0.001, "the plugin never produced audio");

    engine.apply(Command::Stop);
    // Let one block round-trip finish, then measure the stopped tail.
    render(&mut engine, 4_800);
    let stopped = render(&mut engine, 96_000);
    eprintln!("stopped peak {stopped:.6}");
    assert!(
        stopped < 1e-3,
        "the plugin keeps sounding after Stop (peak {stopped})"
    );
}

/// A CLAP parameter whose range is not `0..1` must still follow the host knob.
///
/// CLAP automation carries the **plain** value (`min_value .. max_value`), not
/// the knob position. Sending the normalized value made every u-he instrument
/// ignore the edit: the plugin read `0.8` as 0.8 Hz (or 0.8 semitones) and the
/// knob looked like it did nothing. The 0..1 plugins that other tests use
/// cannot catch that, so this one asks a wide-range plugin to move.
#[test]
fn a_host_knob_change_reaches_a_wide_range_clap_parameter() {
    let _guard = PLUGIN_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(path) = find_plugin(&[
        "/home/danimanx/.clap/u-he/Bazille.64.clap",
        "/home/danimanx/.clap/u-he/Diva.64.clap",
        "/home/danimanx/.clap/u-he/Hive.64.clap",
        "/home/danimanx/.clap/u-he/Zebra2.64.clap",
        "/home/danimanx/.clap/u-he/Repro-1.64.clap",
        "/Library/Audio/Plug-Ins/CLAP/u-he/Diva.clap",
    ]) else {
        eprintln!("skipping: no wide-range CLAP instrument installed");
        return;
    };
    let (mut runtime, input, output) = PluginRuntime::start(48_000.0);
    let mut engine = Engine::new_with_io(
        48_000.0,
        EngineIo {
            plugin_input: Some(input),
            plugin_output: Some(output),
            ..EngineIo::default()
        },
    );
    let params = load(&mut runtime, &mut engine, path.clone(), PluginFormat::Clap);
    let Some(first) = params.first().cloned() else {
        eprintln!("skipping: the plugin exposes no parameters");
        return;
    };
    engine.apply(Command::SetTrackPlugin {
        track: 0,
        active: true,
        instrument: true,
    });

    let target = if first.value > 0.5 { 0.2 } else { 0.8 };
    runtime.send(PluginCommand::SetParam {
        slot: 0,
        id: first.id,
        value: target,
    });
    render(&mut engine, 48_000);

    let after = refreshed_params(&mut runtime, &mut engine);
    let value = after
        .iter()
        .find(|param| param.id == first.id)
        .map(|param| param.value)
        .unwrap_or(f32::NAN);
    eprintln!(
        "{} param {} ({}) {:.3} -> {:.3} (target {:.3})",
        path.display(),
        first.id,
        first.name,
        first.value,
        value,
        target
    );
    assert!(
        (value - target).abs() < 0.05,
        "the plugin ignored the host knob change: {value} vs {target}"
    );
}
