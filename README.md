# Replicant DAW

A cyberpunk DAW prototype in Rust: a real-time FM synthesizer and stereo mix bus
driven by `cpal`, a neon session interface built with `eframe`/`egui` on `wgpu`,
lock-free `rtrb` queues between the threads, MIDI recording, project files, a
modular effect chain and in-process CLAP/VST3 plugin hosting.

**Version 1.0.3** · MIT · Linux, macOS and Windows.

![Replicant DAW session view](docs/screenshot.png)

> Captured headlessly from the real release binary with the silent fallback
> clock (no audio device attached), so the badges show `SILENT FALLBACK`.
> The session shows recorded MIDI clips in the matrix and arrangement, and a
> five-module effect chain.

![FM synth editor at 125% UI zoom](docs/synth.png)

![Piano roll clip editor](docs/piano-roll.png)

![Plugin rack hosting a real VST3 instrument](docs/plugins.png)

## What's new in 1.0.3

- **Piano roll clip editor** — double-click any clip cell in the session matrix
  (or the MIDI row) to open a 16-bar piano roll. Click the grid to add notes,
  drag the body to move them, drag the right edge to change length, right-click
  to delete, and edit velocity per note. Edits are sent to the engine and saved
  with the project.
- **Arrangement editing** — double-click a MIDI clip in the arrangement to open
  the same piano roll, or double-click an audio clip/lane to open the audio clip
  editor with waveform, clip gain, fade-in and fade-out. The envelope is applied
  to the rendered track audio and persisted in the project.
- **Per-track plugin inserts** — the device chain now includes a `PLUGIN INSERT`
  module for the selected track. Load a CLAP or VST3 plugin there (or from the
  rack/browser) and it stays assigned to that track, before the master effect
  chain. The rack shows all eight track inserts and their parameter knobs.
- **Donation button** — a `DONATE` control in the menu bar opens the same
  PayPal page used by the other Replicant projects.
- **Project format v3** — stores per-track plugin assignments and audio-clip
  envelopes. v1/v2 projects still load with neutral defaults.

## Features

- **Two-thread audio architecture**
  - Audio thread: the `cpal` output callback renders stereo at 48 kHz, 32-bit
    float when the device supports it.
  - UI thread: `eframe` + `egui` with the `wgpu` renderer.
  - Lock-free SPSC `rtrb` rings for commands, telemetry, scope samples, MIDI,
    recording events and plugin audio blocks.
- **Synthesis**
  - 8 tracks with a 32-voice polyphonic two-operator FM engine.
  - Per-track editable FM ratio, modulation index and ADSR envelope.
  - Per-track gain, pan, mute and solo; equal-power stereo bus.
  - Generative 16-step clip patterns across 8 scenes in a 16-bar loop.
- **Modular master chain**
  - `SYNAPSE DRIVE`: saturation, wet mix and output trim.
  - `NEURAL GEN V5`: high-cut tone filter.
  - `QUANTUM RESONATOR`: resonant topology-preserving state-variable low-pass.
  - `MIRROR WIDTH`: mid/side stereo width.
  - `ECHO MEMORY`: stereo delay with feedback and wet mix.
  - Add, reorder, bypass and close modules; master gain, soft clip, DC blocking
    and a hard output guard stay fixed at the end of the chain.
- **MIDI recording**
  - Live MIDI input through `midir` (ALSA, CoreMIDI, WinMM), with rescan and
    device selection.
  - Arm a track, hit `REC`, and play: notes are captured at 16th-note
    resolution into a clip that replaces the generative pattern for that track.
  - An on-screen two-octave keyboard feeds the same MIDI path, so the workflow
    is testable without hardware.
  - Clips are visible in the session matrix and the arrangement, and can be
    muted without deleting them.
- **Piano roll and audio clip editing**
  - Double-click a session or arrangement clip to program MIDI on its track.
  - 16-bar grid at 16th-note resolution, four-octave view with octave shift.
  - Add, move, resize, delete and set velocity per note.
  - Audio clips expose clip gain and loop-normalized fade-in/fade-out.
