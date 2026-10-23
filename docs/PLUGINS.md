# Plugin support

Replicant DAW hosts **CLAP** and **VST3** plugins in process, on a dedicated
worker thread. **VST2** plugins are discovered and listed but cannot be loaded,
and **Audio Unit** is not implemented.

## Format matrix

- **CLAP** — load, activate, stereo audio, note events, parameter events,
  parameter listing, state save/restore.
- **VST3** — load, activate, stereo audio, MIDI, parameter listing and control,
  state save/restore, latency reporting.
- **VST2** — scan only. The format's SDK is proprietary and cannot be vendored,
  so there is no host implementation. The browser marks these plugins as
  `not hostable` and tells you to use the VST3 or CLAP build.
- **AU / AUv3** — not implemented. macOS users should prefer CLAP or VST3.

## Where plugins are found

The scanner never loads a library, so a broken plugin cannot crash the browser.
It walks the standard locations:

Linux:

```
~/.clap            /usr/lib/clap        /usr/local/lib/clap
~/.vst3            /usr/lib/vst3        /usr/local/lib/vst3
~/.vst
```

macOS:

```
~/Library/Audio/Plug-Ins/CLAP      /Library/Audio/Plug-Ins/CLAP
~/Library/Audio/Plug-Ins/VST3      /Library/Audio/Plug-Ins/VST3
~/Library/Audio/Plug-Ins/VST       /Library/Audio/Plug-Ins/VST
```

Windows:

```
%LOCALAPPDATA%\Programs\Common\CLAP
%COMMONPROGRAMFILES%\CLAP
%LOCALAPPDATA%\Programs\Common\VST3
%COMMONPROGRAMFILES%\VST3
%COMMONPROGRAMFILES%\VST2
%PROGRAMFILES%\VSTPlugins
```

Add extra roots with a path list:

```bash
REPLICANT_DAW_PLUGIN_PATH="/data/plugins/clap:/data/plugins/vst3" cargo run --release
```

## Using the rack

1. Plugins are scanned automatically at startup; **RESCAN** in the picker
   refreshes the list.
2. A new track starts empty. Click `SELECT VST3 / CLAP` in the device chain
   to open the instrument browser, which lists every CLAP/VST3 plugin installed
   on the system. Picking one assigns it to the track; this build has no
   built-in synth.
3. **RACK** lists all eight track inserts. Each row has **EDIT**, **LOAD**,
   **BYPASS** and **X** (unload), reports its latency, and exposes its
   parameters as knobs.
4. **EDIT** (or the `OPEN EDITOR` button in the instrument module) opens the
   plugin editor: a parameter surface that can fill the whole window with
   **FULLSCREEN** (F11 toggles it, Esc restores it) and filters parameters by
   name.
5. **GUI** (instrument module, rack row, or `PLUGIN GUI` in the editor) opens
   the plugin's own window — its native interface, where the vendor's controls
   are the ones you see.
6. State is saved with the project (`Ctrl+S`). VST3 and CLAP state blobs are
   stored base64-encoded in the `.rproj` file.

## Native plugin windows

The DAW hosts the plugin's real editor, not a reimplementation:

- **CLAP** — the host creates an X11 toplevel, hands its id to the plugin's
  `clap.gui` extension (`create` → `set_parent` → `show`) and answers
  `clap_host_gui` (resize/show/hide/closed), `clap_host_thread-check`,
  `clap_host_timer_support` and `clap_host_posix_fd_support`. Plugins that only
  support floating windows are created floating instead.
- **VST3** — the editor is opened through the VST3 host's own window; the worker
  pumps `service_platform_events` and `service_run_loop` (the Linux `IRunLoop`
  that plugins register their X11 socket and timers with).
- **Servicing** — both paths are serviced every ~4 ms by the plugin worker,
  independently of the audio stream, so the editor stays interactive while
  playing.
