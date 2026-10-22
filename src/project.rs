//! Project persistence: a JSON document describing the session.
//!
//! Recorded MIDI clips are stored as note lists, plugin state as opaque
//! base64 blobs, and the mix as plain numbers, so a project file is
//! human-inspectable and versionable.
//!
//! Format history:
//! - v1: fixed master FX block (`fx`), generative clips only.
//! - v2: modular effect chain, per-track synth parameters, MIDI clip state.
//!   v1 files still load: the legacy `fx` block is translated into a chain.
//! - v3: per-track plugin inserts and audio-clip envelopes. Older files load
//!   with neutral defaults (unity clip gain, no fades).
//! - v4: one MIDI clip per session slot and user-editable track names. v3
//!   `notes` are migrated into the slot named by the track's `scene`.
//! - v5: the instrument slot is explicit (`empty` / `synth` / `plugin`).
//!   v4 files map their `instrument_plugin` flag onto it and keep the synth
//!   audible, because that was the only source back then.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::audio::messages::{EffectKind, RecordedNote, SynthParam, TRACK_COUNT};
use crate::audio::{SynthParams, TRACKS};

pub const PROJECT_EXTENSION: &str = "rproj";
pub const PROJECT_FORMAT_VERSION: u32 = 6;

/// Editable FM synth settings stored per track.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectSynth {
    pub ratio: f32,
    pub index: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub level: f32,
}

impl ProjectSynth {
    pub fn from_params(params: &SynthParams) -> Self {
        Self {
            ratio: params.fm_ratio,
            index: params.mod_index,
            attack: params.attack,
            decay: params.decay,
            sustain: params.sustain,
            release: params.release,
            level: params.level,
        }
    }

    pub fn default_for_track(index: usize) -> Self {
        let spec = &TRACKS[index.min(TRACK_COUNT - 1)];
        Self::from_params(&SynthParams::from_spec(spec))
    }

