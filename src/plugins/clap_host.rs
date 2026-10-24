//! In-process CLAP hosting.
//!
//! The instance and its audio processor live on the plugin worker thread.
//! Parameters are read while the plugin is inactive and written as CLAP
//! parameter events during processing, which is the portable path without
//! implementing every optional host extension.

use std::path::{Path, PathBuf};

use clack_common::events::event_types::{MidiEvent, NoteOffEvent, NoteOnEvent, ParamValueEvent};
use clack_common::events::io::EventBuffer;
use clack_common::events::{Match, Pckn};
use clack_extensions::audio_ports::PluginAudioPorts;
use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiSize, HostGui, HostGuiImpl, PluginGui,
};
use clack_extensions::params::{ParamInfoBuffer, PluginParams};
#[cfg(unix)]
use clack_extensions::posix_fd::{FdFlags, HostPosixFd, HostPosixFdImpl, PluginPosixFd};
use clack_extensions::state::PluginState;
use clack_extensions::thread_check::{HostThreadCheck, HostThreadCheckImpl};
use clack_extensions::timer::{HostTimer, HostTimerImpl, PluginTimer, TimerId};
use clack_host::prelude::*;
use clack_host::process::PluginAudioProcessor;
use clack_host::utils::ClapId;

use super::{BlockEvent, ParamInfo, PLUGIN_BLOCK_FRAMES};

/// Host-side state shared with the plugin.
///
/// Registering the `clap.gui` host extension is mandatory for plugins that
/// embed their editor: u-he instruments abort when `host.get_extension("clap.gui")`
/// returns null, and several others silently refuse to draw.
/// A timer the plugin registered through `clap_host_timer_support`.
struct HostTimerEntry {
    id: TimerId,
    period: std::time::Duration,
    next: std::time::Instant,
}

/// Mutable registries the host keeps for one plugin instance.
#[derive(Default)]
pub struct HostReactor {
    timers: std::sync::Mutex<Vec<HostTimerEntry>>,
    next_timer: std::sync::atomic::AtomicU32,
    #[cfg(unix)]
    fds: std::sync::Mutex<Vec<(std::os::unix::io::RawFd, FdFlags)>>,
}

impl HostReactor {
    /// Timer ids whose period elapsed; reschedules them.
    fn due_timers(&self) -> Vec<TimerId> {
        let Ok(mut timers) = self.timers.lock() else {
            return Vec::new();
        };
        let now = std::time::Instant::now();
        let mut due = Vec::new();
        for timer in timers.iter_mut() {
            if timer.next <= now {
                due.push(timer.id);
                // Keep the cadence but never pile up if we were stalled.
                timer.next = now + timer.period;
            }
        }
        due
    }

    /// File descriptors that are ready for the events the plugin asked for.
    #[cfg(unix)]
    fn ready_fds(&self) -> Vec<(std::os::unix::io::RawFd, FdFlags)> {
        let Ok(fds) = self.fds.lock() else {
            return Vec::new();
        };
        if fds.is_empty() {
            return Vec::new();
        }
        let mut pollfds: Vec<libc::pollfd> = fds
            .iter()
            .map(|(fd, flags)| libc::pollfd {
                fd: *fd,
                events: {
                    let mut events = 0;
                    if flags.contains(FdFlags::READ) {
                        events |= libc::POLLIN;
                    }
                    if flags.contains(FdFlags::WRITE) {
                        events |= libc::POLLOUT;
                    }
                    events
                },
                revents: 0,
            })
            .collect();
        // Non-blocking: the worker calls this on every tick.
        let ready = unsafe { libc::poll(pollfds.as_mut_ptr(), pollfds.len() as libc::nfds_t, 0) };
        if ready <= 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (index, pollfd) in pollfds.iter().enumerate() {
            if pollfd.revents == 0 {
                continue;
            }
            let mut flags = FdFlags::empty();
            if pollfd.revents & (libc::POLLIN | libc::POLLPRI) != 0 {
                flags |= FdFlags::READ;
            }
            if pollfd.revents & libc::POLLOUT != 0 {
                flags |= FdFlags::WRITE;
            }
            if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                flags |= FdFlags::ERROR;
            }
            out.push((fds[index].0, flags));
        }
        out
    }
}

