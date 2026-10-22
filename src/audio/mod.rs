//! Audio subsystem: messages, sequencing, synthesis, engine and host I/O.

pub mod engine;
pub mod messages;
pub mod midi;
pub mod stream;
pub mod synth;
pub mod tracks;

pub use engine::{Engine, EngineIo, DEFAULT_BPM, LOOP_BARS, MAX_VOICES, SAMPLE_RATE};
pub use messages::{
    db_to_gain, gain_to_db, Command, FxParam, RecordEvent, RecordedClip, RecordedNote, Telemetry,
    LOOP_STEPS, MAX_RECORDED_NOTES, SCENE_COUNT, STEPS_PER_BAR, TRACK_COUNT,
};
pub use midi::{MidiMessage, MidiRuntime};
pub use stream::AudioRuntime;
pub use tracks::{clip_density, note_at, TrackSpec, DEFAULT_SCENES, SCENE_NAMES, TRACKS};
