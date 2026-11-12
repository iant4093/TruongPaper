//! Local Steam discovery: `libraryfolders.vdf`, Wallpaper Engine core `assets/`, workshop content
//! scanning for `steamapps/workshop/content/431960`, and a `notify` filesystem watcher.
//!
//! Pure logic ([`parse_library_folders_vdf`], [`scan_workshop_roots`], [`workshop_event_paths`])
//! is separated from environment lookups and the OS watcher so it is testable on any host.

use crate::WALLPAPER_ENGINE_APP_ID;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredWorkshopWallpaper {
    pub wallpaper_id: String,
    pub workshop_id: String,
    pub title: String,
    pub r#type: String,
    pub preview_path: String,
    pub install_dir: String,
    pub bytes: u64,
    pub audio_reactive: bool,
    pub windows_only: bool,
    pub project_json: serde_json::Value,
}

/// Returns candidate root Steam installation directories on macOS and Windows.
pub fn find_default_steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    #[cfg(target_os = "macos")]
    if let Ok(home) = std::env::var("HOME") {
        let mac_steam = PathBuf::from(home).join("Library/Application Support/Steam");
        if mac_steam.is_dir() {
            roots.push(mac_steam);
        }
    }

    #[cfg(target_os = "windows")]
    {
        for env_key in ["PROGRAMFILES(X86)", "PROGRAMFILES"] {
            if let Ok(base) = std::env::var(env_key) {
                let candidate = PathBuf::from(base).join("Steam");
                if candidate.is_dir() {
                    roots.push(candidate);
                }
            }
        }
        for drive in ['C', 'D', 'E', 'F'] {
            let candidate = PathBuf::from(format!("{drive}:\\Steam"));
            if candidate.is_dir() && !roots.contains(&candidate) {
                roots.push(candidate);
            }
        }
    }

    roots
}

/// Parses the text of `steamapps/libraryfolders.vdf` and returns every library root path in
/// numeric key order. Handles the modern format (`"0" { "path" "..." ... }`) and the legacy
/// format (`"1" "D:\\SteamLibrary"`). Invalid input yields an empty list.
pub fn parse_library_folders_vdf(vdf_text: &str) -> Vec<PathBuf> {
    let Ok(vdf) = keyvalues_parser::parse(vdf_text) else {
        return Vec::new();
    };
    let Some(root_obj) = vdf.value.get_obj() else {
        return Vec::new();
    };

    // Only numeric keys are library slots; legacy files also carry keys such as
    // `TimeNextStatsReport` and `ContentStatsID` with string values.
    let mut slots: Vec<(u64, PathBuf)> = Vec::new();
    for (key, values) in root_obj.iter() {
        let Ok(index) = key.parse::<u64>() else {
            continue;
        };
        for val in values {
            let path = if let Some(entry) = val.get_obj() {
                entry
                    .get("path")
                    .and_then(|v| v.first())
                    .and_then(|v| v.get_str())
            } else {
                val.get_str()
            };
            if let Some(path) = path.filter(|p| !p.is_empty()) {
                slots.push((index, PathBuf::from(path)));
            }
        }
    }
    slots.sort_by_key(|(i, _)| *i);
    slots.into_iter().map(|(_, p)| p).collect()
}

/// Discovers all Steam library folders on the machine (default roots + `libraryfolders.vdf`).
pub fn discover_all_steam_libraries() -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut libraries = Vec::new();

    for root in find_default_steam_roots() {
        if seen.insert(root.clone()) {
            libraries.push(root.clone());
        }
        let vdf_path = root.join("steamapps").join("libraryfolders.vdf");
        if let Ok(vdf_text) = std::fs::read_to_string(&vdf_path) {
            for lib_path in parse_library_folders_vdf(&vdf_text) {
                if lib_path.is_dir() && seen.insert(lib_path.clone()) {
                    libraries.push(lib_path);
                }
            }
        }
    }
    libraries
}

/// `<library>/steamapps/workshop/content/431960`
pub fn workshop_content_dir(library: &Path) -> PathBuf {
    library
        .join("steamapps")
        .join("workshop")
        .join("content")
        .join(WALLPAPER_ENGINE_APP_ID.to_string())
}

/// `<library>/steamapps/common/wallpaper_engine/assets` if it holds a `shaders/` dir.
pub fn we_core_assets_in_library(library: &Path) -> Option<PathBuf> {
    let candidate = library
        .join("steamapps")
        .join("common")
        .join("wallpaper_engine")
        .join("assets");
    candidate.join("shaders").is_dir().then_some(candidate)
}

