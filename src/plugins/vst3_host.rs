//! In-process VST3 hosting.

use std::path::{Path, PathBuf};

use vst3_host::audio::AudioBuffers;
use vst3_host::midi::{MidiChannel, MidiEvent};

use super::{BlockEvent, ParamInfo, PLUGIN_BLOCK_FRAMES};

pub struct Vst3Instance {
    plugin: vst3_host::Plugin,
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

        let info = plugin.info().clone();
        let buffers = AudioBuffers::new(2, 2, PLUGIN_BLOCK_FRAMES, sample_rate as f64);
        let mut instance = Self {
            plugin,
            buffers,
            path: path.to_path_buf(),
            name: info.name,
            vendor: info.vendor,
            version: info.version,
            category: info.category,
            params: Vec::new(),
        };

        if let Some(state) = state {
            instance
                .plugin
                .load_state(state)
                .map_err(|error| format!("VST3 state restore failed: {error}"))?;
        }

        instance.params = instance
            .plugin
            .get_parameters()
            .map(|params| {
                params
                    .iter()
                    .map(|param| ParamInfo {
                        id: param.id,
                        name: param.name.clone(),
                        value: param.value as f32,
                        text: instance
                            .plugin
                            .format_parameter(param.id, param.value)
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        instance
            .plugin
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
        self.plugin.latency_samples()
    }

    pub fn is_instrument(&self) -> bool {
        self.category.to_lowercase().contains("instrument")
            || self.category.to_lowercase().contains("synth")
    }

    pub fn set_param(&mut self, id: u32, value: f32) {
        let normalized = value.clamp(0.0, 1.0) as f64;
        if self.plugin.set_parameter(id, normalized).is_ok() {
            if let Some(param) = self.params.iter_mut().find(|p| p.id == id) {
                param.value = normalized as f32;
                param.text = self
                    .plugin
                    .format_parameter(id, normalized)
                    .unwrap_or_default();
            }
        }
    }

    pub fn save_state(&mut self) -> Result<Vec<u8>, String> {
        self.plugin
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
            let _ = self.plugin.send_live_midi_event(midi);
        }

        self.buffers.block_size = frames;
        self.buffers.inputs[0][..frames].copy_from_slice(&left[..frames]);
        self.buffers.inputs[1][..frames].copy_from_slice(&right[..frames]);
        self.buffers.outputs[0][..frames].fill(0.0);
        self.buffers.outputs[1][..frames].fill(0.0);

        self.plugin
            .process_audio(&mut self.buffers)
            .map_err(|error| format!("VST3 process failed: {error}"))?;

        left[..frames].copy_from_slice(&self.buffers.outputs[0][..frames]);
        right[..frames].copy_from_slice(&self.buffers.outputs[1][..frames]);
        Ok(())
    }
}

impl Drop for Vst3Instance {
    fn drop(&mut self) {
        let _ = self.plugin.stop_processing();
    }
}