- **Project save / load**
  - Versioned JSON `.rproj` files: tempo, mix, scenes, synth parameters, effect
    chain, master gain, recorded MIDI and plugin state (base64).
  - `Ctrl+S` quick save, a project browser, and `--project <path>` on startup.
  - Format v1 files still load; the legacy fixed FX block becomes a chain.
  - `examples/Demo.rproj` ships with MIDI clips and a five-module chain.
- **Plugin hosting**
  - **CLAP**: in-process hosting with audio, note/parameter events and state.
  - **VST3**: in-process hosting with audio, MIDI, parameters and state.
  - **VST2**: discovered and listed, but not hostable because the Steinberg SDK
    is proprietary and not vendored (see [docs/PLUGINS.md](docs/PLUGINS.md)).
  - **Audio Unit**: not implemented (macOS only).
  - One insert slot per track (8 total), bypass, per-slot parameter knobs and
    latency reporting. Inserts run on the track bus before the master chain.
- **Session UI**
  - Menu bar with project, plugin, synth, MIDI and zoom controls.
  - Transport with bar/beat/step counter, editable BPM, play/stop, REC, audio
    callback CPU meter, master meter and link status.
  - Clip matrix (8 × 8) plus a MIDI clip row, with live step progress and
    scene launch.
  - Mixer strips with vertical faders, LED dB meters, pan and mute/solo.
  - Arrangement lanes with a 16-bar ruler, animated playhead and MIDI clip
    containers.
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
with the engine through lock-free rings. Each block carries one stereo bus per
track, so an insert processes only the track it is assigned to. This keeps
`!Send` plugin handles out of the audio callback and isolates load/state cost
from the UI.

**VST3 unload workaround:** some third-party VST3s (verified with OB-Xd) crash
asynchronously inside their own teardown. Closing such a slot stops its DSP and
parks the instance for the lifetime of the process instead of destroying it, so
the application stays stable. The trade-off is that the parked plugin's memory
is only reclaimed when the app exits; CLAP instances are destroyed normally.

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
cargo run --release -- --selftest       # DSP, chain, project, MIDI and clock smoke test
cargo run --release -- --project examples/Demo.rproj
cargo test                              # 72 unit tests
cargo clippy --all-targets -- -D warnings
```

Environment variables:

```bash
REPLICANT_DAW_NO_AUDIO=1        # force the silent fallback clock
REPLICANT_DAW_SIZE=1200x760     # initial window size (clamped to the minimum)
REPLICANT_DAW_ZOOM=125          # UI scale at startup (75..150)
REPLICANT_DAW_PROJECT=x.rproj   # load a project at startup
REPLICANT_DAW_PROJECTS=dir      # override the projects directory
REPLICANT_DAW_PLUGIN_PATH=a:b   # extra plugin scan roots
REPLICANT_DAW_OPEN_PLUGIN=/path/to/plugin.vst3   # QA: load into slot 1
REPLICANT_DAW_SMOKE=docs/screenshot.png
REPLICANT_DAW_SMOKE_SECS=4      # auto-play, wait, screenshot, exit
REPLICANT_DAW_SMOKE_WINDOWS=rack,midi,synth      # open utility windows
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
- **Ctrl + / Ctrl − / Ctrl 0** — zoom in / out / reset. The `ZOOM` menu offers
  75%, 90%, 100%, 110%, 125% and 150%.
- **Play / Stop / REC** — transport and MIDI recording on the selected track.
- **BPM** — click to type, drag vertically, or use `-` / `+`.
- **MIDI row** — click a track's clip to mute/unmute it; empty slots prompt to
  arm REC. The row also shows a live loop position.
- **Clip slot** — click to launch, click again (or right-click) to stop. The
  triangle at the left of a row launches the whole scene.
- **Track header** — selects the track (also the MIDI input/record target and
  the track edited by the synth module).