- **Floating windows** — a plugin's own floating editor is preferred when it
  offers one. Otherwise the host window is created as a `_NET_WM_WINDOW_TYPE_UTILITY`
  with `_NET_WM_STATE_ABOVE`, `WM_TRANSIENT_FOR` pointing at the DAW window and
  `WM_NORMAL_HINTS` matching the editor, so it floats above the DAW, can be
  moved on its own and never clips the plugin's canvas.
- **Sizing** — the host window is created at the plugin's reported size, then
  kept in sync: plugin-driven resizes are applied to the window, user resizes
  are pushed to the plugin when it can resize, and reversed when it cannot.

Known limits:

- Linux/X11 is the tested path. On Windows and macOS the CLAP path reports
  that native windows are only wired for X11; the parameter editor still works.
- The editor sequence follows the CLAP specification strictly: `create`, then
  embedding, then `get_size`, then `show`. Calling `size` before `create` made
  u-he plugins abort the process; a regression test now guards the order.
- No window embedding into the DAW's own canvas: each plugin editor is its own
  OS window.

Instrument vs effect:

The track has one external plugin slot. There is no built-in synth and no
source switch, so a track cannot play two generators at once.

- A loaded **instrument** owns the track and the panel shows its parameters.
  Unload it with `X` to free the slot.
- A loaded **effect** is a track insert. The module header reads
  `NAME · FX INSERT`.
- Classification uses the CLAP feature tags (`instrument`, `synthesizer`,
  `sampler` vs `audio-effect`) and the VST3 bus layout plus category, so a
  sampler that also exposes an audio input is not mistaken for an effect.

Routing rules:

- Each plugin processes only its track's stereo bus, before the master effect
  chain.
- **Effects** replace the track signal (100% wet insert).
- **Instruments** are the track's only generator, so loading another plugin
  into the same slot replaces the previous instance.
- **MIDI → PLUGINS** in the MIDI window forwards live notes, CC and pitch bend
  to the insert on the selected input track. It is on by default.
- The sequencer and the piano roll also send note on/off to the instrument
  insert, so hosted instruments play session clips and recorded MIDI.

## Unloading and teardown

CLAP instances are destroyed normally when a slot is closed. Some third-party
VST3s crash asynchronously inside their own destructor (verified with OB-Xd:
the process segfaults shortly after `Plugin::drop`). To keep the DAW stable,
closing a VST3 slot:

1. stops the plugin's DSP, and
2. parks the instance in the plugin worker for the lifetime of the process
   instead of destroying it.

The slot disappears from the audio path immediately, but its memory is only
reclaimed on exit. This is a pragmatic crash guard, not a fix; process
isolation (`vst3_host::simple::load_plugin_isolated`, which needs the
`vst3-host-helper` binary shipped alongside the app) is the proper solution and
is planned.

## Real-time model and limitations

- Plugin handles are not `Send`, so they live on a worker thread created for
  them. The engine exchanges 512-frame stereo blocks through `rtrb` rings.
- This adds roughly one block of latency (about 10 ms at 48 kHz) when a plugin
  is active. The rack reports `plugin latency + block latency`.
- Plugin DSP is third-party code. A crash or a real-time violation is not
  contained yet; process isolation is a planned follow-up.
- GUI embedding is not implemented: parameters are edited from the rack.
- Only the first stereo channel pair is used. Surround and sidechain buses are
  ignored.
- Scanning reads filenames only; the real plugin name and parameter list appear
  after loading.

## Troubleshooting

**No plugins found.** Confirm the directories above exist, or set
`REPLICANT_DAW_PLUGIN_PATH`. Press **RESCAN** after installing plugins.

**A plugin fails to load.** The rack shows an error and the engine continues
with the dry signal. Common causes: missing runtime libraries, a plugin built
for another architecture, or a CLAP plugin that requires host extensions this
DAW does not implement (for example a GUI or remote-control extension).

**A plugin is silent.** Instruments only produce sound when they receive MIDI:
connect a keyboard or use the on-screen keys, and verify **MIDI → PLUGINS** is
enabled. Effects require audio on the master bus, so press play.

**Crackles with plugins.** In-process plugin DSP plus a one-block bridge can
exceed the audio budget on slow machines. Bypass slots or raise the device
buffer size.
