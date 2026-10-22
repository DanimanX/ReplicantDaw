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
use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiSize, HostGui, HostGuiImpl, PluginGui,
};
use clack_extensions::params::{ParamInfoBuffer, PluginParams};
use clack_extensions::state::PluginState;
use clack_extensions::thread_check::{HostThreadCheck, HostThreadCheckImpl};
use clack_host::prelude::*;
use clack_host::process::PluginAudioProcessor;
use clack_host::utils::ClapId;

use super::{BlockEvent, ParamInfo, PLUGIN_BLOCK_FRAMES};

/// Host-side state shared with the plugin.
///
/// Registering the `clap.gui` host extension is mandatory for plugins that
/// embed their editor: u-he instruments abort when `host.get_extension("clap.gui")`
/// returns null, and several others silently refuse to draw.
pub struct HostShared {
    /// Thread that owns the plugin instance (this build's audio + main thread).
    owner: std::thread::ThreadId,
    /// Pending resize request encoded as `width << 32 | height` (0 = none).
    resize: std::sync::atomic::AtomicU64,
    show: std::sync::atomic::AtomicBool,
    hide: std::sync::atomic::AtomicBool,
    /// Set by the plugin when its floating window was closed by the user.
    closed: std::sync::atomic::AtomicBool,
}

impl Default for HostShared {
    fn default() -> Self {
        Self {
            owner: std::thread::current().id(),
            resize: Default::default(),
            show: Default::default(),
            hide: Default::default(),
            closed: Default::default(),
        }
    }
}

impl HostShared {
    fn take_resize(&self) -> Option<(u32, u32)> {
        let packed = self.resize.swap(0, std::sync::atomic::Ordering::Relaxed);
        (packed != 0).then_some(((packed >> 32) as u32, (packed & 0xffff_ffff) as u32))
    }

    fn take_show(&self) -> bool {
        self.show.swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    fn take_hide(&self) -> bool {
        self.hide.swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    fn is_closed(&self) -> bool {
        self.closed
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }
}

impl HostThreadCheckImpl for HostShared {
    fn is_main_thread(&self) -> bool {
        std::thread::current().id() == self.owner
    }

    /// Audio blocks and GUI calls share this thread in the prototype, so the
    /// owning thread is also the one that renders audio.
    fn is_audio_thread(&self) -> bool {
        std::thread::current().id() == self.owner
    }
}

impl<'a> SharedHandler<'a> for HostShared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}

impl HostGuiImpl for HostShared {
    fn resize_hints_changed(&self) {}