- **Fader** — drag vertically, double-click for 0 dB.
- **Pan knob** — drag vertically, double-click to centre.
- **M / S** — mute and solo.
- **SYNTH module** — click its track strip to cycle tracks; drag its seven
  knobs. `SYNTH` in the menu opens the larger editor with RESET.
- **Effect module** — drag knobs; `b` bypasses, `X` closes, `‹ ›` reorders.
  `+ ADD FX` opens the module menu (and `RESET CHAIN`).
- **Rack** — load, bypass or remove plugins; up to 16 parameters per slot.
- **MIDI** — connect an input, play the on-screen keyboard, arm REC, clear a
  clip, toggle MIDI to plugins.

## Architecture

```
┌────────────────────────── UI thread (eframe / egui / wgpu) ─────────────────────────┐
│ menu+zoom · transport · clip matrix+MIDI row · mixer · lanes · FFT · chain · windows  │
│    │ Command              ▲ Telemetry / scope / record events                        │
└────┼──────────────────────┼─────────────────────────────────────────────────────────┘
     │ rtrb                 │ rtrb
┌────▼──────────────────────┴─────────────────────────────────────────────────────────┐
│ Audio thread: cpal callback (or paced silent clock)                                  │
│ sequencer → FM voices (per-track params) → track mix → modular FX chain → master      │
│      │ dry blocks                                                ▲ processed blocks  │
└──────┼───────────────────────────────────────────────────────────┼───────────────────┘
       │ rtrb                                                      │ rtrb
┌──────▼───────────────────────────────────────────────────────────┴───────────────────┐
│ Plugin worker thread: CLAP + VST3 instances, params, state                           │
└──────────────────────────────────────────────────────────────────────────────────────┘

MIDI hardware ──midir──▶ bridge ring ──UI pump──▶ MIDI ring ──▶ engine (voices + record)
```

- `src/audio/engine.rs` renders audio. It never allocates, locks or calls into
  third-party code; the plugin chain only moves blocks between rings.
- `src/audio/effects.rs` implements the chain modules with fixed state and
  smoothed parameters.
- `src/plugins/worker.rs` owns all plugin handles. It is created inside its own
  thread because plugin instances are not `Send`.
- `src/studio.rs` owns MIDI, plugins, zoom and projects on the UI side.
- `src/project.rs` is the serialized session model.

## DSP details

- Two-operator phase-modulation voice: `sin(phase + index * env * sin(mod_phase))`,
  with per-track ratio, index and ADSR edited live from the UI.
- The sequencer derives the current 16th note from a sample-accurate transport
  position, so tempo changes and loops stay tight.
- Chain order is user-defined; master gain, soft clip, DC blocker and output
  guard always come last, followed by the plugin inserts.
- Effect parameters are clamped to their module ranges and smoothed to avoid
  zipper noise. Delay runs a 1.05 s stereo line.
- `src/dsp/spectrum.rs` uses `rustfft` with a 1024-point Hann window and
  log-spaced display bins from 30 Hz to 16 kHz.

## Project format

`.rproj` is versioned JSON (format 3):

```json
{
  "format_version": 3,
  "app_version": "1.0.3",
  "name": "Neon Demo",
  "bpm": 142.0,
  "effects": [
    { "kind": "DRIVE",  "params": [0.22, 1.0, 1.0], "bypass": false },
    { "kind": "FILTER", "params": [6200.0, 0.24, 1.0], "bypass": false },
    { "kind": "DELAY",  "params": [0.28, 0.32, 0.18], "bypass": false }
  ],
  "master_gain": 0.9,
  "tracks": [
    { "name": "SYNAPSE 01", "gain_db": -7.0, "pan": -0.12,
      "mute": false, "solo": false, "scene": 1,
      "clip_enabled": true,
      "clip_gain_db": 0.0, "fade_in": 0.0, "fade_out": 0.0,
      "synth": { "ratio": 1.0, "index": 2.4, "attack": 0.004,
                 "decay": 0.18, "sustain": 0.25, "release": 0.25,
                 "level": 0.9 },
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

`plugins[].slot` is the track index. Unknown fields are ignored (forward
compatible), while a newer `format_version` is rejected with a clear error.
Loading a v1 file translates its `fx` block into chain modules; v2 plugin slots
map to the same track numbers.

## Verification

Verified on Linux (Arch, kernel 7.2) with a real AMD/Radeon desktop, a
Keystation 49 MK3 MIDI keyboard, and third-party plugins already installed:

```
$ cargo test
test result: ok. 78 passed; 0 failed

