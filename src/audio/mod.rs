//! Audio subsystem: messages, sequencing, synthesis, engine and host I/O.

pub mod engine;
pub mod messages;
pub mod stream;
pub mod synth;
pub mod tracks;

pub use engine::{Engine, DEFAULT_BPM, LOOP_BARS, MAX_VOICES, SAMPLE_RATE};
pub use messages::{
    db_to_gain, gain_to_db, Command, FxParam, Telemetry, SCENE_COUNT, STEPS_PER_BAR, TRACK_COUNT,
};
pub use stream::AudioRuntime;
pub use tracks::{clip_density, note_at, TrackSpec, DEFAULT_SCENES, SCENE_NAMES, TRACKS};