    fn request_resize(&self, new_size: GuiSize) -> Result<(), HostError> {
        let packed = ((new_size.width as u64) << 32) | new_size.height as u64;
        self.resize
            .store(packed, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    fn request_show(&self) -> Result<(), HostError> {
        self.show.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    fn request_hide(&self) -> Result<(), HostError> {
        self.hide.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    fn closed(&self, was_destroyed: bool) {
        if was_destroyed {
            self.closed
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// The host type handed to CLAP plugins.
pub struct ReplicantHost;

impl HostHandlers for ReplicantHost {
    type Shared<'a> = HostShared;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder.register::<HostGui>();
        builder.register::<HostThreadCheck>();
    }
}

pub struct ClapInstance {
    // Declaration order matters: the processor must drop before the instance,
    // and the entry library must outlive both.
    processor: Option<PluginAudioProcessor<ReplicantHost>>,
    instance: PluginInstance<ReplicantHost>,
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
    gui: Option<ClapGui>,
    /// Whether the plugin reported GUI support when it was loaded.
    gui_supported: bool,
}

/// The windowing API this build asks CLAP plugins to draw into.
fn preferred_gui_api() -> GuiApiType<'static> {
    #[cfg(target_os = "linux")]
    {
        GuiApiType::X11
    }
    #[cfg(target_os = "windows")]
    {
        GuiApiType::WIN32
    }
    #[cfg(target_os = "macos")]
    {
        GuiApiType::COCOA
    }
}

/// A plugin editor window the host owns and hands to the plugin.
///
/// CLAP plugins overwhelmingly implement *embedded* GUIs only (Dexed, TAL and
/// JC303 all report `floating = false`), so the host has to own a real window
/// and pass its handle. On Linux that window is a plain X11 toplevel created
/// through `xcb`; the plugin then draws into it.
struct ClapGui {
    #[cfg(target_os = "linux")]
    connection: Option<xcb::Connection>,
    #[cfg(target_os = "linux")]
    window: Option<xcb::x::Window>,
    created: bool,
    floating: bool,
    title: String,
}

impl ClapGui {
    fn new(title: String) -> Self {
        Self {
            #[cfg(target_os = "linux")]
            connection: None,
            #[cfg(target_os = "linux")]
            window: None,
            created: false,
            floating: false,
            title,
        }
    }

    /// Create the native parent window the plugin will draw into.
    #[allow(unused_variables)]
    fn create_parent(&mut self, width: u16, height: u16) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            let (connection, screen_number) = xcb::Connection::connect(None)
                .map_err(|error| format!("could not reach the X server: {error}"))?;
            let screen = connection
                .get_setup()
                .roots()
                .nth(screen_number as usize)
                .ok_or_else(|| "the X server reports no screen".to_string())?;
            let window = connection.generate_id();
            connection
                .send_and_check_request(&xcb::x::CreateWindow {
                    depth: xcb::x::COPY_FROM_PARENT as u8,
                    wid: window,
                    parent: screen.root(),
                    x: 0,
                    y: 0,
                    width,
                    height,
                    border_width: 0,
                    class: xcb::x::WindowClass::InputOutput,
                    visual: screen.root_visual(),
                    value_list: &[
                        xcb::x::Cw::BackPixel(screen.white_pixel()),
                        xcb::x::Cw::EventMask(
                            xcb::x::EventMask::EXPOSURE
                                | xcb::x::EventMask::KEY_PRESS
                                | xcb::x::EventMask::STRUCTURE_NOTIFY,
                        ),
                    ],
                })
                .map_err(|error| format!("could not create the editor window: {error}"))?;
            connection.send_request(&xcb::x::ChangeProperty {
                mode: xcb::x::PropMode::Replace,
                window,
                property: xcb::x::ATOM_WM_NAME,
                r#type: xcb::x::ATOM_STRING,
                data: self.title.as_bytes(),
            });
            let _ = connection.flush();
            self.connection = Some(connection);
            self.window = Some(window);
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(
                "native CLAP editor windows are only wired for X11 in this build; \
                 use the parameter editor or a VST3 build of the plugin"
                    .to_string(),
            )
        }
    }

    /// Handle to hand to `set_parent`.
    fn parent_handle(&self) -> Result<clack_extensions::gui::Window<'static, 'static>, String> {
        #[cfg(target_os = "linux")]
        {
            use xcb::Xid;
            let window = self
                .window
                .ok_or_else(|| "the editor window was not created".to_string())?;
            Ok(clack_extensions::gui::Window::from_x11_handle(
                window.resource_id() as std::os::raw::c_ulong,
            ))
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("no native CLAP parent window on this platform".to_string())
        }
    }

    /// Map (show) the parent window.
    fn map_parent(&self) {
        #[cfg(target_os = "linux")]
        if let (Some(connection), Some(window)) = (&self.connection, self.window) {
            connection.send_request(&xcb::x::MapWindow { window });
            let _ = connection.flush();
        }
    }

    /// Unmap the parent (the plugin asked to hide its editor).
    fn unmap_parent(&self) {
        #[cfg(target_os = "linux")]
        if let (Some(connection), Some(window)) = (&self.connection, self.window) {
            connection.send_request(&xcb::x::UnmapWindow { window });
            let _ = connection.flush();
        }
    }

    /// Resize the parent so an embedded editor fits exactly.
    fn resize_parent(&self, width: u16, height: u16) {
        #[cfg(target_os = "linux")]
        if let (Some(connection), Some(window)) = (&self.connection, self.window) {
            connection.send_request(&xcb::x::ConfigureWindow {
                window,
                value_list: &[
                    xcb::x::ConfigWindow::Width(width as u32),
                    xcb::x::ConfigWindow::Height(height as u32),
                ],
            });
            let _ = connection.flush();
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (width, height);
    }

    /// Unmap and destroy the parent window. Must run after the plugin's
    /// `destroy`, never before.
    fn teardown(&mut self) {
        #[cfg(target_os = "linux")]
        {
            if let (Some(connection), Some(window)) = (self.connection.take(), self.window.take()) {
                connection.send_request(&xcb::x::DestroyWindow { window });
                let _ = connection.flush();
            }
        }
        self.created = false;
    }

    fn is_open(&self) -> bool {
        self.created
    }
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
        let mut instance = PluginInstance::<ReplicantHost>::new(
            |_| HostShared::default(),
            |_| (),
            &entry,
            &id,
            &host_info,
        )
        .map_err(|error| format!("CLAP instantiation failed: {error}"))?;

        if let Some(state) = state {
            if let Some(state_ext) = instance.plugin_handle().get_extension::<PluginState>() {
                state_ext
                    .load(&instance.plugin_handle(), &mut &state[..])
                    .map_err(|error| format!("CLAP state restore failed: {error}"))?;
            }
        }

        let params = Self::read_params(&mut instance);
        let gui_supported = Self::probe_gui(&mut instance);
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
            gui: None,
            gui_supported,
        };
        this.activate()?;
        Ok(this)
    }

    /// Ask the plugin whether it exposes a GUI extension.
    ///
    /// Deliberately does not call `is_api_supported`: u-he instruments answer
    /// that by querying their own size before `create`, which aborts some of
    /// them. The real negotiation happens in [`ClapInstance::open_gui`].
    fn probe_gui(instance: &mut PluginInstance<ReplicantHost>) -> bool {
        instance
            .plugin_handle()
            .get_extension::<PluginGui>()
            .is_some()
    }

    fn read_params(instance: &mut PluginInstance<ReplicantHost>) -> Vec<ParamInfo> {
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
        let mut processor: PluginAudioProcessor<ReplicantHost> = stopped.into();
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

    /// Does this plugin expose a GUI at all on the current platform?
    pub fn has_gui(&self) -> bool {
        self.gui_supported
    }

    /// Can this plugin draw a GUI on the current platform?
    pub fn supports_gui(&mut self) -> bool {
        let api = preferred_gui_api();
        let handle = self.instance.plugin_handle();
        let Some(gui) = handle.get_extension::<PluginGui>() else {
            return false;
        };
        let embedded = GuiConfiguration {
            api_type: api,
            is_floating: false,
        };
        let floating = GuiConfiguration {
            api_type: api,
            is_floating: true,
        };
        gui.is_api_supported(&handle, embedded) || gui.is_api_supported(&handle, floating)
    }

    pub fn is_gui_open(&self) -> bool {
        self.gui.as_ref().is_some_and(ClapGui::is_open)
    }

    /// Create the plugin's editor window and show it.
    pub fn open_gui(&mut self) -> Result<(), String> {
        if self.is_gui_open() {
            return Ok(());
        }
        self.destroy_gui();

        let api = preferred_gui_api();
        let mut gui = ClapGui::new(format!("{} — {}", self.name, self.vendor));

        let embedded = GuiConfiguration {
            api_type: api,
            is_floating: false,
        };
        let floating = GuiConfiguration {
            api_type: api,
            is_floating: true,
        };

        let (use_floating, size) = {
            let handle = self.instance.plugin_handle();
            let gui_ext = handle
                .get_extension::<PluginGui>()
                .ok_or_else(|| format!("{} exposes no CLAP GUI extension", self.name))?;
            if gui_ext.is_api_supported(&handle, embedded) {
                let size = gui_ext.get_size(&handle);
                (false, size)
            } else if gui_ext.is_api_supported(&handle, floating) {
                (true, gui_ext.get_size(&handle))
            } else {
                return Err(format!(
                    "{} does not support a {} GUI in this build",
                    self.name,
                    api.0.to_string_lossy()
                ));
            }
        };

        let width = size.map(|size| size.width).unwrap_or(900).clamp(320, 2400) as u16;
        let height = size.map(|size| size.height).unwrap_or(620).clamp(240, 1600) as u16;

        // Embedded editors are sensitive to the parent being mapped *before*
        // `set_parent`: several toolkits (JUCE, among others) query the window
        // attributes on attach and stay black if it is still unmapped.
        if !use_floating {
            gui.create_parent(width, height)?;
            gui.map_parent();
        }

        let handle = self.instance.plugin_handle();
        let gui_ext = handle
            .get_extension::<PluginGui>()
            .ok_or_else(|| format!("{} exposes no CLAP GUI extension", self.name))?;
        let configuration = if use_floating { floating } else { embedded };
        gui_ext
            .create(&handle, configuration)
            .map_err(|error| format!("CLAP GUI create failed: {error:?}"))?;
        gui.created = true;

        if use_floating {
            gui.floating = true;
            let title = std::ffi::CString::new(gui.title.clone()).unwrap_or_default();
            gui_ext.suggest_title(&handle, &title);
        } else {
            // Deliberately minimal: CLAP only guarantees `create`, `set_parent`
            // and `show` for embedded editors. Sizing calls before the parent
            // exists crash some plugins (Dexed segfaults inside `set_scale`),
            // so the parent is created at a sane default and resized later from
            // the size the plugin reports.
            if let Err(error) = gui.create_parent(900, 620) {
                gui_ext.destroy(&handle);
                gui.created = false;
                gui.teardown();
                return Err(error);
            }
            gui.map_parent();

            let parent = gui.parent_handle()?;
            // SAFETY: the X11 window lives in `gui` until after `destroy`.
            if let Err(error) = unsafe { gui_ext.set_parent(&handle, parent) } {
                gui_ext.destroy(&handle);
                gui.created = false;
                gui.teardown();
                return Err(format!("CLAP GUI set_parent failed: {error:?}"));
            }

            if let Some(size) = gui_ext.get_size(&handle) {
                gui.resize_parent(size.width as u16, size.height as u16);
            }
        }

        if let Err(error) = gui_ext.show(&handle) {
            gui_ext.destroy(&handle);
            gui.created = false;
            gui.teardown();
            return Err(format!("CLAP GUI show failed: {error:?}"));
        }

        self.gui = Some(gui);
        Ok(())
    }

    /// Answer the requests the plugin made through `clap_host_gui`.
    ///
    /// Runs on the worker thread while the editor is open: a resize request
    /// grows/shrinks our parent window, `hide` unmaps it and a destroyed
    /// window tears the editor down.
    pub fn service_gui(&mut self) {
        let Some(gui) = self.gui.as_ref() else {
            return;
        };
        if !gui.created {
            return;
        }
        let resize = self
            .instance
            .access_shared_handler(|shared| shared.take_resize());
        let show = self
            .instance
            .access_shared_handler(|shared| shared.take_show());
        let hide = self
            .instance
            .access_shared_handler(|shared| shared.take_hide());
        if show {
            if let Some(gui) = self.gui.as_ref() {
                gui.map_parent();
            }
        }
        let closed = self
            .instance
            .access_shared_handler(|shared| shared.is_closed());

        if let Some((width, height)) = resize {
            if let Some(gui) = self.gui.as_ref() {
                gui.resize_parent(
                    width.clamp(160, 4096) as u16,
                    height.clamp(120, 4096) as u16,
                );
            }
        }
        if hide {
            if let Some(gui) = self.gui.as_ref() {
                gui.unmap_parent();
            }
        }
        if closed {
            self.destroy_gui();
        }
    }

    pub fn close_gui(&mut self) {
        self.destroy_gui();
    }

    fn destroy_gui(&mut self) {
        let Some(mut gui) = self.gui.take() else {
            return;
        };
        if gui.created {
            let handle = self.instance.plugin_handle();
            if let Some(gui_ext) = handle.get_extension::<PluginGui>() {
                let _ = gui_ext.hide(&handle);
                gui_ext.destroy(&handle);
            }
        }
        gui.teardown();
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

    /// Tear the editor down; `Drop` cannot run plugin code safely.
    pub fn shutdown(&mut self) {
        self.destroy_gui();
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
