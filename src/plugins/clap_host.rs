//! In-process CLAP hosting.
//!
//! The instance and its audio processor live on the plugin worker thread.
//! Parameters are read while the plugin is inactive and written as CLAP
//! parameter events during processing, which is the portable path without
//! implementing every optional host extension.

use std::path::{Path, PathBuf};

use clack_common::events::event_types::{NoteOffEvent, NoteOnEvent, ParamValueEvent};
use clack_common::events::io::EventBuffer;
use clack_common::events::{Match, Pckn};
use clack_extensions::audio_ports::PluginAudioPorts;
use clack_extensions::params::{ParamInfoBuffer, PluginParams};
use clack_extensions::state::PluginState;
use clack_host::prelude::*;
use clack_host::process::PluginAudioProcessor;
use clack_host::utils::ClapId;

use super::{BlockEvent, ParamInfo, PLUGIN_BLOCK_FRAMES};

pub struct ClapInstance {
    // Declaration order matters: the processor must drop before the instance,
    // and the entry library must outlive both.
    processor: Option<PluginAudioProcessor<()>>,
    instance: PluginInstance<()>,
    /// Loaded library; must outlive the instance.
    _entry: PluginEntry,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    events: EventBuffer,
    path: PathBuf,
    name: String,
    vendor: String,
    version: String,
    params: Vec<ParamInfo>,
    has_audio_input: bool,
    sample_rate: f32,
}