$ cargo clippy --all-targets -- -D warnings
Finished

$ ./target/release/replicant-daw --selftest
- rendered 8.0 s of audio in 0.106 s (75.7x realtime)
- lock-free path: telemetry playing=true peaks=0.442 | scope frames=32768
- chain: 4 default modules -> 4 after remove/add; synth index 3.2
- project: 2 module(s), 1 note(s), synth index 4.25
- midi recording: 1 note(s), 2 UI event(s)
- midi inputs available: 5
- midi input opened: Midi Through:Midi Through Port-0 14:0
- silent clock: peak 0.577, playing=true
SELFTEST OK

$ ./target/release/replicant-daw --audio-check
AUDIO OK: Alsa | Default Audio Device | 48000 Hz | ... telemetry frames | stopped (silent)
```

Additional manual checks:

- **VST3**: `OB-Xd 3.vst3` loads on the worker thread, exposes its parameters,
  reports latency and produces audio from MIDI; closing the slot no longer
  crashes thanks to the parked-instance workaround.
- **CLAP**: `Dexed.clap` loads, exposes parameters and processes audio.
- **Projects**: saved a session with chain, synth parameters and MIDI clips,
  then loaded it in a fresh process; `examples/Demo.rproj` is the same file.
- **UI zoom**: captured at 125% (`docs/synth.png`), with the logical layout
  preserved and the synth editor open.
- **Screenshots**: `docs/screenshot.png`, `docs/synth.png`, `docs/plugins.png`
  and `docs/piano-roll.png` are captured from the real binary under Xvfb.

The CI matrix runs tests and the self-test on Ubuntu, macOS and Windows. The
release workflows cross-build both macOS architectures and publish packages for
Linux, macOS and Windows on every tag (`.github/workflows/release-*.yml`).

## Limitations / next steps

This is a prototype, not a production DAW:

- Plugin hosting is in-process: a crashing plugin takes the app down, and the
  plugin worker adds one block of latency. An out-of-process sandbox is the
  natural next step.
- The VST3 workaround parks closed instances instead of destroying them, so
  their memory is held until exit.
- No plugin GUI embedding; parameters are edited with the rack knobs.
- VST2 and Audio Unit hosting are not implemented (see the matrix above).
- The modular effects are still a master chain; per-track inserts host one
  plugin each. There is no arbitrary per-track effect chain yet.
- MIDI clips are stored per track (one clip per track), not per scene slot.
- No MIDI export, automation lanes or undo. Recording replaces the target clip,
  no overdub.
- One 16-bar loop.
- Windows and macOS are covered by CI builds but were not manually auditioned
  for this release; Linux/ALSA, MIDI input, CLAP and VST3 were verified on
  hardware.

## Support

Replicant DAW is free and open source. If it earns a place in your setup and
you feel like saying thanks, you can buy me a coffee — completely optional.

<p align="center">
  <a href="https://paypal.me/DanimanX?locale.x=en_US&amp;country.x=CL">
    <img src="https://img.shields.io/badge/Donate-PayPal-00457C?logo=paypal&amp;logoColor=white&amp;style=for-the-badge" alt="Donate with PayPal">
  </a>
</p>

<p align="center">
  <sub><a href="https://paypal.me/DanimanX?locale.x=en_US&amp;country.x=CL">paypal.me/DanimanX</a></sub>
</p>

---

## License

MIT. See [LICENSE](LICENSE).

The embedded fonts are licensed under the SIL Open Font License; the license
texts are in [`assets/licenses/`](assets/licenses).
