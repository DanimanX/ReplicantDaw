# Replicant DAW

A cyberpunk DAW prototype in Rust: a real-time FM synthesizer and stereo mix bus
driven by `cpal`, a neon session interface built with `eframe`/`egui` on `wgpu`,
lock-free `rtrb` queues between the threads, MIDI recording, project files and
in-process CLAP/VST3 plugin hosting.

**Version 1.0.0** · MIT · Linux, macOS and Windows.

![Replicant DAW session view](docs/screenshot.png)

> Screenshot captured headlessly from the real application with the silent
> fallback clock (no audio device attached), so the badges show `SILENT FALLBACK`.

![Plugin rack hosting a real VST3 instrument](docs/plugins.png)

## What is implemented

- **Two-thread audio architecture**
  - Audio thread: the `cpal` output callback renders stereo at 48 kHz, 32-bit
    float when the device supports it.
  - UI thread: `eframe` + `egui` with the `wgpu` renderer.
  - Lock-free SPSC `rtrb` rings for commands, telemetry, scope samples, MIDI,
    recording events and plugin audio blocks.
- **Synthesis**
  - 8 tracks with a 32-voice polyphonic two-operator FM engine.
  - Sample-accurate ADSR envelopes and gate lengths per track.
  - Per-track gain, pan, mute and solo; equal-power stereo bus.
  - Generative 16-step clip patterns across 8 scenes in a 16-bar loop.
- **Master FX (device chain)**
  - `NEURAL GEN V5`: drive/colour gain and a high-cut tone filter.
  - `QUANTUM RESONATOR`: resonant topology-preserving state-variable low-pass.
  - `SYNAPSE DRIVE`: saturation drive and mid/side stereo width.
  - Soft clipping, DC blocking and a hard output guard.
- **Project save / load**
  - Versioned JSON `.rproj` files: tempo, mix, scenes, FX, recorded MIDI and
    plugin descriptors with their opaque state (base64).
  - `Ctrl+S` quick save, a project browser, and `--project <path>` on startup.
  - Unknown plugins are reported instead of silently dropped.
- **MIDI recording**
  - Live MIDI input through `midir` (ALSA, CoreMIDI, WinMM), with hot-plug
    rescan and device selection.
  - Arm a track, hit `REC`, and play: notes are captured at 16th-note
    resolution into a clip that replaces the generative pattern for that track.
  - An on-screen two-octave keyboard feeds the exact same MIDI path, so the
    workflow is testable without hardware.
  - Recorded clips are drawn as note blocks in the arrangement lanes.
- **Plugin hosting**
  - **CLAP**: in-process hosting with audio, note/parameter events and state.
  - **VST3**: in-process hosting with audio, MIDI, parameters and state.
  - **VST2**: discovered and listed, but not hostable because the Steinberg SDK
    is proprietary and not vendored (see [docs/PLUGINS.md](docs/PLUGINS.md)).
  - **Audio Unit**: not implemented (macOS only).
  - Four insert slots on the master bus, bypass, per-slot parameter knobs and
    latency reporting.
- **Session UI**
  - Menu bar, transport with bar/beat/step counter, editable BPM, play/stop,
    REC, audio callback CPU meter, master meter and link status.
  - Clip matrix (8 × 8) with live step progress and scene launch.
  - Mixer strips with vertical faders, LED dB meters, pan and mute/solo.
  - Arrangement lanes with a 16-bar ruler, animated playhead and recorded-note
    blocks.
  - Live spectral waterfall computed with a 1024-point Hann-windowed FFT and
    rendered as a coloured 3D mesh with `egui::Painter`.
- **Robust host handling**
  - Prefers a 48 kHz stereo F32 device configuration and converts to the
    device's sample format when needed.
  - If no output device exists, a real-time silent clock keeps the engine,
    meters, FFT and plugin chain running and the UI shows `SILENT FALLBACK`.

## Plugin support

| Format | Scan | Host | Notes |
| --- | --- | --- | --- |
| CLAP | yes | yes | audio + note/param events + state |
| VST3 | yes | yes | audio + MIDI + params + state |
| VST2 | yes | no | proprietary SDK not vendored |
| AU / AUv3 | no | no | macOS-only, not implemented |

Plugins run on a dedicated worker thread, exchanging 512-frame stereo blocks
with the engine through lock-free rings. This keeps `!Send` plugin handles out
of the audio callback and isolates load/state cost from the UI. The trade-off is
one block of latency and the fact that a crashing third-party plugin still
crashes the process: there is no out-of-process sandbox yet.

