//! The plugin worker thread.
//!
//! Third-party plugin handles are not `Send`, and plugin DSP is not guaranteed
//! to be real-time safe. Keeping them on a dedicated thread lets the audio
//! callback stay lock-free: it exchanges 512-frame stereo blocks through
//! `rtrb` rings and adds one block of latency when a plugin is active.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};

use super::clap_host::ClapInstance;
use super::vst3_host::Vst3Instance;
use super::{
    ParamInfo, PluginBlock, PluginDescriptor, PluginFormat, MAX_PLUGIN_SLOTS, PLUGIN_BLOCK_FRAMES,
};

const COMMAND_CAPACITY: usize = 256;
const EVENT_CAPACITY: usize = 64;
const AUDIO_RING_BLOCKS: usize = 8;

#[derive(Clone, Debug)]
pub struct PluginSlotInfo {
    pub slot: usize,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub format: PluginFormat,
    pub path: PathBuf,
    pub bypass: bool,
    pub params: Vec<ParamInfo>,
    pub latency: u32,
    pub is_instrument: bool,
    pub has_editor: bool,
    pub gui_open: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub enum PluginCommand {
    Load {
        slot: u8,
        descriptor: PluginDescriptor,
        state: Option<Vec<u8>>,
    },
    Unload {
        slot: u8,
    },
    Bypass {
        slot: u8,
        bypass: bool,
    },
    SetParam {
        slot: u8,
        id: u32,
        value: f32,
    },
    SaveState {
        request: u64,
    },
    /// Update the sample rate used by plugins loaded from now on.
    SetSampleRate {
        sample_rate: f32,
    },
    /// Open the plugin's own editor window (its native GUI).
    ///
    /// `transient_for` is the DAW's own window handle, so the editor can be a
    /// floating window that belongs to it.
    OpenGui {
        slot: u8,
        transient_for: Option<u64>,
    },
    /// Re-read the parameters of every loaded plugin.
    ///
    /// Editing inside the plugin's own window changes values the host never
    /// sees otherwise, so the rack's knobs would drift.
    RefreshParams,
    CloseGui {
        slot: u8,
    },
}

#[derive(Clone, Debug)]
pub enum PluginEvent {
    Slots(Vec<PluginSlotInfo>),
    State {
        request: u64,
        slots: Vec<(u8, Vec<u8>)>,
    },
    /// Result of an editor-window request, so the UI can report failures.
    Gui {
        slot: u8,
        open: bool,
        message: Option<String>,
    },
    Log(String),
}

enum Instance {
    Clap(Box<ClapInstance>),
    Vst3(Box<Vst3Instance>),
}

impl Instance {
    fn name(&self) -> &str {
        match self {
            Self::Clap(plugin) => plugin.name(),
            Self::Vst3(plugin) => plugin.name(),
        }
    }

    fn vendor(&self) -> &str {
        match self {
            Self::Clap(plugin) => plugin.vendor(),
            Self::Vst3(plugin) => plugin.vendor(),
        }
    }

    fn version(&self) -> &str {
        match self {
            Self::Clap(plugin) => plugin.version(),
            Self::Vst3(plugin) => plugin.version(),
        }
    }

    fn path(&self) -> &std::path::Path {
        match self {
            Self::Clap(plugin) => plugin.path(),
            Self::Vst3(plugin) => plugin.path(),
        }
    }

    fn params(&self) -> &[ParamInfo] {
        match self {
            Self::Clap(plugin) => plugin.params(),
            Self::Vst3(plugin) => plugin.params(),
        }
    }

    fn latency(&self) -> u32 {
        match self {
            Self::Clap(plugin) => plugin.latency(),
            Self::Vst3(plugin) => plugin.latency(),
        }
    }

    fn is_instrument(&self) -> bool {
        match self {
            Self::Clap(plugin) => plugin.is_instrument(),
            Self::Vst3(plugin) => plugin.is_instrument(),
        }
    }

    fn is_vst3(&self) -> bool {
        matches!(self, Self::Vst3(_))
    }

    fn set_param(&mut self, id: u32, value: f32) {
        match self {
            Self::Clap(plugin) => plugin.set_param(id, value),
            Self::Vst3(plugin) => plugin.set_param(id, value),
        }
    }

    fn refresh_params(&mut self) {
        match self {
            Self::Clap(plugin) => plugin.refresh_params(),
            Self::Vst3(plugin) => plugin.refresh_params(),
        }
    }

    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        match self {
            Self::Clap(plugin) => plugin.save_state(),
            Self::Vst3(plugin) => plugin.save_state(),
        }
    }

