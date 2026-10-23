# Replicant DAW

A cyberpunk digital audio workstation in Rust: external CLAP/VST3 instruments,
an 8 × 8 session matrix, a 16-bar arrangement with draggable audio clips, a
modular master chain, MIDI recording and versioned `.rproj` projects.

Built with `eframe`/`egui` on `wgpu`; audio is rendered by `cpal` through
lock-free `rtrb` rings.

**Version 1.4.0** · MIT · Linux, macOS and Windows.

![Replicant DAW session and arrangement](docs/screenshot.png)

> Captured headlessly from the real binary with the silent fallback clock, so
> the status reads `SILENT FALLBACK`. A fresh session starts with an empty
> instrument slot and the plugins already scanned.

## What's new in 1.4.0

- **The built-in NEURAL GEN synth is gone; tracks host external plugins only.**
  Removing the internal generator also removes the path that could leave it
  sounding under a VST/CLAP instance. New tracks are silent until a CLAP or
  VST3 instrument is loaded, and selecting another plugin replaces the previous
  instance in the same slot.
- **Arrangement audio clips.** Drag a `.wav`, `.flac` or `.mp3` file from the
  desktop onto an arrangement lane and it becomes an audio clip at that bar.
  Clips play at their timeline position, through the track fader, effects and
  master chain.
- **Drag between arrangement and session.** Drag a session sample cell onto a
  lane to move it to the timeline; drag an arrangement clip onto any session
  cell to move it back. A translucent `DROP AUDIO` block previews the target.
- **Project format v9** stores arrangement clips and drops the retired synth
  patch. Older projects still load; their `synth` instrument id becomes an
  empty slot.
- **Documentation regenerated** for the synth-free device chain and the new
  arrangement workflow.

## Screenshots

![Arrangement lanes with audio clips](docs/arrangement.png)

![Instrument browser listing external CLAP/VST3 plugins](docs/instrument-browser.png)

![Per-track plugin rack hosting a real CLAP instrument](docs/plugins.png)

![Audio clip editor with its loop region](docs/sample-clip.png)

![Piano roll clip editor](docs/piano-roll.png)

## Features

- **One external instrument slot per track** — CLAP and VST3 are hosted
  in-process on a worker thread; VST2 is listed but not hostable.
- **Session matrix** — 8 tracks × 8 scenes, independent MIDI clips, audio
  samples, per-clip loops and mute.
- **Arrangement** — 16-bar timeline with audio clips, a beat grid and an
  animated playhead. Drag audio in from the desktop or from the session view.
- **Audio clip editor** — waveform, trim, gain, normalize, reverse, fades,
  loop region and one-shot preview.
- **Piano roll** — add, move, resize and delete notes; per-clip loop region;
  velocity editing.
- **Mixer and master chain** — gain, pan, mute/solo, drive, tone, filter,
  width and delay modules, master gain and output protection.
- **Plugin hosting** — parameter surface, full-screen editor, native plugin
  windows, state save/restore, latency reporting.
- **Projects** — versioned JSON, quick save and a project browser.
- **Headless fallback** — the silent clock keeps meters, playhead and plugins
  running when no audio device is available.

## Requirements

- Rust 1.85 or newer.
- Linux: ALSA development headers and the usual windowing libraries.

  ```bash
  # Debian/Ubuntu
  sudo apt install build-essential pkg-config libasound2-dev \
      libxkbcommon-dev libwayland-dev libx11-dev libxi-dev

  # Fedora
  sudo dnf install alsa-lib-devel libxkbcommon-devel wayland-devel \
      libX11-devel libXi-devel
  ```

- macOS: Xcode command line tools.
- Windows: MSVC build tools; WASAPI is used through `cpal`.

Fonts and the app icon are embedded.

## Run

```bash
cargo run --release
```

```bash
cargo run --release -- --devices        # list audio output devices
cargo run --release -- --audio-check    # open the default device and verify telemetry
cargo run --release -- --selftest       # engine, chain, project, MIDI and clock smoke test
cargo run --release -- --project /path/to/project.rproj
cargo test
cargo clippy --all-targets -- -D warnings
```

Environment variables:

