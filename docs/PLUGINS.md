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

1. Open **PLUGINS** in the menu bar and press **RESCAN**.
2. Pick a plugin and click **LOAD**; it goes to the slot chosen in the rack
   (default 1). **RACK** opens the rack.
3. Up to four slots run in series on the master bus. Each slot has **BYPASS**
   and **X** (unload), reports its latency, and exposes up to 16 parameters as
   knobs.
4. State is saved with the project (`Ctrl+S`). VST3 and CLAP state blobs are
   stored base64-encoded in the `.rproj` file.

Routing rules:

- **Effects** replace the dry signal (100% wet insert).
- **Instruments** (VST3 category containing `Instrument`/`Synth`, or CLAP
  plugins with no audio input) are summed with the dry mix, so the internal
  synth is not lost when you add a synth plugin.
- **MIDI → PLUGINS** in the MIDI window forwards live notes, CC and pitch bend
  to plugins. It is on by default.

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
prototype does not implement (for example a GUI or remote-control extension).

**A plugin is silent.** Instruments only produce sound when they receive MIDI:
connect a keyboard or use the on-screen keys, and verify **MIDI → PLUGINS** is
enabled. Effects require audio on the master bus, so press play.

**Crackles with plugins.** In-process plugin DSP plus a one-block bridge can
exceed the audio budget on slow machines. Bypass slots or raise the device
buffer size.