pub struct HostShared {
    /// Thread that owns the plugin instance (this build's audio + main thread).
    owner: std::thread::ThreadId,
    /// Timers and file descriptors the plugin registered with the host.
    reactor: std::sync::Arc<HostReactor>,
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
            reactor: Default::default(),
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

    /// Audio blocks and GUI calls share this thread, so the
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

/// The host's `[main-thread]` data: it only needs the reactor the plugin
/// registers its timers and file descriptors into.
pub struct HostMainThread<'a> {
    reactor: std::sync::Arc<HostReactor>,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

impl<'a> MainThreadHandler<'a> for HostMainThread<'a> {}

impl HostTimerImpl for HostMainThread<'_> {
    fn register_timer(&self, period_ms: u32) -> Result<TimerId, HostError> {
        let period = std::time::Duration::from_millis(u64::from(period_ms.clamp(1, 1000)));
        let id = TimerId(
            self.reactor
                .next_timer
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        );
        if let Ok(mut timers) = self.reactor.timers.lock() {
            timers.push(HostTimerEntry {
                id,
                period,
                next: std::time::Instant::now() + period,
            });
        }
        Ok(id)
    }

    fn unregister_timer(&self, timer_id: TimerId) -> Result<(), HostError> {
        if let Ok(mut timers) = self.reactor.timers.lock() {
            timers.retain(|timer| timer.id != timer_id);
        }
        Ok(())
    }
}

#[cfg(unix)]
impl HostPosixFdImpl for HostMainThread<'_> {
    fn register_fd(&self, fd: std::os::unix::io::RawFd, flags: FdFlags) -> Result<(), HostError> {
        if let Ok(mut fds) = self.reactor.fds.lock() {
            fds.retain(|(existing, _)| *existing != fd);
            fds.push((fd, flags));
        }
        Ok(())
    }

    fn modify_fd(&self, fd: std::os::unix::io::RawFd, flags: FdFlags) -> Result<(), HostError> {
        if let Ok(mut fds) = self.reactor.fds.lock() {
            match fds.iter_mut().find(|(existing, _)| *existing == fd) {
                Some(entry) => entry.1 = flags,
                None => fds.push((fd, flags)),
            }
        }
        Ok(())
    }

    fn unregister_fd(&self, fd: std::os::unix::io::RawFd) -> Result<(), HostError> {
        if let Ok(mut fds) = self.reactor.fds.lock() {
            fds.retain(|(existing, _)| *existing != fd);
        }
        Ok(())
    }
}

/// The host type handed to CLAP plugins.
pub struct ReplicantHost;

impl HostHandlers for ReplicantHost {
    type Shared<'a> = HostShared;
    type MainThread<'a> = HostMainThread<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder.register::<HostGui>();
        builder.register::<HostThreadCheck>();
        // GUI toolkits (JUCE among others) drive their editor from a timer and
        // from the X11 socket: without these the plugin draws once and then
        // ignores every click.
        builder.register::<HostTimer>();
        #[cfg(unix)]
        builder.register::<HostPosixFd>();
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
    /// Parameter changes queued by the host, delivered as CLAP parameter
    /// events at the start of the next block.
    ///
    /// CLAP has no host-side `set_value`: parameter automation is expressed as
    /// `ParamValueEvent`s on the processor's input event list. Updating only
    /// the mirrored `params` made the host knobs look alive while the plugin
    /// never heard them.
    pending_params: Vec<(u32, f32)>,
    /// Whether the plugin declares an audio input port (used for wiring).
    has_audio_input: bool,
    /// Whether the plugin is a generator rather than a track effect.
    is_instrument: bool,
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

/// What the host should do to keep the editor and its window the same size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SizeAction {
    /// Both sides already agree.
    None,
    /// The plugin wants a different canvas: resize the host window.
    ResizeHost((u16, u16)),
    /// The user resized the host window: ask the plugin to re-layout.
    ResizePlugin((u16, u16)),
}

