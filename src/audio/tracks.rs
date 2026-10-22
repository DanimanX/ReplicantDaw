//! Static track definitions and the generative clip patterns used by the demo set.

use super::messages::{SCENE_COUNT, TRACK_COUNT};

pub const SCENE_NAMES: [&str; SCENE_COUNT] = [
    "INTRO", "PULSE", "DRIFT", "ASCEND", "VOID", "SURGE", "ECHO", "OUTRO",
];

/// A voice / mixer-strip description that stays constant for the session.
#[derive(Clone, Copy, Debug)]
pub struct TrackSpec {
    pub name: &'static str,
    /// UI accent colour (RGB).
    pub accent: [u8; 3],
    /// MIDI note the pattern offsets are relative to.
    pub base_note: u8,
    /// FM modulator : carrier frequency ratio.
    pub fm_ratio: f32,
    /// FM modulation index.
    pub mod_index: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    /// Fraction of a 16th-note step that the envelope gate stays open.
    pub gate: f32,
    /// Default fader position.
    pub gain_db: f32,
    /// Default stereo position, -1.0 = left, 1.0 = right.
    pub pan: f32,
    /// Per-voice amplitude trim.
    pub level: f32,
}

pub const TRACKS: [TrackSpec; TRACK_COUNT] = [
    TrackSpec {
        name: "SYNAPSE 01",
        accent: [0, 229, 255],
        base_note: 64,
        fm_ratio: 1.0,
        mod_index: 1.15,
        attack: 0.006,
        decay: 0.22,
        sustain: 0.30,
        release: 0.22,
        gate: 0.55,
        gain_db: -7.0,
        pan: -0.12,
        level: 0.90,
    },
    TrackSpec {
        name: "BIOLINK",
        accent: [47, 139, 255],
        base_note: 60,
        fm_ratio: 2.0,
        mod_index: 0.85,
        attack: 0.002,
        decay: 0.28,
        sustain: 0.08,
        release: 0.30,
        gate: 0.45,
        gain_db: -10.0,
        pan: 0.22,
        level: 0.85,
    },
    TrackSpec {
        name: "NEURAL BASS",
        accent: [164, 92, 255],
        base_note: 33,
        fm_ratio: 1.0,
        mod_index: 1.45,
        attack: 0.004,
        decay: 0.20,
        sustain: 0.42,
        release: 0.14,
        gate: 0.62,
        gain_db: -5.0,
        pan: 0.0,
        level: 1.0,
    },
    TrackSpec {
        name: "SUB-QUANT 04",
        accent: [255, 138, 61],
        base_note: 33,
        fm_ratio: 0.5,
        mod_index: 0.25,
        attack: 0.008,
        decay: 0.35,
        sustain: 0.55,
        release: 0.18,
        gate: 0.8,
        gain_db: -4.0,
        pan: 0.0,
        level: 1.0,
    },
    TrackSpec {
        name: "AI VOICELET",
        accent: [255, 79, 216],
        base_note: 57,
        fm_ratio: 1.5,
        mod_index: 3.1,
        attack: 0.035,
        decay: 0.40,
        sustain: 0.5,
        release: 0.45,
        gate: 0.78,
        gain_db: -12.0,
        pan: 0.28,
        level: 0.75,
    },
    TrackSpec {
        name: "QUANTUM PAD",
        accent: [39, 224, 192],
        base_note: 45,
        fm_ratio: 1.0,
        mod_index: 0.65,
        attack: 0.45,
        decay: 1.10,
        sustain: 0.75,
        release: 1.30,
        gate: 0.98,
        gain_db: -14.0,
        pan: -0.35,
        level: 0.60,
    },
    TrackSpec {
        name: "GHOSTWIRE",
        accent: [57, 255, 136],
        base_note: 76,
        fm_ratio: 3.5,
        mod_index: 2.0,
        attack: 0.001,
        decay: 0.075,
        sustain: 0.0,
        release: 0.07,
        gate: 0.20,
        gain_db: -15.0,
        pan: 0.32,
        level: 0.55,
    },
    TrackSpec {
        name: "MIRROR CORE",
        accent: [224, 36, 195],
        base_note: 72,
        fm_ratio: 2.0,
        mod_index: 1.1,
        attack: 0.002,
        decay: 0.50,
        sustain: 0.08,
        release: 0.65,
        gate: 0.35,
        gain_db: -13.0,
        pan: 0.12,
        level: 0.65,
    },
];

