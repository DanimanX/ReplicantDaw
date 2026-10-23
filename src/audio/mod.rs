//! Audio subsystem: messages, sequencing, synthesis, engine and host I/O.

pub mod effects;
pub mod engine;
pub mod messages;
pub mod midi;
pub mod stream;
/// The retired built-in synth stays compiled only for engine unit tests.
/// Release builds contain no synth code; every track must host a plugin.
#[cfg(test)]
pub mod synth_xt;
pub mod tracks;

pub use effects::EffectSlot;
pub use engine::{Engine, EngineIo, DEFAULT_BPM, LOOP_BARS, MAX_VOICES, SAMPLE_RATE};
pub use messages::{
    db_to_gain, gain_to_db, ArrangementClip, Command, EffectKind, EffectKnob, RecordEvent,
    RecordedClip, RecordedNote, SampleBuffer, SampleUpload, Telemetry, LOOP_STEPS, MAX_EFFECTS,
    MAX_RECORDED_NOTES, PREVIEW_TRACK, SCENE_COUNT, STEPS_PER_BAR, TRACK_COUNT,
};
pub use midi::{MidiMessage, MidiRuntime};
pub use stream::AudioRuntime;
#[cfg(test)]
pub use synth_xt::{
    EnvParams, FilterKind, FilterParams, LfoParams, LfoShape, ModDest, ModRoute, ModSource,
    OscParams, Stage, SynthParam, SynthXt, Voice, Waveform, ENV_COUNT, FILTER_COUNT, LFO_COUNT,
    MOD_SLOTS, OSC_COUNT, PRESETS,
};
pub use tracks::{clip_density, note_at, TrackSpec, DEFAULT_SCENES, SCENE_NAMES, TRACKS};
