//! Fast, crash-free plugin discovery.
//!
//! The scanner never loads a plugin library. It walks the standard plugin
//! directories and reports what it finds by filename, so a broken third-party
//! binary cannot take the application down while browsing. Metadata such as
//! the real plugin name is filled in when the plugin is actually loaded.

use std::path::{Path, PathBuf};

use super::PluginFormat;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginDescriptor {
    pub name: String,
    pub path: PathBuf,
    pub format: PluginFormat,
}

impl PluginDescriptor {
    pub fn hostable(&self) -> bool {
        self.format.is_hostable()
    }
}

/// Scan the platform default directories.
pub fn scan_all() -> Vec<PluginDescriptor> {
    scan_paths(&super::default_plugin_dirs())
}

/// Recursively scan the given roots for CLAP, VST3 and VST2 plugins.
pub fn scan_paths(roots: &[PathBuf]) -> Vec<PluginDescriptor> {
    let mut found = Vec::new();
    for root in roots {
        walk(root, 0, &mut found);
    }
    found.sort_by(|a, b| {
        a.format
            .label()
            .cmp(b.format.label())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    found.dedup_by(|a, b| a.path == b.path);
    found
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PluginDescriptor>) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let lower = name.to_lowercase();

        if file_type.is_dir() {
            // VST3 bundles are directories ending in `.vst3`.
            if lower.ends_with(".vst3") {
                if vst3_bundle_is_complete(&path) {
                    out.push(PluginDescriptor {
                        name: strip_suffix(&name, ".vst3"),
                        path,
                        format: PluginFormat::Vst3,
                    });
                }
                continue;
            }
            // Some vendors ship CLAP files inside a directory.
            if lower.ends_with(".clap") || lower.ends_with(".vst") {
                continue;
            }
            walk(&path, depth + 1, out);
            continue;
        }

        if lower.ends_with(".clap") {
            out.push(PluginDescriptor {
                name: strip_suffix(&name, ".clap"),
                path,
                format: PluginFormat::Clap,
            });
        } else if lower.ends_with(".vst3") {
            // A single-file VST3 (rare, but some bundles are flat).
            out.push(PluginDescriptor {
                name: strip_suffix(&name, ".vst3"),
                path,
                format: PluginFormat::Vst3,
            });
        } else if is_vst2_library(&lower) {
            out.push(PluginDescriptor {
                name: strip_suffix(&name, vst2_extension(&lower)),
                path,
                format: PluginFormat::Vst2,
            });
        }
    }
}

fn strip_suffix(name: &str, suffix: &str) -> String {
    name.strip_suffix(suffix)
        .or_else(|| {
            // Case-insensitive suffix removal.
            let lower = name.to_lowercase();
            if lower.ends_with(&suffix.to_lowercase()) {
                Some(&name[..name.len() - suffix.len()])
            } else {
                None
            }
        })
        .unwrap_or(name)
        .to_string()
}

fn is_vst2_library(lower: &str) -> bool {
    lower.ends_with(".so") || lower.ends_with(".dll") || lower.ends_with(".dylib")
}

fn vst2_extension(lower: &str) -> &'static str {
    if lower.ends_with(".so") {
        ".so"
    } else if lower.ends_with(".dll") {
        ".dll"
    } else {
        ".dylib"
    }
}

/// A VST3 bundle is usable if it contains a `Contents` directory with a
/// platform library. Some vendors also ship a flat `.vst3` file.
fn vst3_bundle_is_complete(path: &Path) -> bool {
    let contents = path.join("Contents");
    if !contents.is_dir() {
        return false;
    }
    for arch in [
        "x86_64-linux",
        "aarch64-linux",
        "x86_64-win",
        "x86_64-mac",
        "arm64-mac",
    ] {
        let dir = contents.join(arch);
        if dir.is_dir() {
            return true;
        }
    }
    // macOS uses `Contents/MacOS`.
    contents.join("MacOS").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("replicant-scan-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_flat_clap_files() {
        let dir = temp_dir("clap");
        std::fs::write(dir.join("Dexed.clap"), b"not really a plugin").unwrap();
        let found = scan_paths(std::slice::from_ref(&dir));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Dexed");
        assert_eq!(found[0].format, PluginFormat::Clap);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn finds_vst3_bundles_and_skips_incomplete_ones() {
        let dir = temp_dir("vst3");
        let good = dir.join("Synth.vst3").join("Contents").join("x86_64-linux");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(good.join("Synth.so"), b"stub").unwrap();
        std::fs::create_dir_all(dir.join("Broken.vst3")).unwrap();

        let found = scan_paths(std::slice::from_ref(&dir));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Synth");
        assert_eq!(found[0].format, PluginFormat::Vst3);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn marks_vst2_as_not_hostable() {
        let dir = temp_dir("vst2");
        std::fs::write(dir.join("Legacy.so"), b"stub").unwrap();
        let found = scan_paths(std::slice::from_ref(&dir));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].format, PluginFormat::Vst2);
        assert!(!found[0].hostable());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ignores_unrelated_files() {
        let dir = temp_dir("misc");
        std::fs::write(dir.join("readme.txt"), b"hi").unwrap();
        std::fs::write(dir.join("cover.png"), b"hi").unwrap();
        assert!(scan_paths(std::slice::from_ref(&dir)).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