    /// Convert to engine values, clamping anything out of range.
    pub fn to_params(&self) -> SynthParams {
        let mut params = SynthParams::from_spec(&TRACKS[0]);
        params.set(SynthParam::Ratio, self.ratio);
        params.set(SynthParam::Index, self.index);
        params.set(SynthParam::Attack, self.attack);
        params.set(SynthParam::Decay, self.decay);
        params.set(SynthParam::Sustain, self.sustain);
        params.set(SynthParam::Release, self.release);
        params.set(SynthParam::Level, self.level);
        params
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectEffect {
    /// Stable module id, e.g. `DRIVE`, `FILTER`, `TONE`, `WIDTH`, `DELAY`.
    pub kind: String,
    #[serde(default)]
    pub params: [f32; 3],
    #[serde(default)]
    pub bypass: bool,
}

impl ProjectEffect {
    pub fn new(kind: EffectKind) -> Self {
        Self {
            kind: kind.id().to_string(),
            params: kind.defaults(),
            bypass: false,
        }
    }

    pub fn effect_kind(&self) -> Option<EffectKind> {
        EffectKind::from_id(&self.kind)
    }
}

/// One session slot's MIDI clip.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProjectClip {
    /// Session scene index (0-7).
    pub scene: u8,
    #[serde(default)]
    pub notes: Vec<RecordedNote>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Loop region in 16th notes. Absent in v5 files, which looped everything.
    #[serde(default)]
    pub loop_start: Option<u16>,
    #[serde(default)]
    pub loop_end: Option<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectTrack {
    pub name: String,
    pub gain_db: f32,
    pub pan: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    /// Launched scene, `None` when the clip slot is stopped.
    #[serde(default)]
    pub scene: Option<u8>,
    /// Session clips, one per non-empty slot.
    #[serde(default)]
    pub clips: Vec<ProjectClip>,
    /// Legacy (v3) single clip. Migrated into `clips` on load.
    #[serde(default)]
    pub notes: Vec<RecordedNote>,
    /// Legacy (v3) clip playback enable.
    #[serde(default = "default_true")]
    pub clip_enabled: bool,
    /// Audio-clip envelope: level plus loop-normalized fades.
    #[serde(default)]
    pub clip_gain_db: f32,
    #[serde(default)]
    pub fade_in: f32,
    #[serde(default)]
    pub fade_out: f32,
    #[serde(default)]
    pub synth: Option<ProjectSynth>,
    /// Instrument slot source: `empty`, `synth` or `plugin`.
    #[serde(default)]
    pub instrument: String,
    /// Legacy (v4) instrument choice. Migrated into `instrument` on load.
    #[serde(default)]
    pub instrument_plugin: Option<bool>,
}

/// Legacy (v1) fixed master FX block.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProjectFx {
    #[serde(default)]
    pub gain: f32,
    #[serde(default)]
    pub tone: f32,
    #[serde(default)]
    pub cutoff: f32,
    #[serde(default)]
    pub resonance: f32,
    #[serde(default)]
    pub drive: f32,
    #[serde(default)]
    pub width: f32,
}

fn default_true() -> bool {
    true
}

fn default_master_gain() -> f32 {
    0.90
}

fn default_chain() -> Vec<ProjectEffect> {
    [
        EffectKind::Drive,
        EffectKind::Tone,
        EffectKind::Filter,
        EffectKind::Width,
    ]
    .into_iter()
    .map(ProjectEffect::new)
    .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectPlugin {
    pub slot: u8,
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    pub format: String,
    pub path: PathBuf,
    #[serde(default)]
    pub bypass: bool,
    /// Base64-encoded plugin state, empty when the plugin exposes none.
    #[serde(default)]
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub format_version: u32,
    #[serde(default)]
    pub app_version: String,
    pub name: String,
    pub bpm: f32,
    #[serde(default = "default_chain")]
    pub effects: Vec<ProjectEffect>,
    #[serde(default = "default_master_gain")]
    pub master_gain: f32,
    /// v1 compatibility only. Ignored when `effects` is present.
    #[serde(default, rename = "fx")]
    pub legacy_fx: Option<ProjectFx>,
    pub tracks: Vec<ProjectTrack>,
    #[serde(default)]
    pub plugins: Vec<ProjectPlugin>,
}

impl Default for Project {
    fn default() -> Self {
        let tracks = TRACKS
            .iter()
            .enumerate()
            .map(|(index, spec)| ProjectTrack {
                name: spec.name.to_string(),
                gain_db: spec.gain_db,
                pan: spec.pan,
                mute: false,
                solo: false,
                scene: Some(crate::audio::DEFAULT_SCENES[index] as u8),
                clips: Vec::new(),
                notes: Vec::new(),
                clip_enabled: true,
                clip_gain_db: 0.0,
                fade_in: 0.0,
                fade_out: 0.0,
                synth: Some(ProjectSynth::default_for_track(index)),
                instrument: "synth".to_string(),
                instrument_plugin: None,
            })
            .collect();

        Self {
            format_version: PROJECT_FORMAT_VERSION,
            app_version: crate::APP_VERSION.to_string(),
            name: "Untitled".to_string(),
            bpm: crate::audio::DEFAULT_BPM,
            effects: default_chain(),
            master_gain: 0.90,
            legacy_fx: None,
            tracks,
            plugins: Vec::new(),
        }
    }
}

impl Project {
    /// Default directory for project files.
    pub fn projects_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("REPLICANT_DAW_PROJECTS") {
            return PathBuf::from(dir);
        }
        let base = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        #[cfg(target_os = "windows")]
        {
            if let Some(appdata) = std::env::var_os("APPDATA") {
                return PathBuf::from(appdata).join("ReplicantDaw/projects");
            }
        }
        #[cfg(target_os = "macos")]
        {
            return base.join("Library/Application Support/ReplicantDaw/projects");
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            base.join(".local/share/replicant-daw/projects")
        }
        #[cfg(not(unix))]
        {
            base.join("ReplicantDaw/projects")
        }
    }

    pub fn ensure_projects_dir() -> Result<PathBuf, String> {
        let dir = Self::projects_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
        Ok(dir)
    }

    pub fn path_for_name(name: &str) -> PathBuf {
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let safe = safe.trim();
        let safe = if safe.is_empty() { "Untitled" } else { safe };
        Self::projects_dir().join(format!("{safe}.{PROJECT_EXTENSION}"))
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|error| format!("serialize failed: {error}"))
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let mut project: Self =
            serde_json::from_str(json).map_err(|error| format!("parse failed: {error}"))?;
        if project.format_version > PROJECT_FORMAT_VERSION {
            return Err(format!(
                "project format {} is newer than this build supports ({})",
                project.format_version, PROJECT_FORMAT_VERSION
            ));
        }

        // v3 stored a single clip per track. Move it into its session slot.
        if project.format_version < 4 {
            for track in project.tracks.iter_mut() {
                if track.clips.is_empty() && !track.notes.is_empty() {
                    track.clips.push(ProjectClip {
                        scene: track.scene.unwrap_or(0),
                        notes: std::mem::take(&mut track.notes),
                        enabled: track.clip_enabled,
                        loop_start: None,
                        loop_end: None,
                    });
                }
            }
        }

        // v4 stored the instrument choice as a tri-state flag.
        if project.format_version < 5 {
            for track in project.tracks.iter_mut() {
                track.instrument = match track.instrument_plugin {
                    Some(true) => "plugin".to_string(),
                    Some(false) | None => "synth".to_string(),
                };
                track.instrument_plugin = None;
            }
        }

        // A v1 file has no `effects`; translate its fixed FX block.
        if project.format_version < 2 {
            if let Some(legacy) = project.legacy_fx.take() {
                project.effects = legacy_chain(&legacy);
                project.master_gain = if legacy.gain > 0.0 { legacy.gain } else { 0.90 };
            }
            for (index, track) in project.tracks.iter_mut().enumerate() {
                if track.synth.is_none() {
                    track.synth = Some(ProjectSynth::default_for_track(index));
                }
            }
        }

        project.format_version = PROJECT_FORMAT_VERSION;
        Ok(project)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        let json = self.to_json()?;
        std::fs::write(path, json)
            .map_err(|error| format!("could not write {}: {error}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        Self::from_json(&json)
    }

    /// List `.rproj` files in the projects directory.
    pub fn list_available() -> Vec<PathBuf> {
        let dir = Self::projects_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case(PROJECT_EXTENSION))
            })
            .collect();
        files.sort();
        files
    }

    pub fn from_session(name: &str, bpm: f32, state: &crate::state::UiState) -> Self {
        let tracks = (0..TRACK_COUNT)
            .map(|index| ProjectTrack {
                name: state.track_label(index).to_string(),
                gain_db: state.track_gain_db[index],
                pan: state.track_pan[index],
                mute: state.mute[index],
                solo: state.solo[index],
                scene: state.launched[index],
                clips: (0..crate::audio::SCENE_COUNT)
                    .filter(|scene| !state.recorded_notes(index, *scene).is_empty())
                    .map(|scene| {
                        let (start, end) = state.clip_loop(index, scene);
                        ProjectClip {
                            scene: scene as u8,
                            notes: state.recorded_notes(index, scene).to_vec(),
                            enabled: state.recorded_active[index][scene],
                            loop_start: Some(start),
                            loop_end: Some(end),
                        }
                    })
                    .collect(),
                notes: Vec::new(),
                clip_enabled: true,
                clip_gain_db: state.clip_gain_db[index],
                fade_in: state.clip_fade_in[index],
                fade_out: state.clip_fade_out[index],
                synth: Some(ProjectSynth::from_params(&state.synth_params(index))),
                instrument: state.instrument[index].id().to_string(),
                instrument_plugin: None,
            })
            .collect();

        let effects = state
            .effects
            .iter()
            .map(|effect| ProjectEffect {
                kind: effect.kind.id().to_string(),
                params: effect.params,
                bypass: effect.bypass,
            })
            .collect();

        Self {
            format_version: PROJECT_FORMAT_VERSION,
            app_version: crate::APP_VERSION.to_string(),
            name: name.to_string(),
            bpm,
            effects,
            master_gain: state.master_gain,
            legacy_fx: None,
            tracks,
            plugins: Vec::new(),
        }
    }
}