Details, scan paths and troubleshooting: [docs/PLUGINS.md](docs/PLUGINS.md).

## Requirements

- Rust 1.85 or newer (stable toolchain).
- Linux: ALSA development headers and the usual windowing libraries.

  ```bash
  # Debian/Ubuntu
  sudo apt install build-essential pkg-config libasound2-dev \
      libxkbcommon-dev libwayland-dev libx11-dev libxi-dev

  # Fedora
  sudo dnf install alsa-lib-devel libxkbcommon-devel wayland-devel \
      libX11-devel libXi-devel
  ```

- macOS: Xcode command line tools; no extra packages.
- Windows: MSVC build tools; no extra packages. WASAPI is used through `cpal`.

Fonts and the app icon are embedded, so there are no runtime asset paths.

## Run

```bash
cargo run --release
```

```bash
cargo run --release -- --devices        # list audio output devices
cargo run --release -- --audio-check    # open the default device and verify telemetry
cargo run --release -- --selftest       # DSP, MIDI, plugin-ring and clock smoke test
cargo run --release -- --project session.rproj
cargo test                              # 54 unit tests
cargo clippy --all-targets -- -D warnings
```

Environment variables:

```bash
REPLICANT_DAW_NO_AUDIO=1        # force the silent fallback clock
REPLICANT_DAW_SIZE=1200x760     # initial window size (clamped to the minimum)
REPLICANT_DAW_PROJECT=x.rproj   # load a project at startup
REPLICANT_DAW_PROJECTS=dir      # override the projects directory
REPLICANT_DAW_PLUGIN_PATH=a:b   # extra plugin scan roots
REPLICANT_DAW_OPEN_PLUGIN=/path/to/plugin.vst3   # demo/QA: load into slot 1
REPLICANT_DAW_SMOKE=docs/screenshot.png
REPLICANT_DAW_SMOKE_SECS=4      # auto-play, wait, screenshot, exit
REPLICANT_DAW_SMOKE_WINDOWS=rack,midi            # open utility windows
REPLICANT_DAW_SMOKE_SAVE=/tmp/demo.rproj         # save a project before exiting
WGPU_BACKEND=gl|vulkan|metal|dx12
```

Default project directory:

- Linux: `~/.local/share/replicant-daw/projects`
- macOS: `~/Library/Application Support/ReplicantDaw/projects`
- Windows: `%APPDATA%\ReplicantDaw\projects`

## Controls

- **Space** — play / stop. **Esc** — stop and rewind.
- **Ctrl+S** — save. **Ctrl+O** — project window. **Ctrl+P** — plugin browser.
- **Play / Stop / REC** — transport and MIDI recording on the selected track.
- **BPM** — click to type, drag vertically, or use `-` / `+`.
- **Clip slot** — click to launch, click again (or right-click) to stop. The
  triangle at the left of a row launches the whole scene.
- **Track header** — selects the track (also the MIDI input/record target).
- **Fader** — drag vertically, double-click for 0 dB.
- **Pan knob** — drag vertically, double-click to centre.
- **M / S** — mute and solo.
- **Device knobs** — drag vertically, double-click resets.
- **Rack** — load, bypass or remove plugins; up to 16 parameters per slot.
- **MIDI** — connect an input, play the on-screen keyboard, arm REC, clear a
  clip, toggle MIDI to plugins.

## Architecture

```
┌────────────────────────── UI thread (eframe / egui / wgpu) ─────────────────────────┐
│ menu · transport · clip matrix · mixer · lanes · FFT · device chain · rack · midi    │
│    │ Command              ▲ Telemetry / scope / record events                       │
└────┼──────────────────────┼────────────────────────────────────────────────────────┘
     │ rtrb                 │ rtrb
┌────▼──────────────────────┴────────────────────────────────────────────────────────┐
│ Audio thread: cpal callback (or paced silent clock)                                 │
│ sequencer → FM voices → track mix → drive/tone/SVF/width → master guard             │
│      │ dry blocks                                                ▲ processed blocks │
└──────┼───────────────────────────────────────────────────────────┼──────────────────┘
       │ rtrb                                                      │ rtrb
┌──────▼───────────────────────────────────────────────────────────┴──────────────────┐
│ Plugin worker thread: CLAP + VST3 instances, params, state                          │
└─────────────────────────────────────────────────────────────────────────────────────┘

MIDI hardware ──midir──▶ bridge ring ──UI pump──▶ MIDI ring ──▶ engine (voices + record)
```