    fn process(
        &mut self,
        frames: usize,
        left: &mut [f32],
        right: &mut [f32],
        events: &[super::BlockEvent],
    ) -> Result<(), String> {
        match self {
            Self::Clap(plugin) => plugin.process(frames, left, right, events),
            Self::Vst3(plugin) => plugin.process(frames, left, right, events),
        }
    }
}

struct Slot {
    instance: Instance,
    bypass: bool,
    error: Option<String>,
}

struct Worker {
    slots: Vec<Option<Slot>>,
    audio_rx: Consumer<PluginBlock>,
    audio_tx: Producer<PluginBlock>,
    command_rx: Consumer<PluginCommand>,
    event_tx: Producer<PluginEvent>,
    sample_rate: f32,
    midi_enabled: bool,
    /// VST3 instances removed from their slot. Some vendors crash inside
    /// their own destructor, so they are stopped and kept alive instead of
    /// being dropped. The OS reclaims them when the process exits.
    retired: Vec<Instance>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Never tear down VST3 components during shutdown either: the crash
        // would turn a normal quit into a core dump.
        for slot in self.slots.iter_mut() {
            if let Some(mut slot) = slot.take() {
                match &mut slot.instance {
                    Instance::Vst3(plugin) => {
                        plugin.close_gui();
                        plugin.stop();
                    }
                    Instance::Clap(plugin) => plugin.shutdown(),
                }
                match slot.instance {
                    Instance::Vst3(plugin) => std::mem::forget(plugin),
                    Instance::Clap(plugin) => drop(plugin),
                }
            }
        }
        for instance in std::mem::take(&mut self.retired) {
            if let Instance::Vst3(plugin) = instance {
                std::mem::forget(plugin);
            }
        }
    }
}

