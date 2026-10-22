//! Project persistence: a JSON document describing the session.
//!
//! Recorded MIDI clips are stored as note lists, plugins as descriptors plus
//! their opaque state blob (base64), and the mix as plain numbers, so a
//! project file is human-inspectable and versionable.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::audio::messages::{FxParam, RecordedNote, TRACK_COUNT};
use crate::audio::TRACKS;

pub const PROJECT_EXTENSION: &str = "rproj";
pub const PROJECT_FORMAT_VERSION: u32 = 1;

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
    #[serde(default)]
    pub notes: Vec<RecordedNote>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectFx {
    pub gain: f32,
    pub tone: f32,
    pub cutoff: f32,
    pub resonance: f32,
    pub drive: f32,
    pub width: f32,
}

impl Default for ProjectFx {
    fn default() -> Self {
        Self {
            gain: 0.90,
            tone: 16_000.0,
            cutoff: 5_200.0,
            resonance: 0.18,
            drive: 0.12,
            width: 1.0,
        }
    }
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
    #[serde(default)]
    pub fx: ProjectFx,
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
                notes: Vec::new(),
            })
            .collect();

        Self {
            format_version: PROJECT_FORMAT_VERSION,
            app_version: crate::APP_VERSION.to_string(),
            name: "Untitled".to_string(),
            bpm: crate::audio::DEFAULT_BPM,
            fx: ProjectFx::default(),
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
        let project: Self =
            serde_json::from_str(json).map_err(|error| format!("parse failed: {error}"))?;
        if project.format_version > PROJECT_FORMAT_VERSION {
            return Err(format!(
                "project format {} is newer than this build supports ({})",
                project.format_version, PROJECT_FORMAT_VERSION
            ));
        }
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
                name: TRACKS[index].name.to_string(),
                gain_db: state.track_gain_db[index],
                pan: state.track_pan[index],
                mute: state.mute[index],
                solo: state.solo[index],
                scene: state.launched[index],
                notes: state.recorded_notes(index).to_vec(),
            })
            .collect();

        Self {
            format_version: PROJECT_FORMAT_VERSION,
            app_version: crate::APP_VERSION.to_string(),
            name: name.to_string(),
            bpm,
            fx: ProjectFx {
                gain: state.fx.gain,
                tone: state.fx.tone,
                cutoff: state.fx.cutoff,
                resonance: state.fx.resonance,
                drive: state.fx.drive,
                width: state.fx.width,
            },
            tracks,
            plugins: Vec::new(),
        }
    }

    /// Look up a stored FX value as a typed command payload.
    pub fn fx_value(&self, param: FxParam) -> f32 {
        match param {
            FxParam::Gain => self.fx.gain,
            FxParam::Tone => self.fx.tone,
            FxParam::Cutoff => self.fx.cutoff,
            FxParam::Resonance => self.fx.resonance,
            FxParam::Drive => self.fx.drive,
            FxParam::Width => self.fx.width,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_project_has_all_tracks() {
        let project = Project::default();
        assert_eq!(project.tracks.len(), TRACK_COUNT);
        assert_eq!(project.format_version, PROJECT_FORMAT_VERSION);
        assert!(project.tracks.iter().all(|track| track.scene.is_some()));
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
        project.tracks[1].notes.push(RecordedNote {
            note: 60,
            velocity: 100,
            start_step: 16,
            length_steps: 2,
        });
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
        assert_eq!(loaded.tracks[1].notes.len(), 1);
        assert_eq!(loaded.tracks[1].notes[0].start_step, 16);
        assert_eq!(loaded.plugins[0].format, "CLAP");
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
        assert!((project.fx.gain - 0.9).abs() < 1e-6);
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
