//! Audio plugin hosting: CLAP and VST3 in-process, VST2 discovery only.
//!
//! Plugins run on a dedicated [`worker::PluginWorker`] thread. The audio
//! engine exchanges fixed-size stereo blocks with that thread through
//! lock-free `rtrb` rings, which keeps `!Send` plugin handles out of the
//! audio callback and gives the UI a single place to load, unload and
//! automate plugins.

pub mod clap_host;
pub mod scanner;
pub mod vst3_host;
pub mod worker;

use std::fmt;
use std::path::PathBuf;

pub use scanner::{scan_all, scan_paths, PluginDescriptor};
pub use worker::{PluginCommand, PluginEvent, PluginRuntime, PluginSlotInfo};

/// A plugin parameter as shown in the rack.
#[derive(Clone, Debug, Default)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    /// Normalized value in `0.0..=1.0`.
    pub value: f32,
    pub text: String,
}

/// Maximum number of insert slots on the master bus.
pub const MAX_PLUGIN_SLOTS: usize = 4;
/// Frames exchanged per plugin block.
pub const PLUGIN_BLOCK_FRAMES: usize = 512;
/// Maximum MIDI events attached to a single block.
pub const MAX_BLOCK_EVENTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginFormat {
    Clap,
    Vst3,
    Vst2,
}

impl PluginFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Clap => "CLAP",
            Self::Vst3 => "VST3",
            Self::Vst2 => "VST2",
        }
    }

    /// Formats this build can host in-process.
    pub fn is_hostable(self) -> bool {
        matches!(self, Self::Clap | Self::Vst3)
    }
}

impl fmt::Display for PluginFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// One MIDI event inside an audio block.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockEvent {
    /// Sample offset inside the block.
    pub offset: u16,
    pub bytes: [u8; 3],
    pub len: u8,
}

/// Stereo audio block exchanged with the plugin worker.
#[derive(Clone, Copy)]
pub struct PluginBlock {
    pub frames: u16,
    pub left: [f32; PLUGIN_BLOCK_FRAMES],
    pub right: [f32; PLUGIN_BLOCK_FRAMES],
    pub events: [BlockEvent; MAX_BLOCK_EVENTS],
    pub event_count: u8,
}

impl Default for PluginBlock {
    fn default() -> Self {
        Self {
            frames: 0,
            left: [0.0; PLUGIN_BLOCK_FRAMES],
            right: [0.0; PLUGIN_BLOCK_FRAMES],
            events: [BlockEvent::default(); MAX_BLOCK_EVENTS],
            event_count: 0,
        }
    }
}

impl PluginBlock {
    pub fn push_event(&mut self, offset: u16, bytes: [u8; 3], len: u8) {
        let index = self.event_count as usize;
        if index < MAX_BLOCK_EVENTS {
            self.events[index] = BlockEvent { offset, bytes, len };
            self.event_count += 1;
        }
    }

    pub fn clear_events(&mut self) {
        self.event_count = 0;
    }
}

impl fmt::Debug for PluginBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginBlock")
            .field("frames", &self.frames)
            .field("events", &self.event_count)
            .finish()
    }
}

/// A loaded plugin slot as seen by the UI.
#[derive(Clone, Debug, Default)]
pub struct SlotState {
    pub info: Option<PluginSlotInfo>,
    pub pending: bool,
}

/// Where plugins live on this platform.
pub fn default_plugin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);

    #[cfg(target_os = "windows")]
    {
        let common = std::env::var_os("COMMONPROGRAMFILES").map(PathBuf::from);
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let program = std::env::var_os("PROGRAMFILES").map(PathBuf::from);
        if let Some(local) = &local {
            dirs.push(local.join("Programs/Common/CLAP"));
            dirs.push(local.join("Programs/Common/VST3"));
        }
        if let Some(common) = &common {
            dirs.push(common.join("CLAP"));
            dirs.push(common.join("VST3"));
            dirs.push(common.join("VST2"));
        }
        if let Some(program) = &program {
            dirs.push(program.join("VSTPlugins"));
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = &home {
            dirs.push(home.join("Library/Audio/Plug-Ins/CLAP"));
            dirs.push(home.join("Library/Audio/Plug-Ins/VST3"));
            dirs.push(home.join("Library/Audio/Plug-Ins/VST"));
        }
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/VST"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(home) = &home {
            dirs.push(home.join(".clap"));
            dirs.push(home.join(".vst3"));
            dirs.push(home.join(".vst"));
        }
        dirs.push(PathBuf::from("/usr/lib/clap"));
        dirs.push(PathBuf::from("/usr/local/lib/clap"));
        dirs.push(PathBuf::from("/usr/lib/vst3"));
        dirs.push(PathBuf::from("/usr/local/lib/vst3"));
    }

    // An explicit override is useful for CI and for plugin collections that
    // live outside the platform defaults.
    if let Some(extra) = std::env::var_os("REPLICANT_DAW_PLUGIN_PATH") {
        for part in std::env::split_paths(&extra) {
            dirs.push(part);
        }
    }

    dirs.retain(|dir| dir.is_dir());
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_event_capacity_is_bounded() {
        let mut block = PluginBlock::default();
        for i in 0..(MAX_BLOCK_EVENTS + 10) {
            block.push_event(i as u16, [0x90, 60, 100], 3);
        }
        assert_eq!(block.event_count as usize, MAX_BLOCK_EVENTS);
        block.clear_events();
        assert_eq!(block.event_count, 0);
    }

    #[test]
    fn formats_report_hostability() {
        assert!(PluginFormat::Clap.is_hostable());
        assert!(PluginFormat::Vst3.is_hostable());
        assert!(!PluginFormat::Vst2.is_hostable());
        assert_eq!(PluginFormat::Vst3.label(), "VST3");
    }
}
