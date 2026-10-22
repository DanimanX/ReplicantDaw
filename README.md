# Replicant DAW

A cyberpunk DAW prototype in Rust: a real-time FM synthesizer and stereo mix bus
driven by `cpal`, a neon session interface built with `eframe`/`egui` on `wgpu`,
and lock-free `rtrb` queues between the two.

![Replicant DAW session view](docs/screenshot.png)

> Screenshot captured headlessly from the real application with the silent
> fallback clock (no audio device attached), so the badges show `SILENT FALLBACK`.

## What is implemented

- **Two-thread architecture**
  - Audio thread: the `cpal` output callback renders audio at 48 kHz, stereo,
    32-bit float when the device supports it.
  - UI thread: `eframe` + `egui` with the `wgpu` renderer.
  - Communication: two lock-free SPSC `rtrb` rings for commands and telemetry,
    plus a third ring carrying full-rate post-FX samples for the scope/FFT.
- **Synthesis**
  - 8 tracks with a 32-voice polyphonic two-operator FM engine.
  - Sample-accurate ADSR envelopes and gate lengths per track.
  - Per-track gain, pan, mute and solo; equal-power stereo bus.
  - 16-step generative clip patterns across 8 scenes, arranged in a 16-bar loop.
- **Master FX (device chain)**
  - `NEURAL GEN V5`: drive/colour gain and a high-cut tone filter.
  - `QUANTUM RESONATOR`: resonant topology-preserving state-variable low-pass
    with cutoff and resonance.
  - `SYNAPSE DRIVE`: saturation drive and mid/side stereo width.
  - Master output with soft clipping, DC blocking and a hard output guard.
  - Every knob sends a command through `rtrb` and changes the DSP in real time.
- **Session UI**
  - Transport bar with bar/beat/step counter, editable BPM, play/stop,
    audio callback CPU meter, master meter and link status.
  - Clip matrix with 8 scenes × 8 tracks, live step progress and scene launch.
  - Mixer strips with vertical faders, LED dB meters, pan and mute/solo.
  - Arrangement lanes with a 16-bar ruler and animated playhead.
  - Live spectral waterfall computed with a 1024-point Hann-windowed FFT and
    rendered as a coloured 3D mesh with `egui::Painter`.
- **Robust host handling**
  - Prefers a 48 kHz stereo F32 device configuration and converts to the
    device's sample format when needed.
  - If no output device exists (CI, headless, broken audio server), a real-time
    silent clock keeps the engine, meters and FFT running, and the UI shows
    `SILENT FALLBACK`.

## Requirements

- Rust 1.85 or newer (stable toolchain).
- Linux: ALSA development headers and the usual windowing libraries. On
  Debian/Ubuntu:

  ```bash
  sudo apt install build-essential pkg-config libasound2-dev \
      libxkbcommon-dev libwayland-dev libx11-dev libxi-dev
  ```

  Fedora: `sudo dnf install alsa-lib-devel libxkbcommon-devel wayland-devel
  libX11-devel libXi-devel`.

- macOS: Xcode command line tools; no extra packages.
- Windows: MSVC build tools; no extra packages. WASAPI is used through `cpal`.

The UI uses embedded fonts and PNG assets, so there are no runtime asset paths
to configure.

## Run

```bash
cargo run --release
```

Useful commands and environment variables:

```bash
cargo run --release -- --devices       # list output devices
cargo run --release -- --audio-check   # open the default device silently and test telemetry
cargo run --release -- --selftest      # offline DSP/RT smoke test, no window or device
cargo test                             # 34 unit tests
cargo clippy --all-targets -- -D warnings

REPLICANT_DAW_NO_AUDIO=1       # force the silent fallback clock
REPLICANT_DAW_SIZE=1200x760    # initial window size (clamped to the minimum)
REPLICANT_DAW_SMOKE=docs/screenshot.png
REPLICANT_DAW_SMOKE_SECS=4     # auto-play, wait, screenshot, exit
WGPU_BACKEND=gl|vulkan|metal|dx12
```

### Headless screenshot / CI check

`REPLICANT_DAW_SMOKE` starts the demo, waits, requests an egui screenshot and
closes. On a machine without a display, run it under Xvfb and make sure the
Wayland session does not take priority:

```bash
env -u WAYLAND_DISPLAY \
  REPLICANT_DAW_NO_AUDIO=1 \
  REPLICANT_DAW_SIZE=1660x1000 \
  REPLICANT_DAW_SMOKE=docs/screenshot.png \
  WGPU_BACKEND=gl \
  xvfb-run -a -s "-screen 0 1920x1200x24" \
  cargo run --release
```

The process prints `SMOKE_SCREENSHOT_SAVED <path>` and exits. A failure to write
the PNG exits with code 2.

## Controls

- **Space** — play / stop. **Esc** — stop and rewind.
- **Play / Stop buttons** — transport, with the bar counter following the
  engine's sample clock.
- **BPM** — click the value to type, drag vertically to scrub, or use `-` / `+`.
  BPM is sent to the engine immediately.
- **Clip slot** — click to launch; click the running slot or right-click to
  stop it. The triangle at the left of a scene launches that scene on all tracks.