/// Decide how to reconcile the host window, the plugin's canvas and the size
/// they last agreed on.
///
/// - the plugin reporting a new size wins (it may re-layout on preset change);
/// - otherwise a changed host window is pushed to the plugin when it can
///   resize, and undone when it cannot (fixed-size editors would be clipped).
fn size_action(
    parent: Option<(u16, u16)>,
    reported: Option<(u16, u16)>,
    agreed: Option<(u16, u16)>,
    can_resize: bool,
) -> SizeAction {
    let (Some(parent), Some(reported)) = (parent, reported) else {
        return SizeAction::None;
    };
    match agreed {
        // First measurement: adopt the plugin's size.
        None if parent != reported => SizeAction::ResizeHost(reported),
        None => SizeAction::None,
        Some(agreed) if reported != agreed => SizeAction::ResizeHost(reported),
        Some(agreed) if parent != agreed => {
            if can_resize {
                SizeAction::ResizePlugin(parent)
            } else {
                SizeAction::ResizeHost(agreed)
            }
        }
        _ => SizeAction::None,
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
    /// Last size the plugin reported or that we pushed to it. Lets the host
    /// tell "the plugin changed size" from "the user resized our window".
    last_size: Option<(u16, u16)>,
    /// X11 window of the DAW, used as the floating window's transient parent.
    transient_for: Option<u64>,
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
            last_size: None,
            transient_for: None,
        }
    }

    /// Create the native window the plugin draws into.
    ///
    /// It is a *floating utility window*: transient for the DAW, kept above it
    /// and movable on its own, which is what users expect from a plugin editor
    /// and what a bare toplevel fails to be under Wayland/XWayland.
    #[allow(unused_variables)]
    fn create_parent(
        &mut self,
        width: u16,
        height: u16,
        transient_for: Option<u64>,
        resizable: bool,
    ) -> Result<(), String> {
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
            // Give the window an identity so window managers decorate, focus
            // and place it like a normal application window instead of an
            // anonymous override window.
            connection.send_request(&xcb::x::ChangeProperty {
                mode: xcb::x::PropMode::Replace,
                window,
                property: xcb::x::ATOM_WM_CLASS,
                r#type: xcb::x::ATOM_STRING,
                data: b"replicant-daw\0ReplicantDaw\0",
            });
            // Window-manager hints: a floating utility dialog that belongs to
            // the DAW window, stays above it and can be moved freely.
            let atom = |name: &[u8]| -> xcb::x::Atom {
                let cookie = connection.send_request(&xcb::x::InternAtom {
                    only_if_exists: false,
                    name,
                });
                connection
                    .wait_for_reply(cookie)
                    .map(|reply| reply.atom())
                    .unwrap_or(xcb::x::ATOM_NONE)
            };
            let window_type = atom(b"_NET_WM_WINDOW_TYPE");
            let utility = atom(b"_NET_WM_WINDOW_TYPE_UTILITY");
            if window_type != xcb::x::ATOM_NONE && utility != xcb::x::ATOM_NONE {
                connection.send_request(&xcb::x::ChangeProperty {
                    mode: xcb::x::PropMode::Replace,
                    window,
                    property: window_type,
                    r#type: xcb::x::ATOM_ATOM,
                    data: &[utility],
                });
            }
            let state = atom(b"_NET_WM_STATE");
            let above = atom(b"_NET_WM_STATE_ABOVE");
            if state != xcb::x::ATOM_NONE && above != xcb::x::ATOM_NONE {
                connection.send_request(&xcb::x::ChangeProperty {
                    mode: xcb::x::PropMode::Replace,
                    window,
                    property: state,
                    r#type: xcb::x::ATOM_ATOM,
                    data: &[above],
                });
            }
            if let Some(parent) = transient_for {
                connection.send_request(&xcb::x::ChangeProperty {
                    mode: xcb::x::PropMode::Replace,
                    window,
                    property: xcb::x::ATOM_WM_TRANSIENT_FOR,
                    r#type: xcb::x::ATOM_WINDOW,
                    data: &[parent as u32],
                });
            }

            // Size hints so the window manager honours the editor's size.
            // Flags: USPosition | USSize | PMinSize | PMaxSize (and PResizeInc
            // is left off), i.e. a fixed-size dialog when the plugin is fixed.
            let mut hints = [0u32; 18];
            hints[0] = if resizable { 0x33 } else { 0x3f };
            hints[3] = width as u32;
            hints[4] = height as u32;
            hints[5] = width as u32;
            hints[6] = height as u32;
            hints[7] = if resizable { 4096 } else { width as u32 };
            hints[8] = if resizable { 4096 } else { height as u32 };
            connection.send_request(&xcb::x::ChangeProperty {
                mode: xcb::x::PropMode::Replace,
                window,
                property: xcb::x::ATOM_WM_NORMAL_HINTS,
                r#type: xcb::x::ATOM_WM_SIZE_HINTS,
                data: &hints,
            });

            // Center it on the screen, and keep it fully visible when the editor
            // is smaller than the display.
            let screen_w = screen.width_in_pixels();
            let screen_h = screen.height_in_pixels();
            let x = ((screen_w as i32 - width as i32) / 2).max(0);
            let y = ((screen_h as i32 - height as i32) / 2).max(0);
            connection.send_request(&xcb::x::ConfigureWindow {
                window,
                value_list: &[xcb::x::ConfigWindow::X(x), xcb::x::ConfigWindow::Y(y)],
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

    /// Map (show) the parent window, raise it and give it keyboard focus.
    fn map_parent(&self) {
        #[cfg(target_os = "linux")]
        if let (Some(connection), Some(window)) = (&self.connection, self.window) {
            connection.send_request(&xcb::x::MapWindow { window });
            connection.send_request(&xcb::x::ConfigureWindow {
                window,
                value_list: &[xcb::x::ConfigWindow::StackMode(xcb::x::StackMode::Above)],
            });
            connection.send_request(&xcb::x::SetInputFocus {
                revert_to: xcb::x::InputFocus::Parent,
                focus: window,
                time: xcb::x::CURRENT_TIME,
            });
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
    fn resize_parent(&mut self, width: u16, height: u16) {
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

    /// Current size of the host window, in pixels.
    fn parent_size(&self) -> Option<(u16, u16)> {
        #[cfg(target_os = "linux")]
        {
            let connection = self.connection.as_ref()?;
            let window = self.window?;
            let cookie = connection.send_request(&xcb::x::GetGeometry {
                drawable: xcb::x::Drawable::Window(window),
            });
            let reply = connection.wait_for_reply(cookie).ok()?;
            Some((reply.width(), reply.height()))
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
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
        // The descriptor's feature tags are the authoritative generator/effect
        // signal: many instruments (samplers with an audio input, for one)
        // expose an input bus and would otherwise look like effects.
        let features: Vec<String> = first
            .features()
            .map(|feature| feature.to_string_lossy().to_lowercase())
            .collect();
        let tagged_instrument = features
            .iter()
            .any(|feature| matches!(feature.as_str(), "instrument" | "synthesizer" | "sampler"));
        let tagged_effect = features.iter().any(|feature| {
            matches!(
                feature.as_str(),
                "audio-effect" | "note-effect" | "analyzer"
            )
        });

        let host_info = HostInfo::new(
            "Replicant DAW",
            "Replicant",
            "https://github.com/DanimanX",
            "1.0",
        )
        .map_err(|error| format!("CLAP host info failed: {error}"))?;
        let mut instance = PluginInstance::<ReplicantHost>::new(
            |_| HostShared::default(),
            |shared| HostMainThread {
                reactor: std::sync::Arc::clone(&shared.reactor),
                _lifetime: std::marker::PhantomData,
            },
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
        // Features win; the bus layout is the fallback for untagged plugins.
        let is_instrument = if tagged_instrument {
            true
        } else if tagged_effect {
            false
        } else {
            !has_audio_input
        };

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
            pending_params: Vec::new(),
            has_audio_input,
            is_instrument,
            sample_rate,
            gui: None,
            gui_supported,
        };
        this.activate()?;
        Ok(this)
    }

    /// Ask the plugin whether it exposes a GUI extension.
    ///
    /// Only the extension is probed, never the size: CLAP only allows `get_size`
    /// after `create`, and u-he plugins abort the process when it is called
    /// earlier. The real negotiation happens in [`ClapInstance::open_gui`].
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
        self.is_instrument
    }

    /// Re-read every parameter from the plugin.
    ///
    /// The cached list is only a mirror: a plugin can change its own values from
    /// its editor, so the host refreshes before displaying or saving them.
    pub fn refresh_params(&mut self) {
        self.params = Self::read_params(&mut self.instance);
    }

    pub fn set_param(&mut self, id: u32, value: f32) {
        let value = value.clamp(0.0, 1.0);
        if let Some(param) = self.params.iter_mut().find(|param| param.id == id) {
            param.value = value;
            param.text = format!("{:.2}", param.value);
        }
        // Queue the change for the plugin itself. Repeated edits of the same
        // parameter collapse into the last value, so dragging a knob cannot
        // overflow the event list.
        if let Some(entry) = self
            .pending_params
            .iter_mut()
            .find(|(pending, _)| *pending == id)
        {
            entry.1 = value;
        } else {
            self.pending_params.push((id, value));
        }
    }

    /// Drop automation older than the current block, keeping the event list
    /// inside its fixed capacity.
    fn queue_pending_params(&mut self) {
        if self.pending_params.len() > super::MAX_BLOCK_EVENTS / 2 {
            let keep = super::MAX_BLOCK_EVENTS / 2;
            let excess = self.pending_params.len() - keep;
            self.pending_params.drain(..excess);
        }
        let Self {
            events,
            pending_params,
            ..
        } = self;
        for (id, value) in pending_params.drain(..).take(super::MAX_BLOCK_EVENTS / 2) {
            events.push(
                &ParamValueEvent::new(0, ClapId::new(id), Pckn::match_all(), value as f64)
                    .as_unknown(),
            );
        }
    }

    /// Does this plugin expose a GUI at all on the current platform?
    pub fn has_gui(&self) -> bool {
        self.gui_supported
    }

    pub fn is_gui_open(&self) -> bool {
        self.gui.as_ref().is_some_and(ClapGui::is_open)
    }

    /// Create the plugin's editor window and show it.
    pub fn open_gui(&mut self, transient_for: Option<u64>) -> Result<(), String> {
        if self.is_gui_open() {
            return Ok(());
        }
        self.destroy_gui();

        let api = preferred_gui_api();
        let mut gui = ClapGui::new(format!("{} — {}", self.name, self.vendor));
        gui.transient_for = transient_for;

        let embedded = GuiConfiguration {
            api_type: api,
            is_floating: false,
        };
        let floating = GuiConfiguration {
            api_type: api,
            is_floating: true,
        };

        // Negotiate the mode with `is_api_supported` only. Sizing must wait:
        // CLAP forbids calling `size` before `create`, and plugins that check
        // it (u-he prints a warning and then aborts) treat the violation as
        // fatal, so no `get_size` may happen here.
        let use_floating = {
            let handle = self.instance.plugin_handle();
            let gui_ext = handle
                .get_extension::<PluginGui>()
                .ok_or_else(|| format!("{} exposes no CLAP GUI extension", self.name))?;
            // Prefer the plugin's own floating window when it supports one: it
            // is a real OS window the user can move and stack, which is what
            // "floating" means in every DAW. Embedded editors get a host window
            // configured as a floating utility instead.
            if gui_ext.is_api_supported(&handle, floating) {
                true
            } else if gui_ext.is_api_supported(&handle, embedded) {
                false
            } else {
                return Err(format!(
                    "{} does not support a {} GUI in this build",
                    self.name,
                    api.0.to_string_lossy()
                ));
            }
        };

        // One handle for the whole sequence: `create` → (embed) → `size` → `show`.
        let handle = self.instance.plugin_handle();
        let gui_ext = handle
            .get_extension::<PluginGui>()
            .ok_or_else(|| format!("{} exposes no CLAP GUI extension", self.name))?;
        let configuration = if use_floating { floating } else { embedded };

        fn fail(gui: &mut ClapGui, gui_ext: &PluginGui, handle: &PluginMainThreadHandle<'_>) {
            gui_ext.destroy(handle);
            gui.created = false;
            gui.teardown();
        }

        if let Err(error) = gui_ext.create(&handle, configuration) {
            return Err(format!("CLAP GUI create failed: {error:?}"));
        }
        gui.created = true;

        if use_floating {
            gui.floating = true;
            let title = std::ffi::CString::new(gui.title.clone()).unwrap_or_default();
            gui_ext.suggest_title(&handle, &title);
        } else {
            // Ask for the size *after* create and build the parent at exactly
            // that size: a host window smaller than the plugin's canvas is what
            // makes the editor look cut off.
            let reported = gui_ext.get_size(&handle);
            let width = reported
                .map(|size| size.width)
                .unwrap_or(900)
                .clamp(160, 4096) as u16;
            let height = reported
                .map(|size| size.height)
                .unwrap_or(620)
                .clamp(120, 4096) as u16;
            let resizable = gui_ext.can_resize(&handle);
            if let Err(error) = gui.create_parent(width, height, transient_for, resizable) {
                fail(&mut gui, &gui_ext, &handle);
                return Err(error);
            }
            gui.last_size = Some((width, height));
            // Some toolkits query the parent's attributes while attaching, so
            // it must be a real, mapped window before `set_parent`.
            gui.map_parent();

            let parent = gui.parent_handle()?;
            // SAFETY: the X11 window lives in `gui` until after `destroy`.
            if let Err(error) = unsafe { gui_ext.set_parent(&handle, parent) } {
                fail(&mut gui, &gui_ext, &handle);
                return Err(format!("CLAP GUI set_parent failed: {error:?}"));
            }
        }

        if let Err(error) = gui_ext.show(&handle) {
            fail(&mut gui, &gui_ext, &handle);
            return Err(format!("CLAP GUI show failed: {error:?}"));
        }

        self.gui = Some(gui);
        Ok(())
    }

    /// Drive what the plugin registered with the host: its timers and the file
    /// descriptors it wants to watch.
    ///
    /// GUI toolkits on Linux run their editor loop from these two callbacks, so
    /// a plugin whose editor is drawn but ignores every click is almost always
    /// missing them.
    pub fn service_host_events(&mut self) {
        let reactor = self
            .instance
            .access_shared_handler(|shared| std::sync::Arc::clone(&shared.reactor));
        let due = reactor.due_timers();
        #[cfg(unix)]
        let ready = reactor.ready_fds();
        #[cfg(not(unix))]
        let ready: Vec<()> = Vec::new();
        if due.is_empty() && ready.is_empty() {
            return;
        }

        let handle = self.instance.plugin_handle();
        if !due.is_empty() {
            if let Some(timers) = handle.get_extension::<PluginTimer>() {
                for id in due {
                    timers.on_timer(&handle, id);
                }
            }
        }
        #[cfg(unix)]
        if !ready.is_empty() {
            if let Some(fds) = handle.get_extension::<PluginPosixFd>() {
                for (fd, flags) in ready {
                    fds.on_fd(&handle, fd, flags);
                }
            }
        }
    }

    /// Answer the requests the plugin made through `clap_host_gui`.
    ///
    /// Runs on the worker thread while the editor is open: a resize request
    /// grows/shrinks our parent window, `hide` unmaps it and a destroyed
    /// window tears the editor down.
    pub fn service_gui(&mut self) {
        // Timers and file descriptors must run even before a GUI exists: some
        // plugins drive preset loading and meters from them.
        self.service_host_events();

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
            if let Some(gui) = self.gui.as_mut() {
                gui.resize_parent(
                    width.clamp(160, 4096) as u16,
                    height.clamp(120, 4096) as u16,
                );
            }
        }

        // Follow the plugin's own size, and let it follow ours when the user
        // resizes the window: an editor that reports a size bigger than the
        // host window is what makes it look cut off.
        self.sync_gui_size();
        if hide {
            if let Some(gui) = self.gui.as_ref() {
                gui.unmap_parent();
            }
        }
        if closed {
            self.destroy_gui();
        }
    }

    /// Keep the host window and the plugin's canvas the same size.
    ///
    /// Two directions, decided by comparing the plugin's reported size with the
    /// last size both sides agreed on:
    /// - the plugin changed size → grow/shrink the host window to fit;
    /// - the host window changed (the user dragged its border) → tell the
    ///   plugin to lay out at the new size.
    fn sync_gui_size(&mut self) {
        let Some(gui) = self.gui.as_ref() else {
            return;
        };
        if !gui.created || gui.floating {
            return;
        }
        let parent = gui.parent_size();
        let handle = self.instance.plugin_handle();
        let Some(gui_ext) = handle.get_extension::<PluginGui>() else {
            return;
        };
        let reported = gui_ext.get_size(&handle).map(|size| {
            (
                size.width.clamp(160, 4096) as u16,
                size.height.clamp(120, 4096) as u16,
            )
        });

        let action = size_action(parent, reported, gui.last_size, gui_ext.can_resize(&handle));
        match action {
            SizeAction::None => {}
            SizeAction::ResizeHost(size) => {
                if let Some(gui) = self.gui.as_mut() {
                    gui.resize_parent(size.0, size.1);
                }
            }
            SizeAction::ResizePlugin(size) => {
                let wanted = clack_extensions::gui::GuiSize {
                    width: size.0 as u32,
                    height: size.1 as u32,
                };
                if gui_ext.set_size(&handle, wanted).is_ok() {
                    if let Some(gui) = self.gui.as_mut() {
                        gui.last_size = Some(size);
                    }
                } else if let Some(gui) = self.gui.as_mut() {
                    // A fixed-size editor that refused the request: undo.
                    if let Some(previous) = gui.last_size {
                        gui.resize_parent(previous.0, previous.1);
                    }
                }
            }
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
        // Build the event list for this block. Host parameter changes go
        // first, so a knob edit lands at the same sample as any note it
        // accompanies. This runs before the processor borrow, because both
        // borrow `self`.
        self.events.clear();
        self.queue_pending_params();
        let ClapInstance {
            processor: Some(processor),
            ..
        } = self
        else {
            return Err("CLAP processor is not active".into());
        };
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
                    // Control changes are MIDI, not parameter automation.
                    // Mapping them onto parameters by index meant CC 120/123
                    // (All Sound Off / All Notes Off) poked whichever knob sat
                    // at that index and never reached the plugin's voice
                    // allocator, so a held note survived Stop.
                    self.events.push(
                        &MidiEvent::new(event.offset as u32, 0, [bytes[0], bytes[1], bytes[2]])
                            .as_unknown(),
                    );
                }
                0xe0 => {
                    self.events.push(
                        &MidiEvent::new(event.offset as u32, 0, [bytes[0], bytes[1], bytes[2]])
                            .as_unknown(),
                    );
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

#[cfg(test)]
mod tests {
    /// CLAP requires `create` before any sizing call; u-he plugins abort the
    /// process otherwise ("clap_plugin_gui.size() was called without a prior
    /// call to clap_plugin_gui.create()"). Guard the call order in the source
    /// so a future refactor cannot reintroduce it.
    #[test]
    fn gui_sizing_never_precedes_create() {
        // Only the non-test half: this file contains its own search strings.
        let source = include_str!("clap_host.rs");
        let source = &source[..source.find("#[cfg(test)]").unwrap_or(source.len())];
        let create = source
            .find("gui_ext.create(&handle, configuration)")
            .expect("create call not found");
        for call in ["gui_ext.get_size(&handle)", "gui_ext.set_size("] {
            let mut cursor = 0;
            while let Some(offset) = source[cursor..].find(call) {
                let position = cursor + offset;
                assert!(
                    position > create,
                    "{call} appears before `create`; CLAP forbids sizing first"
                );
                cursor = position + call.len();
            }
        }
        assert!(
            source.matches("gui_ext.get_size(&handle)").count() >= 1,
            "the host must size the window from the plugin's reported size"
        );
    }

    #[test]
    fn sizing_follows_the_plugin_and_respects_fixed_size_editors() {
        use super::{size_action, SizeAction};
        let plugin = Some((866, 674));
        let bigger = Some((1000, 700));

        // First measurement adopts the plugin's canvas.
        assert_eq!(
            size_action(Some((900, 620)), plugin, None, false),
            SizeAction::ResizeHost((866, 674))
        );
        // Agreement is stable.
        assert_eq!(size_action(plugin, plugin, plugin, true), SizeAction::None);
        // The plugin re-laid out: follow it, even if the user resized earlier.
        assert_eq!(
            size_action(bigger, Some((1200, 680)), plugin, true),
            SizeAction::ResizeHost((1200, 680))
        );
        // The user resized a resizable editor: push the new size down.
        assert_eq!(
            size_action(bigger, plugin, plugin, true),
            SizeAction::ResizePlugin((1000, 700))
        );
        // The user resized a fixed-size editor: snap the window back.
        assert_eq!(
            size_action(bigger, plugin, plugin, false),
            SizeAction::ResizeHost((866, 674))
        );
        // Nothing known yet: leave it alone.
        assert_eq!(size_action(None, None, None, true), SizeAction::None);
    }

    #[test]
    fn reactor_schedules_and_reschedules_timers() {
        use clack_extensions::timer::HostTimerImpl;

        let reactor = std::sync::Arc::new(super::HostReactor::default());
        let main_thread = super::HostMainThread {
            reactor: std::sync::Arc::clone(&reactor),
            _lifetime: std::marker::PhantomData,
        };
        let id = main_thread.register_timer(1).expect("register timer");
        // Nothing is due immediately after registration…
        assert!(reactor.due_timers().is_empty());
        std::thread::sleep(std::time::Duration::from_millis(3));
        // …but it fires once the period elapsed, and then reschedules itself.
        assert_eq!(reactor.due_timers(), vec![id]);
        assert!(reactor.due_timers().is_empty());
        main_thread.unregister_timer(id).expect("unregister");
        std::thread::sleep(std::time::Duration::from_millis(3));
        assert!(reactor.due_timers().is_empty());
    }

    #[test]
    fn preferred_api_matches_the_platform() {
        let api = super::preferred_gui_api();
        #[cfg(target_os = "linux")]
        assert_eq!(api.0.to_string_lossy(), "x11");
        #[cfg(target_os = "windows")]
        assert_eq!(api.0.to_string_lossy(), "win32");
        #[cfg(target_os = "macos")]
        assert_eq!(api.0.to_string_lossy(), "cocoa");
    }
}