- `src/audio/engine.rs` renders audio. It never allocates, locks or calls into
  third-party code; the plugin chain only moves blocks between rings.
- `src/plugins/worker.rs` owns all plugin handles. It is created inside its own
  thread because plugin instances are not `Send`.
- `src/studio.rs` owns MIDI, plugins and projects on the UI side.
- `src/project.rs` is the serialized session model.

## DSP details

- Two-operator phase-modulation voice: `sin(phase + index * env * sin(mod_phase))`,
  with per-track ratio, index and ADSR.
- The sequencer derives the current 16th note from a sample-accurate transport
  position, so tempo changes and loops stay tight.
- Master chain order: saturation → one-pole high-cut → TPT state-variable
  low-pass → mid/side width → gain → soft clip → DC blocker → guard.
- Live parameters are smoothed to avoid zipper noise.
- `src/dsp/spectrum.rs` uses `rustfft` with a 1024-point Hann window and
  log-spaced display bins from 30 Hz to 16 kHz.

## Project format

`.rproj` is versioned JSON:

```json
{
  "format_version": 1,
  "app_version": "1.0.0",
  "name": "Demo",
  "bpm": 142.0,
  "fx": { "gain": 0.9, "tone": 16000.0, "cutoff": 5200.0,
          "resonance": 0.18, "drive": 0.12, "width": 1.0 },
  "tracks": [
    { "name": "SYNAPSE 01", "gain_db": -7.0, "pan": -0.12,
      "mute": false, "solo": false, "scene": 1,
      "notes": [ { "note": 60, "velocity": 100,
                   "start_step": 0, "length_steps": 2 } ] }
  ],
  "plugins": [
    { "slot": 0, "name": "OB-Xd", "format": "VST3",
      "path": "/home/user/.vst3/OB-Xd 3.vst3",
      "bypass": false, "state": "<base64>" }
  ]
}
```

Unknown fields are ignored (forward compatible), while a newer
`format_version` is rejected with a clear error.

## Verification

Verified on Linux (Arch, kernel 7.2) with a real AMD/Radeon desktop, a
Keystation 49 MK3 MIDI keyboard, and third-party plugins already installed:

```
$ cargo test
test result: ok. 54 passed; 0 failed

$ cargo clippy --all-targets -- -D warnings
Finished

$ ./target/release/replicant-daw --selftest
- rendered 8.0 s of audio in 0.106 s (75.7x realtime)
- lock-free path: telemetry playing=true peaks=0.442 | scope frames=32768
- midi recording: 1 note(s), 2 UI event(s)
- midi inputs available: 5
- midi input opened: Midi Through:Midi Through Port-0 14:0
- silent clock: peak 0.646, playing=true
SELFTEST OK

$ ./target/release/replicant-daw --audio-check
AUDIO OK: Alsa | Default Audio Device | 48000 Hz | ... telemetry frames | stopped (silent)
```

Additional manual checks:

- VST3: `OB-Xd 3.vst3` loads on the worker thread, exposes 16 parameters,
  reports latency and produces audio from MIDI.
- CLAP: `Dexed.clap` loads, exposes parameters and processes audio.
- Projects: saved a session with a 250 KB VST3 state blob and loaded it back in
  a fresh process; the plugin and its parameters were restored.
- Screenshots: `docs/screenshot.png` and `docs/plugins.png` are captured from
  the release binary running under Xvfb.

The CI matrix runs tests and the self-test on Ubuntu, macOS (arm64 + x86_64)
and Windows, and each tag publishes platform packages through
`.github/workflows/release-*.yml`.

## Limitations / next steps

This is a prototype, not a production DAW:

- Plugin hosting is in-process: a crashing plugin takes the app down, and the
  plugin worker adds one block of latency. An out-of-process sandbox is the
  natural next step.
- No plugin GUI embedding; parameters are edited with the rack knobs.
- VST2 and Audio Unit hosting are not implemented (see the matrix above).
- No MIDI export, no automation lanes, no undo, no clip editing after
  recording (clear and re-record only).
- Recording replaces the target track's clip; it does not overdub.
- One 16-bar loop and one master insert chain.
- Windows and macOS are covered by CI builds but were not manually auditioned
  for this release; Linux/ALSA, MIDI input, CLAP and VST3 were verified on
  hardware.

## License

MIT. See [LICENSE](LICENSE).

The embedded fonts are licensed under the SIL Open Font License; the license
texts are in [`assets/licenses/`](assets/licenses).