/// Translate a v1 FX block into an explicit effect chain.
fn legacy_chain(fx: &ProjectFx) -> Vec<ProjectEffect> {
    let mut chain = Vec::new();
    if fx.drive > 0.001 {
        let mut effect = ProjectEffect::new(EffectKind::Drive);
        effect.params[0] = fx.drive.clamp(0.0, 1.0);
        chain.push(effect);
    }
    if fx.tone > 0.0 && fx.tone < 18_000.0 {
        let mut effect = ProjectEffect::new(EffectKind::Tone);
        effect.params[0] = fx.tone.clamp(300.0, 18_000.0);
        chain.push(effect);
    }
    if fx.cutoff > 0.0 {
        let mut effect = ProjectEffect::new(EffectKind::Filter);
        effect.params[0] = fx.cutoff.clamp(40.0, 12_000.0);
        effect.params[1] = fx.resonance.clamp(0.0, 1.0);
        chain.push(effect);
    }
    if (fx.width - 1.0).abs() > 0.001 {
        let mut effect = ProjectEffect::new(EffectKind::Width);
        effect.params[0] = fx.width.clamp(0.0, 1.5);
        chain.push(effect);
    }
    if chain.is_empty() {
        default_chain()
    } else {
        chain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_project_has_all_tracks_and_a_chain() {
        let project = Project::default();
        assert_eq!(project.tracks.len(), TRACK_COUNT);
        assert_eq!(project.format_version, PROJECT_FORMAT_VERSION);
        assert!(project.tracks.iter().all(|track| track.scene.is_some()));
        assert_eq!(project.effects.len(), 4);
        assert!(project.tracks.iter().all(|track| track.synth.is_some()));
        assert!(project.tracks.iter().all(|track| track.clip_gain_db == 0.0));
        assert!(project.tracks.iter().all(|track| track.fade_in == 0.0));
    }

    #[test]
    fn project_round_trips_through_json() {
        let mut project = Project {
            name: "Neon Test".to_string(),
            bpm: 137.5,
            ..Project::default()
        };
        project.tracks[2].gain_db = -9.5;
        project.tracks[2].mute = true;
        project.tracks[2].clip_gain_db = -3.5;
        project.tracks[2].fade_in = 0.25;
        project.tracks[2].fade_out = 0.10;
        project.tracks[2].instrument = "plugin".to_string();
        project.tracks[1].clips.push(ProjectClip {
            scene: 2,
            notes: vec![RecordedNote {
                note: 60,
                velocity: 100,
                start_step: 16,
                length_steps: 2,
            }],
            enabled: false,
            loop_start: Some(16),
            loop_end: Some(64),
        });
        project.tracks[1].synth = Some(ProjectSynth {
            ratio: 2.5,
            index: 4.0,
            attack: 0.01,
            decay: 0.2,
            sustain: 0.3,
            release: 0.4,
            level: 1.2,
        });
        project.effects.push(ProjectEffect::new(EffectKind::Delay));
        project.master_gain = 0.75;
        project.plugins.push(ProjectPlugin {
            slot: 0,
            name: "Dexed".into(),
            vendor: "Digital Suburban".into(),
            format: "CLAP".into(),
            path: PathBuf::from("/usr/lib/clap/Dexed.clap"),
            bypass: false,
            state: "AAEC".into(),
        });

        let json = project.to_json().expect("serialize");
        let loaded = Project::from_json(&json).expect("parse");
        assert_eq!(loaded.name, "Neon Test");
        assert!((loaded.bpm - 137.5).abs() < 1e-6);
        assert!(loaded.tracks[2].mute);
        assert!((loaded.tracks[2].clip_gain_db + 3.5).abs() < 1e-6);
        assert!((loaded.tracks[2].fade_in - 0.25).abs() < 1e-6);
        assert!((loaded.tracks[2].fade_out - 0.10).abs() < 1e-6);
        assert_eq!(loaded.tracks[2].instrument, "plugin");
        assert_eq!(loaded.tracks[1].clips.len(), 1);
        assert_eq!(loaded.tracks[1].clips[0].scene, 2);
        assert!(!loaded.tracks[1].clips[0].enabled);
        assert_eq!(loaded.tracks[1].clips[0].notes.len(), 1);
        assert_eq!(loaded.tracks[1].clips[0].notes[0].start_step, 16);
        assert_eq!(loaded.tracks[1].clips[0].loop_start, Some(16));
        assert_eq!(loaded.tracks[1].clips[0].loop_end, Some(64));
        assert_eq!(loaded.effects.len(), 5);
        assert_eq!(loaded.effects[4].kind, "DELAY");
        assert!((loaded.master_gain - 0.75).abs() < 1e-6);
        let synth = loaded.tracks[1].synth.as_ref().unwrap();
        assert!((synth.ratio - 2.5).abs() < 1e-6);
        assert_eq!(loaded.plugins[0].format, "CLAP");
    }

    #[test]
    fn v3_single_clip_migrates_into_its_session_slot() {
        let json = r#"{
            "format_version": 3,
            "name": "Old Clips",
            "bpm": 120.0,
            "tracks": [
                { "name": "LEAD", "gain_db": -6.0, "pan": 0.0, "scene": 4,
                  "clip_enabled": false,
                  "notes": [ { "note": 67, "velocity": 90,
                               "start_step": 4, "length_steps": 2 } ] }
            ]
        }"#;
        let project = Project::from_json(json).expect("parse v3 project");
        assert_eq!(project.format_version, PROJECT_FORMAT_VERSION);
        let track = &project.tracks[0];
        assert_eq!(track.name, "LEAD");
        assert_eq!(track.clips.len(), 1);
        assert_eq!(track.clips[0].scene, 4);
        assert!(!track.clips[0].enabled);
        assert_eq!(track.clips[0].notes.len(), 1);
        assert!(track.notes.is_empty());
    }

    #[test]
    fn v4_instrument_flag_migrates_to_the_source_field() {
        let json = r#"{
            "format_version": 4,
            "name": "Old Instrument",
            "bpm": 120.0,
            "tracks": [
                { "name": "SYNTH TRACK", "gain_db": -6.0, "pan": 0.0,
                  "instrument_plugin": false },
                { "name": "HYBRID TRACK", "gain_db": -6.0, "pan": 0.0,
                  "instrument_plugin": true },
                { "name": "PLAIN TRACK", "gain_db": -6.0, "pan": 0.0 }
            ]
        }"#;
        let project = Project::from_json(json).expect("parse v4 project");
        assert_eq!(project.format_version, PROJECT_FORMAT_VERSION);
        assert_eq!(project.tracks[0].instrument, "synth");
        assert_eq!(project.tracks[1].instrument, "plugin");
        // A v4 file without the flag played the internal synth.
        assert_eq!(project.tracks[2].instrument, "synth");
        assert!(project
            .tracks
            .iter()
            .all(|track| track.instrument_plugin.is_none()));
    }

    #[test]
    fn rejects_newer_format_versions() {
        let project = Project {
            format_version: PROJECT_FORMAT_VERSION + 1,
            ..Project::default()
        };
        let json = project.to_json().unwrap();
        assert!(Project::from_json(&json).is_err());
    }

    #[test]
    fn missing_optional_fields_default_cleanly() {
        let json = r#"{
            "format_version": 1,
            "name": "Minimal",
            "bpm": 120.0,
            "tracks": []
        }"#;
        let project = Project::from_json(json).expect("parse minimal project");
        assert_eq!(project.name, "Minimal");
        assert!(project.plugins.is_empty());
        assert!((project.master_gain - 0.9).abs() < 1e-6);
        assert!(!project.effects.is_empty());
    }

    #[test]
    fn legacy_v1_fx_block_becomes_an_effect_chain() {
        let json = r#"{
            "format_version": 1,
            "name": "Old",
            "bpm": 120.0,
            "fx": { "gain": 0.8, "tone": 9000.0, "cutoff": 2400.0,
                    "resonance": 0.5, "drive": 0.3, "width": 1.2 },
            "tracks": []
        }"#;
        let project = Project::from_json(json).expect("parse v1 project");
        assert_eq!(project.format_version, PROJECT_FORMAT_VERSION);
        let kinds: Vec<_> = project
            .effects
            .iter()
            .map(|effect| effect.kind.as_str())
            .collect();
        assert_eq!(kinds, vec!["DRIVE", "TONE", "FILTER", "WIDTH"]);
        assert!((project.master_gain - 0.8).abs() < 1e-6);
        assert!((project.effects[2].params[0] - 2400.0).abs() < 1e-3);
    }

    #[test]
    fn legacy_v1_tracks_get_their_original_synth_defaults() {
        let json = r#"{
            "format_version": 1,
            "name": "Old",
            "bpm": 120.0,
            "tracks": [
                { "name": "SYNAPSE 01", "gain_db": -7.0, "pan": 0.0 },
                { "name": "NEURAL BASS", "gain_db": -5.0, "pan": 0.0 }
            ]
        }"#;
        let project = Project::from_json(json).expect("parse v1 project");
        let first = project.tracks[0].synth.as_ref().unwrap();
        let expected = crate::audio::TRACKS[0];
        assert!((first.ratio - expected.fm_ratio).abs() < 1e-6);
        assert!((first.index - expected.mod_index).abs() < 1e-6);
        let bass = project.tracks[1].synth.as_ref().unwrap();
        assert!((bass.index - crate::audio::TRACKS[1].mod_index).abs() < 1e-6);
    }

    #[test]
    fn project_save_and_load_to_disk() {
        let dir = std::env::temp_dir().join(format!("replicant-project-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("round-trip.rproj");
        let project = Project {
            name: "Disk".into(),
            ..Project::default()
        };
        project.save(&path).expect("save");
        let loaded = Project::load(&path).expect("load");
        assert_eq!(loaded.name, "Disk");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sanitises_project_names() {
        let path = Project::path_for_name("My / Project: 01");
        let name = path.file_stem().unwrap().to_string_lossy();
        assert!(!name.contains('/'));
        assert!(!name.contains(':'));
        assert!(path
            .extension()
            .unwrap()
            .eq_ignore_ascii_case(PROJECT_EXTENSION));
    }
}
