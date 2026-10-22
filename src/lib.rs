//! Replicant DAW - a cyberpunk DAW prototype.
//!
//! The crate is split into a UI layer (`app`, `ui`, `theme`) and a
//! real-time audio layer (`audio`, `dsp`). Both halves meet only at lock-free
//! `rtrb` queues.

pub mod app;
pub mod audio;
pub mod dsp;
pub mod state;
pub mod theme;
pub mod ui;

pub const APP_NAME: &str = "Replicant DAW";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
