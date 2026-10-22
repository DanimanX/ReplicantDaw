//! In-process VST3 hosting.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use vst3_host::audio::AudioBuffers;
use vst3_host::midi::{MidiChannel, MidiEvent};
use vst3_host::window::PluginWindow;
use vst3_host::Plugin;

use super::{BlockEvent, ParamInfo, PLUGIN_BLOCK_FRAMES};

pub struct Vst3Instance {
    /// Shared with the editor window, because `PluginWindow` takes an
    /// `Arc<Mutex<Plugin>>`. Both live on the plugin worker thread, so the
    /// lock is uncontended.
    plugin: Arc<Mutex<Plugin>>,
    window: Option<PluginWindow>,
    buffers: AudioBuffers,
    path: PathBuf,
    name: String,
    vendor: String,
    version: String,
    category: String,
    params: Vec<ParamInfo>,
}

impl Vst3Instance {
    pub fn load(path: &Path, sample_rate: f32, state: Option<&[u8]>) -> Result<Self, String> {
        let plugin = vst3_host::simple::load_plugin_with_settings(
            path,
            sample_rate as f64,
            PLUGIN_BLOCK_FRAMES,
        )
        .map_err(|error| format!("VST3 load failed: {error}"))?;
        let plugin = Arc::new(Mutex::new(plugin));

        let info = lock(&plugin).info().clone();
        let buffers = AudioBuffers::new(2, 2, PLUGIN_BLOCK_FRAMES, sample_rate as f64);
        let mut instance = Self {
            plugin,
            window: None,
            buffers,
            path: path.to_path_buf(),
            name: info.name,
            vendor: info.vendor,
            version: info.version,
            category: info.category,
            params: Vec::new(),
        };

        if let Some(state) = state {
            lock(&instance.plugin)
                .load_state(state)
                .map_err(|error| format!("VST3 state restore failed: {error}"))?;
        }

        instance.params = {
            let plugin = lock(&instance.plugin);
            plugin
                .get_parameters()
                .map(|params| {
                    params
                        .iter()
                        .map(|param| ParamInfo {
                            id: param.id,
                            name: param.name.clone(),
                            value: param.value as f32,
                            text: plugin
                                .format_parameter(param.id, param.value)
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        };

        lock(&instance.plugin)
            .start_processing()
            .map_err(|error| format!("VST3 could not start processing: {error}"))?;

        Ok(instance)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn vendor(&self) -> &str {
        &self.vendor
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn category(&self) -> &str {
        &self.category
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn params(&self) -> &[ParamInfo] {
        &self.params
    }

    pub fn latency(&self) -> u32 {
        lock(&self.plugin).latency_samples()
    }

    pub fn is_instrument(&self) -> bool {
        self.category.to_lowercase().contains("instrument")
            || self.category.to_lowercase().contains("synth")
    }

    /// Re-read every parameter from the plugin.
    pub fn refresh_params(&mut self) {
        let plugin = lock(&self.plugin);
        if let Ok(params) = plugin.get_parameters() {
            self.params = params
                .iter()
                .map(|param| ParamInfo {
                    id: param.id,
                    name: param.name.clone(),
                    value: param.value as f32,
                    text: plugin
                        .format_parameter(param.id, param.value)
                        .unwrap_or_default(),
                })
                .collect();
        }
    }

    pub fn set_param(&mut self, id: u32, value: f32) {
        let normalized = value.clamp(0.0, 1.0) as f64;
        let mut plugin = lock(&self.plugin);
        if plugin.set_parameter(id, normalized).is_ok() {
            let text = plugin.format_parameter(id, normalized).unwrap_or_default();
            drop(plugin);
            if let Some(param) = self.params.iter_mut().find(|p| p.id == id) {
                param.value = normalized as f32;
                param.text = text;
            }
        }
    }

    /// Does this component expose an editor?
    pub fn has_editor(&self) -> bool {
        lock(&self.plugin).has_editor()
    }

    pub fn is_gui_open(&self) -> bool {
        self.window.as_ref().is_some_and(PluginWindow::is_open)
    }

    /// Open the plugin's own editor window (its native GUI).
    pub fn open_gui(&mut self) -> Result<(), String> {
        if self.is_gui_open() {
            return Ok(());
        }
        self.close_gui();
        let mut window = PluginWindow::new(Arc::clone(&self.plugin));
        window
            .open()
            .map_err(|error| format!("could not open the plugin editor: {error}"))?;
        self.window = Some(window);
        Ok(())
    }

    pub fn close_gui(&mut self) {
        if let Some(mut window) = self.window.take() {
            window.close();
        }
    }

    /// Pump the editor: platform events, the plugin's Linux run loop and any
    /// resize request it made.
    ///
    /// VST3 editors on Linux register their X11 socket and timers through
    /// `IPlugFrame`/`IRunLoop`; the host has to call `service_run_loop`
    /// regularly or the editor draws once and then ignores every click.
    pub fn service_gui(&mut self) {
        let open = match self.window.as_mut() {
            Some(window) if window.is_open() => {
                let _ = window.service_platform_events();
                true
            }
            Some(_) => false,
            None => false,
        };
        if !open {
            if self.window.is_some() {
                self.close_gui();
            }
            return;
        }
        if let Ok(mut plugin) = self.plugin.try_lock() {
            plugin.service_run_loop();
        }
    }

    /// Stop DSP without destroying the component.
    ///
    /// Used when a slot is closed: some third-party VST3s (verified with
    /// OB-Xd) crash asynchronously inside their own teardown, so the worker
    /// parks the instance instead of dropping it.
    pub fn stop(&mut self) {
        self.close_gui();
        let _ = lock(&self.plugin).stop_processing();
    }

    pub fn save_state(&mut self) -> Result<Vec<u8>, String> {
        lock(&self.plugin)
            .save_state()
            .map_err(|error| format!("VST3 state save failed: {error}"))
    }

    pub fn process(
        &mut self,
        frames: usize,
        left: &mut [f32],
        right: &mut [f32],
        events: &[BlockEvent],
    ) -> Result<(), String> {
        for event in events {
            let bytes = &event.bytes[..event.len as usize];
            if bytes.len() < 3 {
                continue;
            }
            let channel =
                MidiChannel::from_index((bytes[0] & 0x0f) + 1).unwrap_or(MidiChannel::Ch1);
            let midi = match bytes[0] & 0xf0 {
                0x90 => MidiEvent::NoteOn {
                    channel,
                    note: bytes[1],
                    velocity: bytes[2],
                },
                0x80 => MidiEvent::NoteOff {
                    channel,
                    note: bytes[1],
                    velocity: bytes[2],
                },
                0xb0 => MidiEvent::ControlChange {
                    channel,
                    controller: bytes[1],
                    value: bytes[2],
                },
                0xe0 => MidiEvent::PitchBend {
                    channel,
                    value: ((bytes[2] as u16) << 7) | bytes[1] as u16,
                },
                _ => continue,
            };
            let _ = lock(&self.plugin).send_live_midi_event(midi);
        }

        self.buffers.block_size = frames;
        self.buffers.inputs[0][..frames].copy_from_slice(&left[..frames]);
        self.buffers.inputs[1][..frames].copy_from_slice(&right[..frames]);
        self.buffers.outputs[0][..frames].fill(0.0);
        self.buffers.outputs[1][..frames].fill(0.0);

        lock(&self.plugin)
            .process_audio(&mut self.buffers)
            .map_err(|error| format!("VST3 process failed: {error}"))?;

        left[..frames].copy_from_slice(&self.buffers.outputs[0][..frames]);
        right[..frames].copy_from_slice(&self.buffers.outputs[1][..frames]);
        Ok(())
    }
}

/// Lock the plugin, recovering from a poisoned mutex.
fn lock(plugin: &Arc<Mutex<Plugin>>) -> std::sync::MutexGuard<'_, Plugin> {
    plugin
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// Teardown is handled inside `vst3_host::Plugin::drop`, which stops
// processing, deactivates the component and terminates it in the order the
// VST3 SDK requires. Calling `stop_processing` here as well crashes some
// plugins (verified with OB-Xd) on unload.