/// Clip that is armed when the application starts (one per track).
pub const DEFAULT_SCENES: [usize; TRACK_COUNT] = [1, 1, 1, 0, 2, 1, 2, 1];

const REST: i8 = i8::MIN;

/// Sixteen-step motifs, in semitones relative to the track root.
/// A-minor flavoured material that sits well together across scenes.
const MOTIFS: [[i8; 16]; TRACK_COUNT] = [
    // Synapse 01 - plucked lead arpeggio
    [
        0, REST, 7, REST, 3, REST, 12, REST, 0, REST, 7, REST, 10, REST, 7, REST,
    ],
    // Biolink - sparse upper pluck
    [
        0, REST, REST, 7, REST, 12, REST, REST, 3, REST, REST, 10, REST, 7, REST, REST,
    ],
    // Neural Bass - driving eighths
    [
        0, REST, 0, REST, 7, REST, 0, REST, 0, REST, 3, REST, 7, REST, 10, REST,
    ],
    // Sub-Quant - long sub pulses
    [
        0, REST, REST, REST, -12, REST, REST, REST, 0, REST, REST, REST, 3, REST, REST, REST,
    ],
    // AI Voicelet - syncopated vocal stabs
    [
        REST, REST, 0, REST, REST, 3, REST, REST, REST, REST, 7, REST, REST, 3, REST, REST,
    ],
    // Quantum Pad - dyads held across the bar
    [
        0, REST, REST, REST, 3, REST, REST, REST, 7, REST, REST, REST, 5, REST, REST, REST,
    ],
    // Ghostwire - high FM percussion
    [
        0, REST, 12, REST, 7, REST, 12, REST, 0, REST, 12, REST, 7, REST, 3, REST,
    ],
    // Mirror Core - bell cascade
    [0, 7, 12, 15, 12, 7, 3, 7, 0, 7, 12, 19, 15, 12, 7, 3],
];

const TRANSPOSE: [i8; SCENE_COUNT] = [0, 0, 5, 3, 7, -2, 10, -5];
const OCTAVE: [i8; SCENE_COUNT] = [0, 0, 0, 1, 0, -1, 1, 0];

/// The note (if any) for a track/scene at a bar-relative step.
///
/// The patterns are deterministic so the UI can draw exactly what the
/// sequencer will play.
pub fn note_at(track: usize, scene: usize, step: usize) -> Option<u8> {
    let track = track.min(TRACK_COUNT - 1);
    let scene = scene.min(SCENE_COUNT - 1);
    let step = step % 16;

    // Rotate the motif per scene so every clip has its own rhythmic identity.
    let rotated = (step + scene * 3) % 16;
    let offset = MOTIFS[track][rotated];
    if offset == REST {
        return None;
    }

    // Thin out the intro and outro scenes. Use the rotated position so
    // sparse scenes still land on motif notes.
    let keep = match scene {
        0 => rotated % 2 == 0,
        7 => rotated % 2 == 0,
        _ => true,
    };
    if !keep {
        return None;
    }

    let note = TRACKS[track].base_note as i16
        + offset as i16
        + TRANSPOSE[scene] as i16
        + OCTAVE[scene] as i16 * 12;
    Some(note.clamp(0, 127) as u8)
}

/// How many notes a clip contains (used for UI density hints).
pub fn clip_density(track: usize, scene: usize) -> usize {
    (0..16)
        .filter(|&step| note_at(track, scene, step).is_some())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_are_in_midi_range() {
        for track in 0..TRACK_COUNT {
            for scene in 0..SCENE_COUNT {
                for step in 0..16 {
                    if let Some(note) = note_at(track, scene, step) {
                        assert!(note < 128);
                    }
                }
            }
        }
    }

    #[test]
    fn every_clip_has_notes() {
        for track in 0..TRACK_COUNT {
            for scene in 0..SCENE_COUNT {
                assert!(
                    clip_density(track, scene) > 0,
                    "clip {track}/{scene} is silent"
                );
            }
        }
    }

    #[test]
    fn scene_rotation_changes_patterns() {
        let a: Vec<_> = (0..16).map(|s| note_at(0, 1, s)).collect();
        let b: Vec<_> = (0..16).map(|s| note_at(0, 4, s)).collect();
        assert_ne!(a, b);
    }
}