/// Checks if a local Wallpaper Engine installation exists and returns its `<we_dir>/assets` path.
pub fn find_local_we_core_assets_dir() -> Option<PathBuf> {
    discover_all_steam_libraries()
        .iter()
        .find_map(|lib| we_core_assets_in_library(lib))
}

/// Existing `431960` content directories across all Steam libraries.
pub fn local_workshop_content_dirs() -> Vec<PathBuf> {
    discover_all_steam_libraries()
        .iter()
        .map(|lib| workshop_content_dir(lib))
        .filter(|d| d.is_dir())
        .collect()
}

/// Scans `<app_data_dir>/wallpapers/workshop/*` plus every local Steam `431960` content folder.
pub fn scan_workshop_directories(app_workshop_dir: &Path) -> Vec<DiscoveredWorkshopWallpaper> {
    let mut roots = Vec::new();
    if app_workshop_dir.is_dir() {
        roots.push(app_workshop_dir.to_path_buf());
    }
    roots.extend(local_workshop_content_dirs());
    scan_workshop_roots(&roots)
}

/// Parses each `<root>/<id>/project.json`. Later roots override earlier ones for the same ID.
pub fn scan_workshop_roots(roots: &[PathBuf]) -> Vec<DiscoveredWorkshopWallpaper> {
    let mut by_workshop_id: HashMap<String, DiscoveredWorkshopWallpaper> = HashMap::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir_path = entry.path();
            if !dir_path.is_dir() {
                continue;
            }
            let Ok(raw_json) = std::fs::read_to_string(dir_path.join("project.json")) else {
                continue;
            };
            // Workshop files may start with a UTF-8 BOM.
            let Ok(project_json) =
                serde_json::from_str::<serde_json::Value>(raw_json.trim_start_matches('\u{feff}'))
            else {
                continue;
            };

            let folder_name = entry.file_name().to_string_lossy().into_owned();
            let title = project_json
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(&folder_name)
                .to_string();
            let raw_type = project_json
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("scene")
                .to_ascii_lowercase();
            // `preview` comes from downloaded content, so keep it inside the item folder.
            let preview_rel = project_json
                .get("preview")
                .and_then(|v| v.as_str())
                .filter(|p| {
                    let path = Path::new(p);
                    !p.is_empty()
                        && path
                            .components()
                            .all(|c| matches!(c, std::path::Component::Normal(_)))
                })
                .unwrap_or("preview.jpg");
            let audio_reactive = project_json
                .pointer("/general/supportsaudioprocessing")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let bytes = compute_dir_size(&dir_path);
            let windows_only = raw_type == "application";

            by_workshop_id.insert(
                folder_name.clone(),
                DiscoveredWorkshopWallpaper {
                    wallpaper_id: format!("workshop:{folder_name}"),
                    workshop_id: folder_name,
                    title,
                    r#type: raw_type,
                    preview_path: dir_path.join(preview_rel).to_string_lossy().into_owned(),
                    install_dir: dir_path.to_string_lossy().into_owned(),
                    bytes,
                    audio_reactive,
                    windows_only,
                    project_json,
                },
            );
        }
    }

    by_workshop_id.into_values().collect()
}

fn compute_dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if let Ok(meta) = e.metadata() {
                if meta.is_file() {
                    total += meta.len();
                } else if meta.is_dir() {
                    total += compute_dir_size(&e.path());
                }
            }
        }
    }
    total
}

/// Returns the `project.json` paths affected by a filesystem event (create, modify, remove).
pub fn workshop_event_paths(event: &Event) -> Vec<PathBuf> {
    if matches!(event.kind, EventKind::Access(_)) {
        return Vec::new();
    }
    event
        .paths
        .iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some("project.json"))
        .cloned()
        .collect()
}

/// Watches `roots` recursively (missing directories are skipped) and sends every changed
/// `project.json` path on `changed_tx`. Keep the returned watcher alive to keep watching.
pub fn spawn_workshop_watcher(
    roots: &[PathBuf],
    changed_tx: mpsc::UnboundedSender<PathBuf>,
) -> anyhow::Result<RecommendedWatcher> {
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                for path in workshop_event_paths(&event) {
                    let _ = changed_tx.send(path);
                }
            }
        },
        Config::default(),
    )?;

    for dir in roots.iter().filter(|d| d.is_dir()) {
        if let Err(err) = watcher.watch(dir, RecursiveMode::Recursive) {
            tracing::warn!("failed to watch {}: {err}", dir.display());
        }
    }
    Ok(watcher)
}