impl Worker {
    fn run(mut self, stop: Arc<AtomicBool>) {
        // Editor event loops need frequent servicing, but not on every audio
        // block; ~250 Hz is plenty for GUI input and plugin timers.
        let mut last_gui_service = std::time::Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let mut did_work = false;

            if last_gui_service.elapsed() >= Duration::from_millis(4) {
                last_gui_service = std::time::Instant::now();
                self.service_gui_windows();
            }

            for _ in 0..16 {
                match self.command_rx.pop() {
                    Ok(command) => {
                        self.handle_command(command);
                        did_work = true;
                    }
                    Err(_) => break,
                }
            }

            match self.audio_rx.pop() {
                Ok(mut block) => {
                    self.process_block(&mut block);
                    if self.audio_tx.push(block).is_err() {
                        // The engine is not draining; dropping is better than
                        // blocking or unbounded latency.
                    }
                }
                Err(_) => {
                    if !did_work {
                        thread::sleep(Duration::from_micros(150));
                    }
                }
            }
        }
    }

    fn handle_command(&mut self, command: PluginCommand) {
        match command {
            PluginCommand::Load {
                slot,
                descriptor,
                state,
            } => {
                let index = slot as usize;
                if index >= MAX_PLUGIN_SLOTS {
                    return;
                }
                self.slots[index] = None;
                let loaded = match descriptor.format {
                    PluginFormat::Clap => ClapInstance::load(
                        &descriptor.path,
                        self.sample_rate,
                        state.as_deref(),
                    )
                    .map(|plugin| Instance::Clap(Box::new(plugin))),
                    PluginFormat::Vst3 => Vst3Instance::load(
                        &descriptor.path,
                        self.sample_rate,
                        state.as_deref(),
                    )
                    .map(|plugin| Instance::Vst3(Box::new(plugin))),
                    PluginFormat::Vst2 => {
                        Err("VST2 hosting requires the proprietary Steinberg SDK, which this build does not vendor. Use the VST3 or CLAP build.".to_string())
                    }
                };

                match loaded {
                    Ok(instance) => {
                        self.slots[index] = Some(Slot {
                            instance,
                            bypass: false,
                            error: None,
                        });
                    }
                    Err(error) => {
                        let _ = self.event_tx.push(PluginEvent::Log(error));
                    }
                }
                self.emit_slots();
            }
            PluginCommand::Unload { slot } => {
                if let Some(entry) = self.slots.get_mut(slot as usize) {
                    if let Some(mut removed) = entry.take() {
                        match &mut removed.instance {
                            Instance::Clap(plugin) => plugin.shutdown(),
                            Instance::Vst3(_) => {}
                        }
                        if removed.instance.is_vst3() {
                            if let Instance::Vst3(plugin) = &mut removed.instance {
                                plugin.stop();
                            }
                            self.retired.push(removed.instance);
                        }
                        // CLAP instances drop normally.
                    }
                }
                self.emit_slots();
            }
            PluginCommand::Bypass { slot, bypass } => {
                if let Some(Some(slot)) = self.slots.get_mut(slot as usize) {
                    slot.bypass = bypass;
                }
                self.emit_slots();
            }
            PluginCommand::SetParam { slot, id, value } => {
                if let Some(Some(slot)) = self.slots.get_mut(slot as usize) {
                    slot.instance.set_param(id, value);
                }
            }
            PluginCommand::SetSampleRate { sample_rate } => {
                self.sample_rate = sample_rate;
            }
            PluginCommand::OpenGui {
                slot,
                transient_for,
            } => {
                let index = slot as usize;
                let mut message = None;
                let opened = match self.slots.get_mut(index).and_then(Option::as_mut) {
                    Some(slot) => {
                        let result = match &mut slot.instance {
                            Instance::Clap(plugin) => plugin.open_gui(transient_for),
                            Instance::Vst3(plugin) => plugin.open_gui(),
                        };
                        match result {
                            Ok(()) => true,
                            Err(error) => {
                                message = Some(error);
                                false
                            }
                        }
                    }
                    None => {
                        message = Some("no plugin loaded in this slot".to_string());
                        false
                    }
                };
                if let Some(message) = &message {
                    let _ = self.event_tx.push(PluginEvent::Log(message.clone()));
                }
                let _ = self.event_tx.push(PluginEvent::Gui {
                    slot,
                    open: opened,
                    message,
                });
                self.emit_slots();
            }
            PluginCommand::RefreshParams => {
                for slot in self.slots.iter_mut().flatten() {
                    slot.instance.refresh_params();
                }
                self.emit_slots();
            }
            PluginCommand::CloseGui { slot } => {
                if let Some(slot) = self.slots.get_mut(slot as usize).and_then(Option::as_mut) {
                    match &mut slot.instance {
                        Instance::Clap(plugin) => plugin.close_gui(),
                        Instance::Vst3(plugin) => plugin.close_gui(),
                    }
                }
                let _ = self.event_tx.push(PluginEvent::Gui {
                    slot,
                    open: false,
                    message: None,
                });
                self.emit_slots();
            }
            PluginCommand::SaveState { request } => {
                let mut states = Vec::new();
                for (index, slot) in self.slots.iter_mut().enumerate() {
                    if let Some(slot) = slot {
                        match slot.instance.save_state() {
                            Ok(bytes) => states.push((index as u8, bytes)),
                            Err(error) => {
                                let _ = self
                                    .event_tx
                                    .push(PluginEvent::Log(format!("slot {index}: {error}")));
                            }
                        }
                    }
                }
                let _ = self.event_tx.push(PluginEvent::State {
                    request,
                    slots: states,
                });
            }
        }
    }

    /// Pump every open editor window.
    fn service_gui_windows(&mut self) {
        for slot in self.slots.iter_mut().flatten() {
            match &mut slot.instance {
                Instance::Clap(plugin) => plugin.service_gui(),
                Instance::Vst3(plugin) => plugin.service_gui(),
            }
        }
    }

    fn emit_slots(&mut self) {
        let mut infos = Vec::with_capacity(MAX_PLUGIN_SLOTS);
        for (index, slot) in self.slots.iter().enumerate() {
            if let Some(slot) = slot {
                infos.push(PluginSlotInfo {
                    slot: index,
                    name: slot.instance.name().to_string(),
                    vendor: slot.instance.vendor().to_string(),
                    version: slot.instance.version().to_string(),
                    format: match &slot.instance {
                        Instance::Clap(_) => PluginFormat::Clap,
                        Instance::Vst3(_) => PluginFormat::Vst3,
                    },
                    path: slot.instance.path().to_path_buf(),
                    bypass: slot.bypass,
                    params: slot.instance.params().to_vec(),
                    latency: slot.instance.latency(),
                    is_instrument: slot.instance.is_instrument(),
                    has_editor: match &slot.instance {
                        Instance::Clap(plugin) => plugin.has_gui(),
                        Instance::Vst3(plugin) => plugin.has_editor(),
                    },
                    gui_open: match &slot.instance {
                        Instance::Clap(plugin) => plugin.is_gui_open(),
                        Instance::Vst3(plugin) => plugin.is_gui_open(),
                    },
                    error: slot.error.clone(),
                });
            }
        }
        let _ = self.event_tx.push(PluginEvent::Slots(infos));
    }

    fn process_block(&mut self, block: &mut PluginBlock) {
        let frames = (block.frames as usize).min(PLUGIN_BLOCK_FRAMES);
        if frames == 0 {
            return;
        }

        // Each track bus goes through its own insert slot. Slots that are
        // empty or bypassed leave the bus untouched.
        for (track, slot) in self.slots.iter_mut().enumerate() {
            let Some(slot) = slot else {
                continue;
            };
            if slot.bypass || track >= block.left.len() {
                continue;
            }

            let mut routed = [super::BlockEvent::default(); super::MAX_BLOCK_EVENTS];
            let mut routed_count = 0usize;
            if self.midi_enabled {
                for event in &block.events[..block.event_count as usize] {
                    if event.bus as usize == track && routed_count < routed.len() {
                        routed[routed_count] = *event;
                        routed_count += 1;
                    }
                }
            }
            let events = &routed[..routed_count];
            let instrument = slot.instance.is_instrument();

            // Instruments generate their own signal, so their output is added
            // to the dry track instead of replacing it. Effects replace it.
            let mut dry_left = [0.0_f32; PLUGIN_BLOCK_FRAMES];
            let mut dry_right = [0.0_f32; PLUGIN_BLOCK_FRAMES];
            if instrument {
                dry_left[..frames].copy_from_slice(&block.left[track][..frames]);
                dry_right[..frames].copy_from_slice(&block.right[track][..frames]);
            }

            let left = &mut block.left[track][..frames];
            let right = &mut block.right[track][..frames];

            match slot.instance.process(frames, left, right, events) {
                Ok(()) => {
                    if instrument {
                        for index in 0..frames {
                            block.left[track][index] += dry_left[index];
                            block.right[track][index] += dry_right[index];
                        }
                    }
                }
                Err(error) => {
                    // Fail open: keep the dry signal and report once.
                    if slot.error.as_deref() != Some(error.as_str()) {
                        slot.error = Some(error.clone());
                        let _ = self.event_tx.push(PluginEvent::Log(format!(
                            "{}: {error}",
                            slot.instance.name()
                        )));
                    }
                }
            }
        }
    }
}