```bash
REPLICANT_DAW_NO_AUDIO=1          # force the silent fallback clock
REPLICANT_DAW_SIZE=1660x1000      # initial window size
REPLICANT_DAW_ZOOM=125            # UI scale at startup (75..200)
REPLICANT_DAW_PROJECT=x.rproj     # load a project at startup
REPLICANT_DAW_PROJECTS=dir        # override the projects directory
REPLICANT_DAW_PLUGIN_PATH=a:b     # extra plugin scan roots
REPLICANT_DAW_OPEN_PLUGIN=/path/to/plugin.vst3
WGPU_BACKEND=gl|vulkan|metal|dx12
```

Default project directory:

- Linux: `~/.local/share/replicant-daw/projects`
- macOS: `~/Library/Application Support/ReplicantDaw/projects`
- Windows: `%APPDATA%\ReplicantDaw\projects`

## Arrangement workflow

1. Open the instrument browser from `SELECT VST3 / CLAP` and load an instrument
   into a track. Without one, MIDI clips are silent.
2. Drop an audio file onto a lane, or drag a sample from the session matrix.
   The clip lands on the nearest beat; the ghost block shows where it will go.
3. Drag the arrangement block onto any session cell to move it back. Dragging
   a session cell onto a lane moves it out of the matrix so it cannot sound
   twice.
4. Double-click an arrangement clip to open the audio editor. Right-click for
   `OPEN AUDIO EDITOR` or `REMOVE FROM ARRANGEMENT`.
5. Press Space to play. Arrangement clips play at their absolute timeline
   position; the 16-bar loop wraps.

## Controls

- **Space** — play / stop. **Esc** — stop and rewind.
- **Ctrl+S** — save. **Ctrl+O** — project window. **Ctrl+P** — plugin browser.
- **Ctrl + / Ctrl − / Ctrl 0** — zoom in / out / reset.
- **BPM** — click to type, drag vertically, or use `-` / `+`.
- **Clip slot** — click to launch, click again to stop; right-click for the
  clip menu.
- **Sample cell** — drag to the arrangement; double-click to edit the clip.
- **Arrangement lane** — drop audio files or dragged clips; double-click empty
  space to edit the track's clip.
- **Fader / pan** — drag vertically, double-click to reset.
- **M / S** — mute and solo.
- **Effect module** — drag knobs; `b` bypasses, `X` closes, `‹ ›` reorders.
- **Rack** — load, bypass, edit or remove the plugin in each track.

## Plugin support

- **CLAP** — scan, load, stereo audio, note/parameter events, state, native
  editor.
- **VST3** — scan, load, audio, MIDI, parameters, state, latency, editor.
- **VST2** — discovered and listed only; the Steinberg SDK is not vendored.
- **Audio Unit** — not implemented.

Each track has one insert slot. Instruments generate the track's audio; effect
plugins process the track bus. Plugins run on a dedicated worker thread and
exchange 512-frame stereo blocks through lock-free rings.

Details, scan paths and known limits: [docs/PLUGINS.md](docs/PLUGINS.md).

## Project format

`.rproj` is versioned JSON. Format v9 stores tempo, mix, track names, session
MIDI clips, session samples, arrangement audio clips, effect chain, master gain
and plugin state. Older formats load with migrations; a track whose instrument
id is `synth` loads as an empty slot because the built-in synth was removed.
Unknown fields are ignored and newer format versions are rejected with a clear
error.

## Limits

- One MIDI clip per session slot and one external instrument per track.
- Arrangement clips are audio; there is no MIDI arrangement clip model yet.
- One 16-bar loop; no automation lanes, undo or audio export.
- Plugin hosting is in-process: a crashing plugin can take the app down.
- Native plugin windows are tested on Linux/X11; other platforms fall back to
  the parameter editor.
- Windows and macOS are covered by CI builds but were not manually auditioned
  for this release.

## License

MIT. See [LICENSE](LICENSE).

The embedded fonts are licensed under the SIL Open Font License; license texts
are in [`assets/licenses/`](assets/licenses).

If this project earns a place in your setup, you can buy me a coffee:
[paypal.me/DanimanX](https://paypal.me/DanimanX?locale.x=en_US&country.x=CL).