/// Watches every local `steamapps/workshop/content/431960` folder.
pub fn spawn_local_workshop_watcher(
    changed_tx: mpsc::UnboundedSender<PathBuf>,
) -> anyhow::Result<RecommendedWatcher> {
    spawn_workshop_watcher(&local_workshop_content_dirs(), changed_tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, ModifyKind};

    const MODERN_VDF: &str = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/Users/ian/Library/Application Support/Steam"
		"label"		""
		"apps"
		{
			"431960"		"123456"
		}
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
		"label"		""
	}
}
"#;

    const LEGACY_VDF: &str = r#"
"LibraryFolders"
{
	"TimeNextStatsReport"		"1700000000"
	"ContentStatsID"		"-123456789"
	"1"		"E:\\Games\\Steam"
}
"#;

    #[test]
    fn parses_modern_libraryfolders() {
        let libs = parse_library_folders_vdf(MODERN_VDF);
        assert_eq!(
            libs,
            vec![
                PathBuf::from("/Users/ian/Library/Application Support/Steam"),
                PathBuf::from(r"D:\SteamLibrary"),
            ]
        );
    }

    #[test]
    fn parses_legacy_libraryfolders_and_skips_non_slot_keys() {
        assert_eq!(
            parse_library_folders_vdf(LEGACY_VDF),
            vec![PathBuf::from(r"E:\Games\Steam")]
        );
    }

    #[test]
    fn orders_libraries_numerically() {
        let vdf =
            r#""libraryfolders" { "10" { "path" "/c" } "2" { "path" "/b" } "0" { "path" "/a" } }"#;
        assert_eq!(
            parse_library_folders_vdf(vdf),
            vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c")
            ]
        );
    }

    #[test]
    fn invalid_vdf_yields_empty() {
        assert!(parse_library_folders_vdf("").is_empty());
        assert!(parse_library_folders_vdf("not { vdf").is_empty());
    }

    #[test]
    fn workshop_content_dir_layout() {
        assert_eq!(
            workshop_content_dir(Path::new("/lib")),
            PathBuf::from("/lib/steamapps/workshop/content/431960")
        );
    }

    #[test]
    fn event_filter_keeps_only_project_json_changes() {
        let ev = Event::new(EventKind::Create(CreateKind::File))
            .add_path(PathBuf::from("/w/123456/project.json"))
            .add_path(PathBuf::from("/w/123456/scene.pkg"));
        assert_eq!(
            workshop_event_paths(&ev),
            vec![PathBuf::from("/w/123456/project.json")]
        );

        let modify = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(PathBuf::from("/w/1/project.json"));
        assert_eq!(workshop_event_paths(&modify).len(), 1);

        let access = Event::new(EventKind::Access(AccessKind::Any))
            .add_path(PathBuf::from("/w/1/project.json"));
        assert!(workshop_event_paths(&access).is_empty());
    }

    #[test]
    fn scan_roots_reads_project_json() {
        let root =
            std::env::temp_dir().join(format!("truongpaper-steam-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let item = root.join("2841928371");
        std::fs::create_dir_all(&item).unwrap();
        std::fs::write(
            item.join("project.json"),
            "\u{feff}{\"title\":\"Neon\",\"type\":\"Scene\",\"preview\":\"p.png\",\"general\":{\"supportsaudioprocessing\":true}}",
        )
        .unwrap();
        std::fs::write(item.join("scene.pkg"), [0u8; 10]).unwrap();
        std::fs::create_dir_all(root.join("999999")).unwrap(); // no project.json

        let found = scan_workshop_roots(std::slice::from_ref(&root));
        assert_eq!(found.len(), 1);
        let w = &found[0];
        assert_eq!(w.wallpaper_id, "workshop:2841928371");
        assert_eq!(w.title, "Neon");
        assert_eq!(w.r#type, "scene");
        assert!(w.audio_reactive);
        assert!(!w.windows_only);
        assert!(w.bytes >= 10);
        assert!(w.preview_path.ends_with("p.png"));

        // A preview path that leaves the item folder falls back to the default.
        std::fs::write(
            item.join("project.json"),
            "{\"title\":\"Neon\",\"preview\":\"../../secret.png\"}",
        )
        .unwrap();
        let found = scan_workshop_roots(std::slice::from_ref(&root));
        assert!(Path::new(&found[0].preview_path).ends_with("2841928371/preview.jpg"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