/// UI-side handle to the plugin worker.
pub struct PluginRuntime {
    pub command_tx: Producer<PluginCommand>,
    pub event_rx: Consumer<PluginEvent>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    pub sample_rate: f32,
}

impl PluginRuntime {
    /// Start the worker and return the audio-ring endpoints for the engine.
    pub fn start(sample_rate: f32) -> (Self, Producer<PluginBlock>, Consumer<PluginBlock>) {
        let (command_tx, command_rx) = RingBuffer::new(COMMAND_CAPACITY);
        let (event_tx, event_rx) = RingBuffer::new(EVENT_CAPACITY);
        let (audio_in_tx, audio_in_rx) = RingBuffer::new(AUDIO_RING_BLOCKS);
        let (audio_out_tx, audio_out_rx) = RingBuffer::new(AUDIO_RING_BLOCKS);

        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);

        // The worker is built *inside* the thread: plugin handles are not
        // `Send`, so they must never cross the thread boundary.
        let handle = thread::Builder::new()
            .name("replicant-plugins".into())
            .spawn(move || {
                let worker = Worker {
                    slots: (0..MAX_PLUGIN_SLOTS).map(|_| None).collect(),
                    audio_rx: audio_in_rx,
                    audio_tx: audio_out_tx,
                    command_rx,
                    event_tx,
                    sample_rate,
                    midi_enabled: true,
                    retired: Vec::new(),
                };
                worker.run(stop_flag);
            })
            .expect("spawn plugin worker");

        (
            Self {
                command_tx,
                event_rx,
                stop,
                worker: Some(handle),
                sample_rate,
            },
            audio_in_tx,
            audio_out_rx,
        )
    }

    pub fn send(&mut self, command: PluginCommand) {
        if self.command_tx.push(command).is_err() {
            log::warn!("Plugin command queue is full; command dropped");
        }
    }
}

impl Drop for PluginRuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