impl ClapInstance {
    pub fn load(path: &Path, sample_rate: f32, state: Option<&[u8]>) -> Result<Self, String> {
        // SAFETY: loading arbitrary plugin binaries is inherently unsafe. The
        // user explicitly selected this file in the plugin browser.
        let entry = unsafe { PluginEntry::load(path) }
            .map_err(|error| format!("CLAP load failed: {error:?}"))?;

        let factory = entry
            .get_plugin_factory()
            .ok_or_else(|| "CLAP bundle exposes no plugin factory".to_string())?;
        let first = factory
            .plugin_descriptor(0)
            .ok_or_else(|| "CLAP bundle exposes no plugins".to_string())?;
        let id = first
            .id()
            .ok_or_else(|| "CLAP plugin has no id".to_string())?
            .to_owned();
        let name = first
            .name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_else(|| "CLAP plugin".to_string())
            });
        let vendor = first
            .vendor()
            .map(|vendor| vendor.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unknown".to_string());
        let version = first
            .version()
            .map(|version| version.to_string_lossy().to_string())
            .unwrap_or_else(|| "?".to_string());

        let host_info = HostInfo::new(
            "Replicant DAW",
            "Replicant",
            "https://github.com/DanimanX",
            "1.0",
        )
        .map_err(|error| format!("CLAP host info failed: {error}"))?;
        let mut instance = PluginInstance::<()>::new(|_| (), |_| (), &entry, &id, &host_info)
            .map_err(|error| format!("CLAP instantiation failed: {error}"))?;

        if let Some(state) = state {
            if let Some(state_ext) = instance.plugin_handle().get_extension::<PluginState>() {
                state_ext
                    .load(&instance.plugin_handle(), &mut &state[..])
                    .map_err(|error| format!("CLAP state restore failed: {error}"))?;
            }
        }

        let params = Self::read_params(&mut instance);
        let has_audio_input = instance
            .plugin_handle()
            .get_extension::<PluginAudioPorts>()
            .map(|ports| ports.count(&instance.plugin_handle(), true) > 0)
            .unwrap_or(true);

        let mut this = Self {
            processor: None,
            instance,
            _entry: entry,
            input: vec![vec![0.0; PLUGIN_BLOCK_FRAMES]; 2],
            output: vec![vec![0.0; PLUGIN_BLOCK_FRAMES]; 2],
            events: EventBuffer::with_capacity(super::MAX_BLOCK_EVENTS),
            path: path.to_path_buf(),
            name,
            vendor,
            version,
            params,
            has_audio_input,
            sample_rate,
        };
        this.activate()?;
        Ok(this)
    }

    fn read_params(instance: &mut PluginInstance<()>) -> Vec<ParamInfo> {
        let handle = instance.plugin_handle();
        let Some(params) = handle.get_extension::<PluginParams>() else {
            return Vec::new();
        };
        let count = params.count(&handle);
        let mut out = Vec::with_capacity(count.min(128) as usize);
        for index in 0..count.min(128) {
            let mut buffer = ParamInfoBuffer::new();
            let Some(info) = params.get_info(&handle, index, &mut buffer) else {
                continue;
            };
            let value = params
                .get_value(&handle, info.id)
                .unwrap_or(info.default_value);
            let span = (info.max_value - info.min_value).abs().max(1e-9);
            let normalized = ((value - info.min_value) / span).clamp(0.0, 1.0);
            out.push(ParamInfo {
                id: info.id.get(),
                name: String::from_utf8_lossy(info.name).to_string(),
                value: normalized as f32,
                text: format!("{value:.2}"),
            });
        }
        out
    }

    fn activate(&mut self) -> Result<(), String> {
        let config = PluginAudioConfiguration {
            sample_rate: self.sample_rate as f64,
            min_frames_count: 1,
            max_frames_count: PLUGIN_BLOCK_FRAMES as u32,
        };
        let stopped = self
            .instance
            .activate(|_, _| (), config)
            .map_err(|error| format!("CLAP activation failed: {error}"))?;
        let mut processor: PluginAudioProcessor<()> = stopped.into();
        processor
            .start_processing()
            .map_err(|error| format!("CLAP could not start processing: {error}"))?;
        self.processor = Some(processor);
        Ok(())
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

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn params(&self) -> &[ParamInfo] {
        &self.params
    }

    pub fn latency(&self) -> u32 {
        0
    }

    pub fn is_instrument(&self) -> bool {
        !self.has_audio_input
    }

    pub fn set_param(&mut self, id: u32, value: f32) {
        if let Some(param) = self.params.iter_mut().find(|param| param.id == id) {
            param.value = value.clamp(0.0, 1.0);
            param.text = format!("{:.2}", param.value);
        }
    }

    pub fn save_state(&mut self) -> Result<Vec<u8>, String> {
        if let Some(mut processor) = self.processor.take() {
            let _ = processor.stop_processing();
            drop(processor);
        }
        let _ = self.instance.try_deactivate_with(|_, _| ());

        let result = {
            let handle = self.instance.plugin_handle();
            match handle.get_extension::<PluginState>() {
                Some(state_ext) => {
                    let mut data = Vec::new();
                    state_ext
                        .save(&self.instance.plugin_handle(), &mut data)
                        .map(|_| data)
                        .map_err(|error| format!("CLAP state save failed: {error}"))
                }
                None => Ok(Vec::new()),
            }
        };

        self.activate()?;
        result
    }

    pub fn process(
        &mut self,
        frames: usize,
        left: &mut [f32],
        right: &mut [f32],
        events: &[BlockEvent],
    ) -> Result<(), String> {
        let Some(processor) = self.processor.as_mut() else {
            return Err("CLAP processor is not active".into());
        };

        // Build the event list for this block.
        self.events.clear();
        for event in events {
            let bytes = &event.bytes[..event.len as usize];
            if bytes.len() < 3 {
                continue;
            }
            let channel = (bytes[0] & 0x0f) as u16;
            match bytes[0] & 0xf0 {
                0x90 => {
                    let pckn = Pckn::new(0u16, channel, bytes[1] as u16, Match::All);
                    self.events.push(
                        &NoteOnEvent::new(event.offset as u32, pckn, bytes[2] as f64 / 127.0)
                            .as_unknown(),
                    );
                }
                0x80 => {
                    let pckn = Pckn::new(0u16, channel, bytes[1] as u16, Match::All);
                    self.events.push(
                        &NoteOffEvent::new(event.offset as u32, pckn, bytes[2] as f64 / 127.0)
                            .as_unknown(),
                    );
                }
                0xb0 => {
                    // Treat CC as a parameter change when the plugin exposes a
                    // matching parameter index; otherwise ignore it.
                    if let Some(param) = self.params.get(bytes[1] as usize) {
                        let id = ClapId::new(param.id);
                        self.events.push(
                            &ParamValueEvent::new(
                                event.offset as u32,
                                id,
                                Pckn::match_all(),
                                bytes[2] as f64 / 127.0,
                            )
                            .as_unknown(),
                        );
                    }
                }
                _ => {}
            }
        }

        // Copy the incoming audio into the plugin's ports.
        for channel in 0..2 {
            let source: &[f32] = if channel == 0 { &*left } else { &*right };
            self.input[channel][..frames].copy_from_slice(&source[..frames]);
            self.output[channel][..frames].fill(0.0);
        }

        let mut ports_in = AudioPorts::with_capacity(2, 1);
        let mut ports_out = AudioPorts::with_capacity(2, 1);

        let input_buffers = if self.has_audio_input {
            Some(
                ports_in.with_input_buffers([AudioPortBuffer {
                    latency: 0,
                    channels: AudioPortBufferType::f32_input_only(
                        self.input
                            .iter_mut()
                            .map(|channel| InputChannel::variable(&mut channel[..frames])),
                    ),
                }]),
            )
        } else {
            None
        };
        let output_buffers = ports_out.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(
                self.output.iter_mut().map(|channel| &mut channel[..frames]),
            ),
        }]);

        let input_events = self.events.as_input();
        let mut output_events = OutputEvents::void();

        let (inputs, mut outputs) = match input_buffers {
            Some(inputs) => (inputs, output_buffers),
            None => (InputAudioBuffers::empty(), output_buffers),
        };

        processor
            .as_started_mut()
            .map_err(|error| format!("CLAP processor is not started: {error}"))?
            .process(
                &inputs,
                &mut outputs,
                &input_events,
                &mut output_events,
                None,
                None,
            )
            .map_err(|error| format!("CLAP process failed: {error}"))?;

        left[..frames].copy_from_slice(&self.output[0][..frames]);
        right[..frames].copy_from_slice(&self.output[1][..frames]);
        Ok(())
    }
}