- **Track header** — selects the track (highlighted in the arrangement).
- **Fader** — drag vertically, double-click to return to 0 dB. Peak meters show
  the post-fader signal in dBFS.
- **Pan knob** — drag vertically, double-click to centre.
- **M / S** — mute and solo.
- **Device knobs** — drag vertically, double-click resets. `GAIN`, `CUT`,
  `CUTOFF`, `RESONANCE`, `DRIVE` and `WIDTH` are applied to the master chain
  in real time.
- **Scene launch arrows** — start the whole row.

## Architecture

```
┌──────────────────────────── UI thread (eframe / egui / wgpu) ───────────────────────────┐
│  transport · clip matrix · faders/meters · lanes · FFT mesh · device knobs              │
│         │ command producer                      scope consumer │ telemetry consumer     │
└─────────┼───────────────────────────────────────────────────────┼───────────────────────┘
          │ rtrb<Command> (1024)          rtrb<f32> (32768)       │ rtrb<Telemetry> (256)
┌─────────▼───────────────────────────────────────────────────────▼───────────────────────┐
│  Audio thread: cpal callback (or paced silent-clock thread)                              │
│  command drain → 16-step sequencer → 32 FM voices → track gains/pan/solo → stereo bus   │
│  → drive → tone → SVF filter → width → gain → soft clip → DC block → device             │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

- `src/audio/engine.rs` is the only place that renders audio. It is allocation
  free, lock free and syscall free inside `process`.
- UI → audio: `SetBpm`, `Play`, `Stop`, track gain/pan/mute/solo, clip launch,
  and FX parameters.
- Audio → UI: ~60 Hz telemetry (transport position, per-track peaks, master
  peaks, CPU load, voice count, sample rate) and a post-FX scope stream at the full sample rate. The UI drains the scope and runs the FFT itself, so FFT cost never
  touches the real-time callback.
- `src/audio/stream.rs` owns the cpal stream (or the fallback thread). Failure
  to open a device is not fatal; the app degrades to the silent clock.
- The CPU meter measures the callback duration divided by the block duration.
  It is the real audio-callback load, not a synthetic animation.

## DSP details

- Two-operator phase-modulation voice: `sin(phase + index * env * sin(mod_phase))`.
  Each track has its own FM ratio, modulation index and ADSR.
- The sequencer derives the current 16th note from a sample-accurate transport
  position, so tempo changes and loops stay tight.
- Master chain order: saturation → one-pole high-cut → TPT state-variable
  low-pass → mid/side width → gain → smooth soft clip → DC blocker → guard.
- All live parameters are smoothed to avoid zipper noise.
- `src/dsp/spectrum.rs` uses `rustfft` with a 1024-point Hann window and
  log-spaced display bins from 30 Hz to 16 kHz.

## Project layout

```
src/
  main.rs            entry point, CLI, selftest, icon/window setup
  app.rs             eframe app, thread pump, screenshot harness
  state.rs           UI mirror of engine state, meter ballistics, scope/FFT feed
  theme.rs           palette, fonts, panel chrome, neon styling
  audio/
    engine.rs        sequencer, voices, bus, FX, telemetry
    synth.rs         two-operator FM voice + ADSR
    tracks.rs        track definitions and clip patterns
    messages.rs      Command/Telemetry types
    stream.rs        cpal stream, fallback clock, audio check
  dsp/
    filter.rs        SVF and one-pole filters
    spectrum.rs      FFT analyser + log display bins
  ui/
    transport.rs     top bar
    session.rs       clip matrix + mixer
    lanes.rs         arrangement lanes + FFT waterfall
    devices.rs       bottom device chain
    widgets.rs       knobs, faders, LED meters, chips
assets/fonts         Orbitron, Rajdhani, Share Tech Mono (OFL)
assets/licenses      font licenses
docs/screenshot.png  headless capture of the running app
```

## Verification

The project was verified on Linux with a real ALSA device and with software
rendering for the headless capture:

```
$ cargo test
test result: ok. 34 passed; 0 failed

$ cargo clippy --all-targets -- -D warnings
Finished

$ cargo run --release -- --audio-check
AUDIO OK: Alsa | Default Audio Device | 48000 Hz | ... telemetry frames | stopped (silent)

$ cargo run --release -- --selftest
- rendered 8.0 s of audio in 0.126 s (63.6x realtime)
- peak 0.682 | rms 0.2052 | voices 13
- lock-free path: telemetry playing=true ...
SELFTEST OK
```

The GitHub Actions workflow runs `cargo test`, `cargo build --release` and
Clippy on Ubuntu, macOS and Windows.

## Limitations / next steps

This is a prototype, not a production DAW:

- No project save/load, audio recording, MIDI input or plugin hosting.
- Clip slots are generative demo patterns rather than user-recorded audio.
- One 16-bar loop, one always-on master chain, no automation.
- The silent fallback produces no sound by design; it keeps the UI and FFT
  alive on headless machines.
- The UI state is not persisted between runs.
- Windows and macOS are covered by the CI build matrix but were not manually
  auditioned for this delivery; Linux/ALSA was verified on hardware.

## License

MIT. See [LICENSE](LICENSE).

The embedded fonts are licensed under the SIL Open Font License; the license
texts are in [`assets/licenses/`](assets/licenses).
