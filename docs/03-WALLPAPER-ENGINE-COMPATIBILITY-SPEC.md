# 03 — Wallpaper Engine Compatibility & Steam Workshop Specification

> Historical design spec, written before implementation. The shipped app differs in places; see [RELEASE-READINESS.md](RELEASE-READINESS.md) and the code for current behavior.

> **Implementation Tier**: Copy-Paste Ready / Byte-Verified Specification  
> All binary structures (`.pkg`, `.tex`, `.mdl`), shader preprocessor rules, particle vertex layouts, `DepotDownloader` v3.4.0 sidecar commands, and 4-layer VFS mounting rules below have been verified directly against the upstream `RePKG`, `linux-wallpaperengine`, and `DepotDownloader` source code.

---

## 1. Steam Workshop Downloader & Local Folder Sync (`truongpaper-steam`)

### 1.1 Why `DepotDownloader` v3.4.0 Sidecar (Instead of `steam-vent` Alone)
> [!WARNING]
> **Critical Crate Fact**: The `steam-vent` crate on `crates.io` only implements the Steam CM login/chat protocol — it does **not** implement SteamPipe Depot Manifest parsing, CDN chunk downloading, or `-pubfile` Workshop extraction.  
> Instead, **`SteamRE/DepotDownloader` (v3.4.0)** publishes official, standalone, zero-dependency native binaries for all 4 target architectures, bundled into `src-tauri/bin/` via `scripts/bundle-depotdownloader.mjs` (matching the sidecar pattern in `TruongFlix`):
> - `DepotDownloader-macos-arm64.zip` $\to$ `src-tauri/bin/DepotDownloader-aarch64-apple-darwin`
> - `DepotDownloader-macos-x64.zip` $\to$ `src-tauri/bin/DepotDownloader-x86_64-apple-darwin`
> - `DepotDownloader-windows-x64.zip` $\to$ `src-tauri/bin/DepotDownloader-x86_64-pc-windows-msvc.exe`
> - `DepotDownloader-windows-arm64.zip` $\to$ `src-tauri/bin/DepotDownloader-aarch64-pc-windows-msvc.exe`
>
> Bundle `DepotDownloader` via Tauri v2's `bundle.externalBin: ["bin/DepotDownloader"]` sidecar mechanism.

### 1.2 Exact `DepotDownloader` CLI Invocations & Non-Newline `stderr` Sidecar Runner

1. **QR Code Login Flow**:
   - Command:
     ```bash
     DepotDownloader -app 431960 -qr -remember-password -manifest-only
     ```
   - `DepotDownloader` prints to `stdout`:
     `Use the Steam Mobile App to sign in with this QR code:` followed by Unicode half-block lines (`█`, `▀`, `▄`, `' '`).
   - Convert those Unicode half-block lines into a `Vec<Vec<bool>>` boolean module matrix using the exact Rust function below and emit `steam-auth://qr-matrix` to render a crisp vector SVG QR code in the Liquid Glass modal:
     ```rust
     pub fn parse_ascii_qr_to_matrix(lines: &[String]) -> Vec<Vec<bool>> {
         let mut matrix = Vec::with_capacity(lines.len() * 2);
         for line in lines {
             let mut top_row = Vec::new();
             let mut bot_row = Vec::new();
             for ch in line.chars() {
                 match ch {
                     '█' => { top_row.push(true);  bot_row.push(true);  }
                     '▀' => { top_row.push(true);  bot_row.push(false); }
                     '▄' => { top_row.push(false); bot_row.push(true);  }
                     _   => { top_row.push(false); bot_row.push(false); }
                 }
             }
             matrix.push(top_row);
             matrix.push(bot_row);
         }
         matrix
     }
     ```
   - Once the user scans the QR code in the Steam Mobile App, `DepotDownloader` prints:
     `Success! Next time you can login with -username <AccountName> -remember-password instead of -qr.`
   - Capture `<AccountName>` and save `steam_username = <AccountName>` in the OS Keychain vault (`src-tauri/src/secrets.rs`).

2. **Username + Password + 2FA Login Flow**:
   - Command:
     ```bash
     DepotDownloader -app 431960 -username <USER> -password <PASS> -remember-password -no-mobile -manifest-only
     ```
   - **CRITICAL `stderr` Quirk**: `ConsoleAuthenticator.cs` in `DepotDownloader` writes the 2FA prompt to **`stderr` WITHOUT a trailing newline**:
     - Authenticator app prompt: `"STEAM GUARD! Please enter your 2-factor auth code from your authenticator app: "`
     - Email prompt: `"STEAM GUARD! Please enter the auth code sent to the email at "`
   - **Never use `BufReader::lines()` on `stderr`** (it will deadlock waiting for `\n` while `DepotDownloader` blocks on `Console.ReadLine()`!). Instead, read `stderr` with raw byte chunks (`stderr.read(&mut buf).await`), match `"Please enter"`, emit `steam-auth://need-2fa` to the UI, and write `format!("{code}\n")` to `child.stdin`:

```rust
// src-tauri/crates/truongpaper-steam/src/sidecar.rs
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum SidecarEvent {
    QrMatrix(Vec<Vec<bool>>),
    Need2Fa { kind: &'static str, prompt: String },
    LoggedInUsername(String),
    ProgressPercent(f32),
    Completed,
}

pub async fn run_depot_downloader_sidecar(
    sidecar_bin: &Path,
    args: &[String],
    mut two_fa_rx: mpsc::Receiver<String>,
    event_tx: mpsc::Sender<SidecarEvent>,
) -> anyhow::Result<()> {
    let mut cmd = Command::new(sidecar_bin);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

    let mut child = cmd.spawn()?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");

    // 1. Spawn raw-chunk stderr reader (catches non-newline 2FA prompts!)
    let tx_err = event_tx.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        while let Ok(n) = stderr.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&buf[..n]).into_owned();
            if text.contains("Please enter") {
                let kind = if text.contains("email") { "email" } else { "totp" };
                let _ = tx_err
                    .send(SidecarEvent::Need2Fa {
                        kind,
                        prompt: text.trim().to_string(),
                    })
                    .await;
                if let Some(code) = two_fa_rx.recv().await {
                    let line = format!("{}\n", code.trim());
                    let _ = stdin.write_all(line.as_bytes()).await;
                    let _ = stdin.flush().await;
                }
            }
        }
    });

    // 2. Read stdout lines for ASCII QR code, saved username, and download progress %
    let mut lines = BufReader::new(stdout).lines();
    let mut qr_accum: Vec<String> = Vec::new();
    let mut in_qr = false;

    while let Some(line) = lines.next_line().await? {
        if line.contains("Use the Steam Mobile App to sign in with this QR code") {
            in_qr = true;
            qr_accum.clear();
            continue;
        }
        if in_qr {
            let is_qr_line = !line.trim().is_empty()
                && line.chars().all(|c| matches!(c, '█' | '▀' | '▄' | ' '));
            if is_qr_line {
                qr_accum.push(line);
                continue;
            } else if !qr_accum.is_empty() {
                let matrix = super::parse_ascii_qr_to_matrix(&qr_accum);
                let _ = event_tx.send(SidecarEvent::QrMatrix(matrix)).await;
                in_qr = false;
                qr_accum.clear();
            }
        }

        // Capture "-username <AccountName> -remember-password" after QR login
        if let Some(idx) = line.find("login with -username ") {
            let rest = &line[idx + "login with -username ".len()..];
            if let Some(user) = rest.split_whitespace().next() {
                let _ = event_tx
                    .send(SidecarEvent::LoggedInUsername(user.to_string()))
                    .await;
            }
        }

        // Capture progress percentage e.g. " 42.68% ..."
        if let Some(pct_idx) = line.find('%') {
            let prefix = line[..pct_idx].trim();
            if let Some(num_str) = prefix.split_whitespace().last() {
                if let Ok(pct) = num_str.parse::<f32>() {
                    let _ = event_tx.send(SidecarEvent::ProgressPercent(pct)).await;
                }
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("DepotDownloader exited with status: {status}");
    }
    let _ = event_tx.send(SidecarEvent::Completed).await;
    Ok(())
}
```

3. **Downloading Any Pasted Workshop Item (`-pubfile`)**:
   - Once authenticated, download any Workshop ID `<WORKSHOP_ID>` with:
     ```bash
     DepotDownloader -app 431960 -pubfile <WORKSHOP_ID> -username <SAVED_USER> -remember-password -dir "<app_data_dir>/wallpapers/workshop/<WORKSHOP_ID>" -max-downloads 8
     ```

### 1.3 Local Steam Workshop Auto-Discovery, `libraryfolders.vdf` Parser & Public Metadata Client (`crates/truongpaper-steam/src/local_sync.rs`)

Provides the complete implementation for `steam_inspect_url` (via Steam's unauthenticated `ISteamRemoteStorage/GetPublishedFileDetails/v1/` API), `libraryfolders.vdf` parsing via `keyvalues-parser`, local Wallpaper Engine `assets/` discovery, `steamapps/workshop/content/431960` scanning, and real-time directory watching via `notify`:

```rust
// src-tauri/crates/truongpaper-steam/src/local_sync.rs
use keyvalues_parser::Vdf;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

pub const WALLPAPER_ENGINE_APP_ID: u32 = 431960;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopPreview {
    pub workshop_id: String,
    pub title: String,
    pub description: String,
    pub preview_url: String,
    pub file_size_bytes: u64,
    pub tags: Vec<String>,
    pub time_updated: u64,
}

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

/// Extracts a numeric Steam Workshop ID from any Workshop URL, `steam://` URI, or raw numeric string.
pub fn parse_workshop_url_or_id(input: &str) -> anyhow::Result<String> {
    let trimmed = input.trim();
    if trimmed.chars().all(|c| c.is_ascii_digit()) && (6..=12).contains(&trimmed.len()) {
        return Ok(trimmed.to_string());
    }
    if let Ok(parsed) = url::Url::parse(trimmed) {
        for (k, v) in parsed.query_pairs() {
            if k == "id" && v.chars().all(|c| c.is_ascii_digit()) {
                return Ok(v.into_owned());
            }
        }
        if let Some(last_seg) = parsed.path_segments().and_then(|s| s.last()) {
            if last_seg.chars().all(|c| c.is_ascii_digit()) && !last_seg.is_empty() {
                return Ok(last_seg.to_string());
            }
        }
    }
    anyhow::bail!("Invalid Steam Workshop URL or ID: {trimmed}")
}

/// Queries the unauthenticated Steam Web API (`ISteamRemoteStorage/GetPublishedFileDetails/v1/`)
/// and verifies that the item belongs to Wallpaper Engine (`consumer_app_id == 431960`).
pub async fn fetch_workshop_preview(
    client: &reqwest::Client,
    url_or_id: &str,
) -> anyhow::Result<WorkshopPreview> {
    let workshop_id = parse_workshop_url_or_id(url_or_id)?;
    let form = [
        ("itemcount", "1"),
        ("publishedfileids[0]", workshop_id.as_str()),
    ];

    let resp: serde_json::Value = client
        .post("https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/")
        .form(&form)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let details = resp
        .pointer("/response/publishedfiledetails/0")
        .ok_or_else(|| anyhow::anyhow!("Missing publishedfiledetails in Steam response"))?;

    let consumer_app_id = details
        .get("consumer_app_id")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    if consumer_app_id != WALLPAPER_ENGINE_APP_ID {
        anyhow::bail!(
            "Workshop item #{workshop_id} belongs to AppID {consumer_app_id}, not Wallpaper Engine (431960)"
        );
    }

    let tags = details
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("tag").and_then(|s| s.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Ok(WorkshopPreview {
        workshop_id,
        title: details
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled Workshop Wallpaper")
            .to_string(),
        description: details
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        preview_url: details
            .get("preview_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        file_size_bytes: details
            .get("file_size")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0),
        tags,
        time_updated: details
            .get("time_updated")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    })
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

/// Parses `steamapps/libraryfolders.vdf` using `keyvalues-parser` and returns all Steam library root paths.
pub fn parse_library_folders_vdf(vdf_text: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Ok(vdf) = Vdf::parse(vdf_text) else {
        return paths;
    };
    let Some(root_obj) = vdf.value.get_obj() else {
        return paths;
    };

    for (_folder_idx, values) in root_obj.iter() {
        for val in values {
            if let Some(entry_obj) = val.get_obj() {
                if let Some(path_vals) = entry_obj.get("path") {
                    if let Some(raw_path) = path_vals.first().and_then(|v| v.get_str()) {
                        let unescaped = raw_path.replace("\\\\", "\\");
                        paths.push(PathBuf::from(unescaped));
                    }
                }
            } else if let Some(legacy_path) = val.get_str() {
                let unescaped = legacy_path.replace("\\\\", "\\");
                paths.push(PathBuf::from(unescaped));
            }
        }
    }
    paths
}

/// Discovers all Steam library folders on the machine (combining default roots + `libraryfolders.vdf`).
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

/// Checks if a local Wallpaper Engine installation exists and returns its `<we_dir>/assets` path if found.
pub fn find_local_we_core_assets_dir() -> Option<PathBuf> {
    for lib in discover_all_steam_libraries() {
        let candidate = lib
            .join("steamapps")
            .join("common")
            .join("wallpaper_engine")
            .join("assets");
        if candidate.join("shaders").is_dir() {
            return Some(candidate);
        }
    }
    None
}

/// Scans all local `steamapps/workshop/content/431960/*` and `<app_data_dir>/wallpapers/workshop/*`
/// directories and parses each `project.json` into a `DiscoveredWorkshopWallpaper`.
pub fn scan_workshop_directories(
    app_workshop_dir: &Path,
) -> Vec<DiscoveredWorkshopWallpaper> {
    let mut content_roots = Vec::new();
    if app_workshop_dir.is_dir() {
        content_roots.push(app_workshop_dir.to_path_buf());
    }
    for lib in discover_all_steam_libraries() {
        let workshop_431960 = lib
            .join("steamapps")
            .join("workshop")
            .join("content")
            .join(WALLPAPER_ENGINE_APP_ID.to_string());
        if workshop_431960.is_dir() {
            content_roots.push(workshop_431960);
        }
    }

    let mut by_workshop_id: HashMap<String, DiscoveredWorkshopWallpaper> = HashMap::new();
    for root in content_roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir_path = entry.path();
            if !dir_path.is_dir() {
                continue;
            }
            let project_file = dir_path.join("project.json");
            let Ok(raw_json) = std::fs::read_to_string(&project_file) else {
                continue;
            };
            let Ok(project_json) = serde_json::from_str::<serde_json::Value>(&raw_json) else {
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
            let preview_rel = project_json
                .get("preview")
                .and_then(|v| v.as_str())
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

/// Watches all discovered `steamapps/workshop/content/431960` folders using `notify`
/// and triggers `changed_tx` whenever a new `project.json` is created or modified.
pub fn spawn_local_workshop_watcher(
    changed_tx: mpsc::UnboundedSender<PathBuf>,
) -> anyhow::Result<RecommendedWatcher> {
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                for path in event.paths {
                    if path.file_name().and_then(|n| n.to_str()) == Some("project.json") {
                        let _ = changed_tx.send(path);
                    }
                }
            }
        },
        Config::default(),
    )?;

    for lib in discover_all_steam_libraries() {
        let workshop_dir = lib
            .join("steamapps")
            .join("workshop")
            .join("content")
            .join(WALLPAPER_ENGINE_APP_ID.to_string());
        if workshop_dir.is_dir() {
            let _ = watcher.watch(&workshop_dir, RecursiveMode::Recursive);
        }
    }

    Ok(watcher)
}
```

---

## 2. Four-Layer Virtual File System (`VFS`) & Core Assets Bootstrap

> [!IMPORTANT]
> **Why `Scene` Wallpapers Need a 4-Layer VFS**  
> Workshop `scene.pkg` archives do **not** bundle Wallpaper Engine's standard base files (`shaders/common.h`, `shaders/common_vertex.h`, `shaders/common_fragment.h`, `shaders/common_blending.h`, `shaders/common_particles.h`, `shaders/genericimage2.vert/.frag`, `shaders/genericparticle.vert/.frag`, `shaders/genericropeparticle.vert/.frag`, `materials/util/effectpassthrough.json`, etc.).
>
> Every `Scene` wallpaper must resolve virtual paths (e.g. `"shaders/genericimage2.vert"`, `"materials/bg.tex"`) across **4 mounted VFS layers in priority order**:
> 1. **Layer 1 (Loose Wallpaper Folder)**: `<wallpaper_dir>/<virtual_path>`
> 2. **Layer 2 (`scene.pkg`)**: Memory-mapped archive `<wallpaper_dir>/scene.pkg`
> 3. **Layer 3 (`gifscene.pkg`)**: Memory-mapped archive `<wallpaper_dir>/gifscene.pkg`
> 4. **Layer 4 (Core `assets/` Directory & Built-in Virtual Shaders)**:
>    - First checks local Steam Wallpaper Engine install (`<steam_lib>/steamapps/common/wallpaper_engine/assets/`).
>    - On macOS (or Windows without WE installed), automatically downloads Wallpaper Engine's `assets/` folder once on first Steam login via:
>      ```bash
>      DepotDownloader -app 431960 -depot 431961 -filelist assets_only.txt -username <USER> -remember-password -dir "<app_data_dir>/we_core_assets"
>      ```
>      where `assets_only.txt` contains `regex:^assets/(shaders|materials|effects|models)/.*`.
>    - Also mounts built-in virtual shaders directly in memory (including `shaders/commands/copy.vert`, `shaders/commands/copy.frag`, and the virtual camera bloom chain `effects/wpenginelinux/bloomeffect.json`).

### 2.1 Complete 4-Layer `WallpaperVfs` Implementation (`crates/truongpaper-we-formats/src/vfs.rs`)

```rust
use crate::binary::{parse_pkg_index, PkgArchive};
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Error, ErrorKind, Result as IoResult};
use std::path::{Component, Path, PathBuf};

pub struct MappedPkg {
    pub mmap: Mmap,
    pub index: PkgArchive,
}

impl MappedPkg {
    pub fn open(path: &Path) -> IoResult<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let mut cursor = Cursor::new(&mmap[..]);
        let index = parse_pkg_index(&mut cursor)?;
        Ok(Self { mmap, index })
    }

    pub fn read_entry(&self, virtual_path: &str) -> Option<&[u8]> {
        let entry = self.index.entries.get(virtual_path)?;
        let end = entry.offset.checked_add(entry.length)?;
        self.mmap.get(entry.offset..end)
    }
}

pub struct WallpaperVfs {
    pub wallpaper_dir: PathBuf,
    pub scene_pkg: Option<MappedPkg>,
    pub gifscene_pkg: Option<MappedPkg>,
    pub core_assets_dir: PathBuf,
    pub builtin_virtual_files: HashMap<&'static str, &'static [u8]>,
}

impl WallpaperVfs {
    pub fn mount(wallpaper_dir: PathBuf, core_assets_dir: PathBuf) -> Self {
        let scene_pkg = MappedPkg::open(&wallpaper_dir.join("scene.pkg")).ok();
        let gifscene_pkg = MappedPkg::open(&wallpaper_dir.join("gifscene.pkg")).ok();

        let mut builtin_virtual_files: HashMap<&'static str, &'static [u8]> = HashMap::new();
        builtin_virtual_files.insert(
            "shaders/commands/copy.vert",
            b"attribute vec3 a_Position;\nattribute vec2 a_TexCoord;\nvarying vec2 v_TexCoord;\nvoid main() {\n    gl_Position = vec4(a_Position, 1.0);\n    v_TexCoord = a_TexCoord;\n}\n",
        );
        builtin_virtual_files.insert(
            "shaders/commands/copy.frag",
            b"varying vec2 v_TexCoord;\nuniform sampler2D g_Texture0;\nvoid main() {\n    gl_FragColor = texSample2D(g_Texture0, v_TexCoord);\n}\n",
        );
        builtin_virtual_files.insert(
            "__video_host__.html",
            include_bytes!("../../truongpaper-platform/src/video-wallpaper-host.html"),
        );

        Self {
            wallpaper_dir,
            scene_pkg,
            gifscene_pkg,
            core_assets_dir,
            builtin_virtual_files,
        }
    }

    /// Normalizes a Wallpaper Engine virtual path and rejects any `..` traversal component.
    pub fn sanitize_virtual_path(raw: &str) -> IoResult<String> {
        let normalized = raw.trim().trim_start_matches(['/', '\\']).replace('\\', "/");
        for comp in Path::new(&normalized).components() {
            if matches!(
                comp,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            ) {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    format!("Rejected path traversal in VFS: {raw}"),
                ));
            }
        }
        Ok(normalized)
    }

    /// Resolves a virtual path across all 4 VFS layers in strict priority order:
    /// 1. Loose file in `<wallpaper_dir>/<rel>`
    /// 2. `<wallpaper_dir>/scene.pkg`
    /// 3. `<wallpaper_dir>/gifscene.pkg`
    /// 4. `<core_assets_dir>/<rel>` or built-in virtual shaders
    pub fn read_bytes(&self, virtual_path: &str) -> IoResult<Vec<u8>> {
        let rel = Self::sanitize_virtual_path(virtual_path)?;

        // Layer 1: Loose file in wallpaper directory
        let loose_path = self.wallpaper_dir.join(&rel);
        if loose_path.is_file() {
            return std::fs::read(&loose_path);
        }

        // Layer 2: scene.pkg
        if let Some(pkg) = &self.scene_pkg {
            if let Some(slice) = pkg.read_entry(&rel) {
                return Ok(slice.to_vec());
            }
        }

        // Layer 3: gifscene.pkg
        if let Some(pkg) = &self.gifscene_pkg {
            if let Some(slice) = pkg.read_entry(&rel) {
                return Ok(slice.to_vec());
            }
        }

        // Layer 4a: Core Wallpaper Engine assets directory
        let core_path = self.core_assets_dir.join(&rel);
        if core_path.is_file() {
            return std::fs::read(&core_path);
        }

        // Layer 4b: Built-in in-memory virtual files
        if let Some(bytes) = self.builtin_virtual_files.get(rel.as_str()) {
            return Ok(bytes.to_vec());
        }

        Err(Error::new(
            ErrorKind::NotFound,
            format!("VFS file not found across all 4 layers: {rel}"),
        ))
    }

    pub fn read_string(&self, virtual_path: &str) -> IoResult<String> {
        let bytes = self.read_bytes(virtual_path)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}
```

---

## 3. Verified Byte-for-Byte Binary Parsers (`truongpaper-we-formats`)

> [!CAUTION]
> **Critical String Encoding Difference Between `.pkg` and `.tex`**:
> - **`.pkg` files** use **32-bit length-prefixed strings** (`read_string_i32_size`: read `u32` byte count $N$, then read $N$ UTF-8 bytes, **no null terminator**).
> - **`.tex` files** use **null-terminated C strings** (`read_n_string`: read bytes until `0x00` byte).
> Mixing these two up will immediately corrupt stream offsets!

### 3.1 Complete Rust `.pkg` & `.tex` Binary Parser (`crates/truongpaper-we-formats/src/binary.rs`)

```rust
use std::collections::HashMap;
use std::io::{Cursor, Error, ErrorKind, Read, Result as IoResult, Seek, SeekFrom};

pub trait WeBinaryExt: Read + Seek {
    fn read_u32_le(&mut self) -> IoResult<u32> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    fn read_i32_le(&mut self) -> IoResult<i32> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(i32::from_le_bytes(b))
    }

    fn read_f32_le(&mut self) -> IoResult<f32> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(f32::from_le_bytes(b))
    }

    /// Used by .pkg files: [i32 length][utf8 bytes] (NO null terminator)
    fn read_string_i32_size(&mut self, max_len: usize) -> IoResult<String> {
        let len = self.read_i32_le()?;
        if len < 0 || (len as usize) > max_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid i32 string length: {len}"),
            ));
        }
        let mut buf = vec![0u8; len as usize];
        self.read_exact(&mut buf)?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// Used by .tex files: null-terminated ASCII/UTF-8 string (e.g. "TEXV0005\0")
    fn read_n_string(&mut self, max_len: usize) -> IoResult<String> {
        let mut buf = Vec::with_capacity(16);
        let mut byte = [0u8; 1];
        while buf.len() < max_len {
            self.read_exact(&mut byte)?;
            if byte[0] == 0 {
                break;
            }
            buf.push(byte[0]);
        }
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }
}

impl<T: Read + Seek> WeBinaryExt for T {}

// ============================================================================
// 1. PKG Archive Reader (PKGV0001 .. PKGV0022+)
// ============================================================================

#[derive(Debug, Clone)]
pub struct PkgEntry {
    pub full_path: String,
    pub offset: usize, // Absolute byte offset inside the .pkg file
    pub length: usize,
}

#[derive(Debug)]
pub struct PkgArchive {
    pub magic: String,
    pub entries: HashMap<String, PkgEntry>,
}

pub fn parse_pkg_index<R: Read + Seek>(reader: &mut R) -> IoResult<PkgArchive> {
    let magic = reader.read_string_i32_size(32)?;
    if !magic.starts_with("PKGV") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Unexpected PKG magic: {magic}"),
        ));
    }

    let entry_count = reader.read_u32_le()? as usize;
    let mut raw_entries = Vec::with_capacity(entry_count);

    for _ in 0..entry_count {
        let full_path = reader.read_string_i32_size(512)?.replace('\\', "/");
        let rel_offset = reader.read_u32_le()? as usize;
        let length = reader.read_u32_le()? as usize;
        raw_entries.push((full_path, rel_offset, length));
    }

    let data_start = reader.stream_position()? as usize;
    let mut entries = HashMap::with_capacity(entry_count);
    for (full_path, rel_offset, length) in raw_entries {
        entries.insert(
            full_path.to_lowercase(),
            PkgEntry {
                full_path,
                offset: data_start + rel_offset,
                length,
            },
        );
    }

    Ok(PkgArchive { magic, entries })
}

// ============================================================================
// 2. TEX Texture Reader (TEXV0005 / TEXI0001 / TEXB0001..0004 / TEXS0001..0003)
// ============================================================================

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexFormat {
    RGBA8888 = 0,
    DXT5 = 4,
    DXT3 = 6,
    DXT1 = 7,
    RG88 = 8,
    R8 = 9,
}

impl TexFormat {
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::RGBA8888),
            4 => Some(Self::DXT5),
            6 => Some(Self::DXT3),
            7 => Some(Self::DXT1),
            8 => Some(Self::RG88),
            9 => Some(Self::R8),
            _ => None,
        }
    }
}

pub mod tex_flags {
    pub const NONE: i32 = 0;
    pub const NO_INTERPOLATION: i32 = 1; // Use Nearest filter
    pub const CLAMP_UVS: i32 = 2;        // Use ClampToEdge address mode
    pub const IS_GIF: i32 = 4;           // Has trailing TEXS frame info block
    pub const IS_VIDEO_TEXTURE: i32 = 32;
}

#[derive(Debug, Clone)]
pub struct TexHeader {
    pub format: TexFormat,
    pub flags: i32,
    pub texture_width: i32,  // Padded power-of-two GPU width
    pub texture_height: i32, // Padded power-of-two GPU height
    pub image_width: i32,    // Actual visible content width (UV max_u = image_width / texture_width)
    pub image_height: i32,   // Actual visible content height (UV max_v = image_height / texture_height)
    pub unk_int0: u32,
}

#[derive(Debug, Clone)]
pub struct TexMipmap {
    pub width: i32,
    pub height: i32,
    pub is_lz4_compressed: bool,
    pub decompressed_bytes_count: i32,
    pub raw_bytes: Vec<u8>, // Decompressed via lz4_flex if is_lz4_compressed == true
}

#[derive(Debug, Clone)]
pub struct TexFrameInfo {
    pub image_id: i32,
    pub frametime: f32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub width_y: f32,
    pub height_x: f32,
    pub height: f32,
}

#[derive(Debug, Clone)]
pub struct ParsedTex {
    pub header: TexHeader,
    /// FreeImageFormat: -1 = raw/DXT pixels, 0 = BMP, 2 = JPEG, 13 = PNG, 25 = GIF, 35 = MP4 Video
    pub free_image_format: i32,
    pub images: Vec<Vec<TexMipmap>>,
    pub sprite_frames: Option<(i32, i32, Vec<TexFrameInfo>)>, // (gif_width, gif_height, frames)
}

pub fn parse_tex(bytes: &[u8]) -> IoResult<ParsedTex> {
    let mut r = Cursor::new(bytes);

    let magic1 = r.read_n_string(16)?;
    if magic1 != "TEXV0005" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Invalid TEX magic1: {magic1}"),
        ));
    }

    let magic2 = r.read_n_string(16)?;
    if magic2 != "TEXI0001" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Invalid TEX magic2: {magic2}"),
        ));
    }

    let format_raw = r.read_i32_le()?;
    let format = TexFormat::from_i32(format_raw).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Unknown TexFormat: {format_raw}"),
        )
    })?;

    let header = TexHeader {
        format,
        flags: r.read_i32_le()?,
        texture_width: r.read_i32_le()?,
        texture_height: r.read_i32_le()?,
        image_width: r.read_i32_le()?,
        image_height: r.read_i32_le()?,
        unk_int0: r.read_u32_le()?,
    };

    let container_magic = r.read_n_string(16)?;
    let image_count = r.read_i32_le()? as usize;

    let mut free_image_format: i32 = -1; // FIF_UNKNOWN
    let mut container_version: u32 = match container_magic.as_str() {
        "TEXB0001" => 1,
        "TEXB0002" => 2,
        "TEXB0003" => {
            free_image_format = r.read_i32_le()?;
            3
        }
        "TEXB0004" => {
            let fmt = r.read_i32_le()?;
            let is_video_mp4 = r.read_i32_le()? == 1;
            free_image_format = if fmt == -1 && is_video_mp4 { 35 } else { fmt };
            // CRITICAL RePKG rule: Version 4 without FIF_MP4 (35) uses Version 3 mipmap layout!
            if free_image_format == 35 { 4 } else { 3 }
        }
        other => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unknown TEXB magic: {other}"),
            ))
        }
    };

    let mut images = Vec::with_capacity(image_count);
    for _ in 0..image_count {
        let mipmap_count = r.read_i32_le()? as usize;
        let mut mipmaps = Vec::with_capacity(mipmap_count);

        for _ in 0..mipmap_count {
            if container_version == 4 {
                let _param1 = r.read_i32_le()?; // == 1
                let _param2 = r.read_i32_le()?; // == 2
                let _condition_json = r.read_n_string(4096)?;
                let _param3 = r.read_i32_le()?; // == 1
            }

            let width = r.read_i32_le()?;
            let height = r.read_i32_le()?;
            let (is_lz4, decompressed_len) = if container_version >= 2 {
                (r.read_i32_le()? == 1, r.read_i32_le()?)
            } else {
                (false, 0)
            };

            let byte_count = r.read_i32_le()? as usize;
            let mut payload = vec![0u8; byte_count];
            r.read_exact(&mut payload)?;

            let raw_bytes = if is_lz4 {
                lz4_flex::block::decompress(&payload, decompressed_len as usize).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                })?
            } else {
                payload
            };

            mipmaps.push(TexMipmap {
                width,
                height,
                is_lz4_compressed: is_lz4,
                decompressed_bytes_count: decompressed_len,
                raw_bytes,
            });
        }
        images.push(mipmaps);
    }

    // Parse animated sprite sheet frames if IsGif flag (4) is set
    let sprite_frames = if (header.flags & tex_flags::IS_GIF) != 0 {
        let texs_magic = r.read_n_string(16)?;
        let frame_count = r.read_i32_le()? as usize;
        let (mut gif_w, mut gif_h) = if texs_magic == "TEXS0003" {
            (r.read_i32_le()?, r.read_i32_le()?)
        } else {
            (0, 0)
        };

        let mut frames = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            let f = if texs_magic == "TEXS0001" {
                TexFrameInfo {
                    image_id: r.read_i32_le()?,
                    frametime: r.read_f32_le()?,
                    x: r.read_i32_le()? as f32,
                    y: r.read_i32_le()? as f32,
                    width: r.read_i32_le()? as f32,
                    width_y: r.read_i32_le()? as f32,
                    height_x: r.read_i32_le()? as f32,
                    height: r.read_i32_le()? as f32,
                }
            } else {
                TexFrameInfo {
                    image_id: r.read_i32_le()?,
                    frametime: r.read_f32_le()?,
                    x: r.read_f32_le()?,
                    y: r.read_f32_le()?,
                    width: r.read_f32_le()?,
                    width_y: r.read_f32_le()?,
                    height_x: r.read_f32_le()?,
                    height: r.read_f32_le()?,
                }
            };
            frames.push(f);
        }
        if (gif_w == 0 || gif_h == 0) && !frames.is_empty() {
            gif_w = frames[0].width as i32;
            gif_h = frames[0].height as i32;
        }
        Some((gif_w, gif_h, frames))
    } else {
        None
    };

    Ok(ParsedTex {
        header,
        free_image_format,
        images,
        sprite_frames,
    })
}

/// Decodes any `TexMipmap` (`DXT1`, `DXT3`, `DXT5`, `RG88`, `R8`, `RGBA8888`, or embedded FreeImage PNG/JPEG/GIF)
/// into tightly packed `width * height * 4` `RGBA8888` bytes ready for `wgpu::Queue::write_texture`.
/// This completely avoids `wgpu` BC1/BC2/BC3 4x4 block-alignment panics on odd-sized textures
/// (`mipmap.width % 4 != 0 || mipmap.height % 4 != 0`).
pub fn decode_tex_mipmap_to_rgba8(
    format: TexFormat,
    free_image_format: i32,
    mipmap: &TexMipmap,
) -> IoResult<Vec<u8>> {
    let w = mipmap.width.max(1) as usize;
    let h = mipmap.height.max(1) as usize;

    // 1. Embedded standard image container (PNG / JPEG / BMP / GIF / WEBP) when free_image_format != -1
    if free_image_format != -1 {
        let img = image::load_from_memory(&mipmap.raw_bytes)
            .map_err(|e| Error::new(ErrorKind::InvalidData, format!("FreeImage decode failed: {e}")))?;
        return Ok(img.to_rgba8().into_raw());
    }

    // 2. Native Wallpaper Engine pixel & block-compressed formats
    let mut out = vec![0u8; w * h * 4];
    match format {
        TexFormat::RGBA8888 => {
            let expected = w * h * 4;
            if mipmap.raw_bytes.len() < expected {
                return Err(Error::new(ErrorKind::UnexpectedEof, "Truncated RGBA8888 mipmap"));
            }
            out.copy_from_slice(&mipmap.raw_bytes[..expected]);
        }
        TexFormat::R8 => {
            let expected = w * h;
            if mipmap.raw_bytes.len() < expected {
                return Err(Error::new(ErrorKind::UnexpectedEof, "Truncated R8 mipmap"));
            }
            for (i, &r) in mipmap.raw_bytes[..expected].iter().enumerate() {
                out[i * 4] = r;
                out[i * 4 + 1] = r;
                out[i * 4 + 2] = r;
                out[i * 4 + 3] = r;
            }
        }
        TexFormat::RG88 => {
            let expected = w * h * 2;
            if mipmap.raw_bytes.len() < expected {
                return Err(Error::new(ErrorKind::UnexpectedEof, "Truncated RG88 mipmap"));
            }
            for i in 0..(w * h) {
                let r = mipmap.raw_bytes[i * 2];
                let g = mipmap.raw_bytes[i * 2 + 1];
                out[i * 4] = r;
                out[i * 4 + 1] = r;
                out[i * 4 + 2] = r;
                out[i * 4 + 3] = g;
            }
        }
        TexFormat::DXT1 => {
            decode_bc_blocks(&mipmap.raw_bytes, w, h, 8, &mut out, decode_dxt1_block)?;
        }
        TexFormat::DXT3 => {
            decode_bc_blocks(&mipmap.raw_bytes, w, h, 16, &mut out, decode_dxt3_block)?;
        }
        TexFormat::DXT5 => {
            decode_bc_blocks(&mipmap.raw_bytes, w, h, 16, &mut out, decode_dxt5_block)?;
        }
    }

    Ok(out)
}

fn decode_bc_blocks<F>(
    src: &[u8],
    width: usize,
    height: usize,
    block_bytes: usize,
    out_rgba: &mut [u8],
    decode_block: F,
) -> IoResult<()>
where
    F: Fn(&[u8], &mut [[u8; 4]; 16]),
{
    let blocks_x = (width + 3) / 4;
    let blocks_y = (height + 3) / 4;
    let mut offset = 0usize;
    let mut pixels = [[0u8; 4]; 16];

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            if offset + block_bytes > src.len() {
                return Err(Error::new(ErrorKind::UnexpectedEof, "Truncated DXT/BC mipmap"));
            }
            decode_block(&src[offset..offset + block_bytes], &mut pixels);
            offset += block_bytes;

            for py in 0..4 {
                let y = by * 4 + py;
                if y >= height {
                    continue;
                }
                for px in 0..4 {
                    let x = bx * 4 + px;
                    if x >= width {
                        continue;
                    }
                    let dst_idx = (y * width + x) * 4;
                    out_rgba[dst_idx..dst_idx + 4].copy_from_slice(&pixels[py * 4 + px]);
                }
            }
        }
    }
    Ok(())
}

fn rgb565_to_rgb888(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 0x1F) as u8;
    let g = ((c >> 5) & 0x3F) as u8;
    let b = (c & 0x1F) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}

fn decode_dxt1_block(block: &[u8], out: &mut [[u8; 4]; 16]) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    let rgb0 = rgb565_to_rgb888(c0);
    let rgb1 = rgb565_to_rgb888(c1);

    let mut palette = [[0u8; 4]; 4];
    palette[0] = [rgb0[0], rgb0[1], rgb0[2], 255];
    palette[1] = [rgb1[0], rgb1[1], rgb1[2], 255];
    if c0 > c1 {
        for i in 0..3 {
            palette[2][i] = ((2 * rgb0[i] as u16 + rgb1[i] as u16) / 3) as u8;
            palette[3][i] = ((rgb0[i] as u16 + 2 * rgb1[i] as u16) / 3) as u8;
        }
        palette[2][3] = 255;
        palette[3][3] = 255;
    } else {
        for i in 0..3 {
            palette[2][i] = ((rgb0[i] as u16 + rgb1[i] as u16) / 2) as u8;
        }
        palette[2][3] = 255;
        palette[3] = [0, 0, 0, 0];
    }

    for i in 0..16 {
        let idx = ((bits >> (i * 2)) & 0x03) as usize;
        out[i] = palette[idx];
    }
}

fn decode_dxt3_block(block: &[u8], out: &mut [[u8; 4]; 16]) {
    decode_dxt1_block(&block[8..16], out);
    let alpha_bits = u64::from_le_bytes([
        block[0], block[1], block[2], block[3], block[4], block[5], block[6], block[7],
    ]);
    for i in 0..16 {
        let a4 = ((alpha_bits >> (i * 4)) & 0x0F) as u8;
        out[i][3] = (a4 << 4) | a4;
    }
}

fn decode_dxt5_block(block: &[u8], out: &mut [[u8; 4]; 16]) {
    decode_dxt1_block(&block[8..16], out);
    let a0 = block[0];
    let a1 = block[1];
    let mut alphas = [0u8; 8];
    alphas[0] = a0;
    alphas[1] = a1;
    if a0 > a1 {
        for i in 1..7 {
            alphas[i + 1] = (((7 - i as u16) * a0 as u16 + (i as u16) * a1 as u16) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            alphas[i + 1] = (((5 - i as u16) * a0 as u16 + (i as u16) * a1 as u16) / 5) as u8;
        }
        alphas[6] = 0;
        alphas[7] = 255;
    }

    let mut alpha_indices: u64 = 0;
    for i in 0..6 {
        alpha_indices |= (block[2 + i] as u64) << (i * 8);
    }
    for i in 0..16 {
        let idx = ((alpha_indices >> (i * 3)) & 0x07) as usize;
        out[i][3] = alphas[idx];
    }
}
```

### 3.2 `.mdl` Puppet Mesh Binary Layout & Parser (`MDLV0021` / `MDLV0023`)
Verified directly from `linux-wallpaperengine` (`CImage::loadPuppetMesh`):
- **Magic Header**: 9 bytes (`"MDLV0021\0"` or `"MDLV0023\0"`).
- **Vertex Stride**: **80 bytes per vertex** (`vertex_bytes % 80 == 0`):
  - Bytes `0..12`: `position: [f32; 3]` (`x, y, z`)
  - Bytes `12..28`: `blend_indices: [u32; 4]`
  - Bytes `28..44`: `blend_weights: [f32; 4]`
  - Bytes `72..80`: `uv: [f32; 2]` (`u, v`)
- **Mesh Block Locator**: Scan from offset `9` up to the `"MDLS"` marker for `(header_offset)` where `u32` at `header_offset + 4` is `vertex_bytes` (`> 0` and divisible by `80`), and `u32` at `header_offset + 8 + vertex_bytes` is `index_bytes` (`> 0` and divisible by `6`, i.e. `3 * sizeof(u16)`).
- **Coordinate Transform to Layer Space**:
  For each vertex `(x, y, z)` and layer size `(size_x, size_y)`:
  $$\text{pos}_{\text{layer}} = \left(\frac{\text{size}_x}{2} + x,\; \frac{\text{size}_y}{2} - y,\; z\right)$$

```rust
// crates/truongpaper-we-formats/src/mdl.rs
use bytemuck::{Pod, Zeroable};
use std::io::{Error, ErrorKind, Result as IoResult};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct MdlPuppetVertex {
    pub position: [f32; 3],
    pub blend_indices: [u32; 4],
    pub blend_weights: [f32; 4],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct ParsedMdlMesh {
    pub version: String,
    pub vertices: Vec<MdlPuppetVertex>,
    pub indices: Vec<u16>,
}

pub fn parse_mdl(bytes: &[u8], layer_size: [f32; 2]) -> IoResult<ParsedMdlMesh> {
    if bytes.len() < 24 || !bytes.starts_with(b"MDLV") {
        return Err(Error::new(ErrorKind::InvalidData, "Invalid .mdl magic header"));
    }
    let version = String::from_utf8_lossy(&bytes[..8]).into_owned();

    // Locate the "MDLS" skeleton section limit if present
    let scan_limit = bytes
        .windows(4)
        .position(|w| w == b"MDLS")
        .unwrap_or(bytes.len());

    let mut found_block: Option<(usize, usize, usize)> = None;
    for offset in 9..scan_limit.saturating_sub(16) {
        let v_bytes = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        if v_bytes == 0 || v_bytes % 80 != 0 || v_bytes > scan_limit {
            continue;
        }
        let idx_len_pos = offset + 8 + v_bytes;
        if idx_len_pos + 4 > scan_limit {
            continue;
        }
        let i_bytes = u32::from_le_bytes([
            bytes[idx_len_pos],
            bytes[idx_len_pos + 1],
            bytes[idx_len_pos + 2],
            bytes[idx_len_pos + 3],
        ]) as usize;
        if i_bytes > 0 && i_bytes % 6 == 0 && idx_len_pos + 4 + i_bytes <= scan_limit {
            found_block = Some((offset + 8, v_bytes, i_bytes));
            break;
        }
    }

    let (v_start, v_bytes, i_bytes) = found_block.ok_or_else(|| {
        Error::new(ErrorKind::InvalidData, "Could not locate valid 80-byte stride mesh block in .mdl")
    })?;

    let v_count = v_bytes / 80;
    let mut vertices = Vec::with_capacity(v_count);
    let half_w = layer_size[0] * 0.5;
    let half_h = layer_size[1] * 0.5;

    for i in 0..v_count {
        let b = &bytes[v_start + i * 80..v_start + (i + 1) * 80];
        let x = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let y = f32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        let z = f32::from_le_bytes([b[8], b[9], b[10], b[11]]);
        let bi = [
            u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
            u32::from_le_bytes([b[16], b[17], b[18], b[19]]),
            u32::from_le_bytes([b[20], b[21], b[22], b[23]]),
            u32::from_le_bytes([b[24], b[25], b[26], b[27]]),
        ];
        let bw = [
            f32::from_le_bytes([b[28], b[29], b[30], b[31]]),
            f32::from_le_bytes([b[32], b[33], b[34], b[35]]),
            f32::from_le_bytes([b[36], b[37], b[38], b[39]]),
            f32::from_le_bytes([b[40], b[41], b[42], b[43]]),
        ];
        let u = f32::from_le_bytes([b[72], b[73], b[74], b[75]]);
        let v = f32::from_le_bytes([b[76], b[77], b[78], b[79]]);

        vertices.push(MdlPuppetVertex {
            position: [half_w + x, half_h - y, z],
            blend_indices: bi,
            blend_weights: bw,
            uv: [u, v],
        });
    }

    let i_start = v_start + v_bytes + 4;
    let i_count = i_bytes / 2;
    let mut indices = Vec::with_capacity(i_count);
    for i in 0..i_count {
        let idx = u16::from_le_bytes([bytes[i_start + i * 2], bytes[i_start + i * 2 + 1]]);
        indices.push(idx);
    }

    Ok(ParsedMdlMesh {
        version,
        vertices,
        indices,
    })
}
```

---

## 4. Verified Shader Preprocessor & Transpiler Rules (`truongpaper-renderer`)

### 4.1 Mandatory Preprocessor Header Macros
Every Wallpaper Engine `.vert` and `.frag` shader must be prefixed with these exact compatibility macros (verified from `ShaderUnit.cpp`):

```glsl
#version 330
precision highp float;
#define mul(x, y) ((y) * (x))
#define max(x, y) max(y, x)
#define lerp mix
#define frac fract
#define CAST2(x) (vec2(x))
#define CAST3(x) (vec3(x))
#define CAST4(x) (vec4(x))
#define CAST3X3(x) (mat3(x))
#define float2 vec2
#define float3 vec3
#define float4 vec4
#define int2 ivec2
#define int3 ivec3
#define int4 ivec4
#define saturate(x) (clamp(x, 0.0, 1.0))
#define texSample2D texture
#define texSample2DLod textureLod
#define log10(x) (log2(x) * 0.301029995663981)
#define atan2 atan
#define fmod(x, y) ((x)-(y)*trunc((x)/(y)))
#define ddx dFdx
#define ddy(x) dFdy(-(x))
#define GLSL 1
```
- **Vertex stage defines**: `#define attribute in` and `#define varying out`
- **Fragment stage defines**: `out vec4 out_FragColor;`, `#define varying in`, and replace all occurrences of `gl_FragColor` with `out_FragColor`.

### 4.2 Five Mandatory Shader Compatibility Rewrites
1. **Automatic `TEX0FORMAT` Combo Injection**:
   - If `g_Texture0` has format `RG88` (`TexFormat::RG88 == 8`), inject `#define TEX0FORMAT 8`.
   - If `g_Texture0` has format `R8` (`TexFormat::R8 == 9`), inject `#define TEX0FORMAT 9`.
2. **Sampler Comment Combo Auto-Activation**:
   - When a uniform line declares `uniform sampler2D g_TextureN; // {"combo":"COMBONAME", ...}`, if texture slot `N` is bound in the pass (or has a met `require` condition), automatically inject `#define COMBONAME 1`.
3. **`#require LightingV1` Injection**:
   - Replace `#require LightingV1` with:
     ```glsl
     vec3 PerformLighting_V1(vec3 worldPos, vec3 albedo, vec3 normal, vec3 viewDir,
         vec3 specularTint, vec3 baseReflectance, float roughness, float metallic) {
         return vec3(0.0);
     }
     ```
4. **Linked Varying `vec2` $\to$ `vec4` Upgrade (`applyLinkedVaryingCompatibility`)**:
   - If the fragment shader declares `varying vec4 <name>;` while the vertex shader declares `varying vec2 <name>;`, rewrite the vertex shader declaration to `varying vec4 <name>;` and rewrite assignments `<name> = <expr>;` to `<name> = vec4(<expr>, 0.0, 1.0);`.
5. **Fragment `v_TexCoord` Swizzle Fix (`applyFragmentTexCoordCompatibility`)**:
   - If the fragment shader declares `varying vec4 v_TexCoord;` (or `vec3`) and uses `v_TexCoord + CAST2(...)`, rewrite `v_TexCoord` in binary arithmetic with `CAST2(...)` to `v_TexCoord.xy`.

### 4.3 Complete Built-in Uniform Table (`CPass::setupUniforms`)
Every shader pass must automatically bind any of the following uniforms if present in the compiled shader reflection:

| Uniform Name | GLSL Type | Value / Binding Source |
| :--- | :--- | :--- |
| `g_Time` | `float` | Elapsed seconds since wallpaper start (scaled by playback speed) |
| `g_Daytime` | `float` | Normalized local time of day $[0.0, 1.0]$ (`0.0` = 00:00, `0.5` = 12:00) |
| `g_PointerPosition` | `vec2` | Normalized desktop cursor position $[0.0, 1.0]$ on that monitor |
| `g_PointerPositionLast` | `vec2` | Previous frame cursor position |
| `g_TexelSize` | `vec2` | `(1.0 / scene_width, 1.0 / scene_height)` |
| `g_TexelSizeHalf` | `vec2` | `(0.5 / scene_width, 0.5 / scene_height)` |
| `g_Texture0` .. `g_Texture7` | `sampler2D` | Bound texture units `0..7` |
| `g_Texture0Resolution` .. `g_Texture7Resolution` | `vec4` | `(texture_width, texture_height, image_width, image_height)` |
| `g_Texture0Rotation` | `vec4` | Sprite sheet frame rotation/size `(w1/tw, w2/tw, h2/th, h1/th)` |
| `g_Texture0Translation` | `vec2` | Sprite sheet frame UV offset `(frame.x / tw, frame.y / th)` |
| `g_ModelViewProjectionMatrix` | `mat4` | Pass MVP matrix (also bound to `g_EffectModelViewProjectionMatrix`) |
| `g_ModelViewProjectionMatrixInverse` | `mat4` | Inverse of Pass MVP matrix |
| `g_ModelMatrix` / `g_ModelMatrixInverse` | `mat4` | Model matrix & inverse |
| `g_ViewProjectionMatrix` | `mat4` | Camera View-Projection matrix |
| `g_LightAmbientColor` / `g_LightSkylightColor` | `vec3` | `scene.general.ambientcolor` / `skylightcolor` |
| `g_Alpha` / `g_UserAlpha` / `g_Brightness` | `float` | Layer alpha / brightness |
| `g_Color` / `g_CompositeColor` / `g_Color4` | `vec3` / `vec4` | Layer RGB / RGBA color |
| `g_OrientationUp` / `Right` / `Forward` | `vec3` | `(0,1,0)` / `(1,0,0)` / `(0,0,1)` |
| `g_ViewUp` / `g_ViewRight` / `g_EyePosition` | `vec3` | `(0,1,0)` / `(1,0,0)` / `(0,0,1000)` |
| `g_RenderVar0` | `vec4` | Particle `(trail_length, trail_max_length, trail_min_length, 0.0)` |
| `g_RenderVar1` | `vec4` | Particle `(frame_w, frame_h, num_frames, texture_aspect_ratio)` |
| `g_AudioSpectrum16Left` / `Right` | `float[16]` | 16-band stereo FFT from `truongpaper-audio` |
| `g_AudioSpectrum32Left` / `Right` | `float[32]` | 32-band stereo FFT from `truongpaper-audio` |
| `g_AudioSpectrum64Left` / `Right` | `float[64]` | 64-band stereo FFT from `truongpaper-audio` |

### 4.4 Complete Rust Shader Preprocessor Implementation (`crates/truongpaper-renderer/src/shader_preprocessor.rs`)

```rust
use std::collections::HashMap;
use std::io::Result as IoResult;
use truongpaper_we_formats::binary::TexFormat;
use truongpaper_we_formats::vfs::WallpaperVfs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeShaderStage {
    Vertex,
    Fragment,
}

const WE_GLSL_PREAMBLE: &str = r#"#version 330
precision highp float;
#define mul(x, y) ((y) * (x))
#define max(x, y) max(y, x)
#define lerp mix
#define frac fract
#define CAST2(x) (vec2(x))
#define CAST3(x) (vec3(x))
#define CAST4(x) (vec4(x))
#define CAST3X3(x) (mat3(x))
#define float2 vec2
#define float3 vec3
#define float4 vec4
#define int2 ivec2
#define int3 ivec3
#define int4 ivec4
#define saturate(x) (clamp(x, 0.0, 1.0))
#define texSample2D texture
#define texSample2DLod textureLod
#define log10(x) (log2(x) * 0.301029995663981)
#define atan2 atan
#define fmod(x, y) ((x)-(y)*trunc((x)/(y)))
#define ddx dFdx
#define ddy(x) dFdy(-(x))
#define GLSL 1
"#;

const LIGHTING_V1_STUB: &str = r#"
vec3 PerformLighting_V1(vec3 worldPos, vec3 albedo, vec3 normal, vec3 viewDir,
    vec3 specularTint, vec3 baseReflectance, float roughness, float metallic) {
    return vec3(0.0);
}
"#;

pub fn preprocess_we_shader(
    vfs: &WallpaperVfs,
    shader_virtual_path: &str,
    stage: WeShaderStage,
    pass_combos: &HashMap<String, i32>,
    tex0_format: Option<TexFormat>,
    bound_texture_slots: &[bool; 8],
) -> IoResult<String> {
    let raw_source = vfs.read_string(shader_virtual_path)?;
    let mut combos = pass_combos.clone();

    // Rule 1: Automatic TEX0FORMAT injection for RG88 (8) and R8 (9)
    if let Some(fmt) = tex0_format {
        match fmt {
            TexFormat::RG88 => {
                combos.entry("TEX0FORMAT".into()).or_insert(8);
            }
            TexFormat::R8 => {
                combos.entry("TEX0FORMAT".into()).or_insert(9);
            }
            _ => {}
        }
    }

    // Parse // [COMBO] JSON lines & Rule 2: Sampler comment combo auto-activation
    for line in raw_source.lines() {
        let trimmed = line.trim();
        if let Some(json_str) = trimmed.strip_prefix("// [COMBO]") {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str.trim()) {
                if let (Some(name), Some(def)) = (
                    val.get("combo").and_then(|v| v.as_str()),
                    val.get("default").and_then(|v| v.as_i64()),
                ) {
                    combos.entry(name.to_string()).or_insert(def as i32);
                }
            }
        } else if trimmed.starts_with("uniform sampler2D g_Texture") {
            if let Some((decl, comment)) = trimmed.split_once("//") {
                for slot in 0..8 {
                    if decl.contains(&format!("g_Texture{slot}")) && bound_texture_slots[slot] {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(comment.trim()) {
                            if let Some(combo_name) = val.get("combo").and_then(|v| v.as_str()) {
                                combos.entry(combo_name.to_string()).or_insert(1);
                            }
                        }
                    }
                }
            }
        }
    }

    // Recursively expand #include "..." directives via 4-layer VFS
    let expanded_body = expand_includes_recursive(vfs, &raw_source, 0)?;

    let mut out = String::with_capacity(expanded_body.len() + 2048);
    out.push_str(WE_GLSL_PREAMBLE);

    for (k, v) in &combos {
        out.push_str(&format!("#define {k} {v}\n"));
    }

    match stage {
        WeShaderStage::Vertex => {
            out.push_str("#define attribute in\n#define varying out\n");
        }
        WeShaderStage::Fragment => {
            out.push_str("#define varying in\nout vec4 out_FragColor;\n");
        }
    }

    // Rule 3: Replace #require LightingV1 and gl_FragColor -> out_FragColor
    for line in expanded_body.lines() {
        if line.trim().starts_with("#require LightingV1") {
            out.push_str(LIGHTING_V1_STUB);
            out.push('\n');
        } else if stage == WeShaderStage::Fragment {
            out.push_str(&line.replace("gl_FragColor", "out_FragColor"));
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }

    Ok(out)
}

fn expand_includes_recursive(
    vfs: &WallpaperVfs,
    source: &str,
    depth: usize,
) -> IoResult<String> {
    if depth > 16 {
        return Ok(source.to_string());
    }
    let mut result = String::with_capacity(source.len());
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#include") {
            let inc_path = rest.trim().trim_matches('"').trim_matches('<').trim_matches('>');
            let full_inc = if inc_path.starts_with("shaders/") {
                inc_path.to_string()
            } else {
                format!("shaders/{inc_path}")
            };
            if let Ok(inc_content) = vfs.read_string(&full_inc) {
                let expanded = expand_includes_recursive(vfs, &inc_content, depth + 1)?;
                result.push_str(&expanded);
                result.push('\n');
                continue;
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    Ok(result)
}

/// Rule 4: Upgrades `varying vec2 <name>;` in the vertex shader to `varying vec4 <name>;`
/// whenever the fragment shader declares `varying vec4 <name>;`, and wraps bare assignments
/// `<name> = <rhs>;` into `<name> = vec4(<rhs>, 0.0, 1.0);`.
pub fn apply_linked_varying_compatibility(vert_src: &str, frag_src: &str) -> String {
    let mut frag_vec4_varyings = Vec::new();
    for line in frag_src.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("varying vec4 ")
            .or_else(|| trimmed.strip_prefix("in vec4 "))
        {
            if let Some(name) = rest.trim_end_matches(';').split_whitespace().next() {
                frag_vec4_varyings.push(name.to_string());
            }
        }
    }

    if frag_vec4_varyings.is_empty() {
        return vert_src.to_string();
    }

    let mut upgraded_names = Vec::new();
    let mut out_lines = Vec::new();

    for line in vert_src.lines() {
        let trimmed = line.trim();
        let mut replaced_decl = false;
        for var_name in &frag_vec4_varyings {
            let decl_varying = format!("varying vec2 {var_name};");
            let decl_out = format!("out vec2 {var_name};");
            if trimmed == decl_varying {
                out_lines.push(line.replace(&decl_varying, &format!("varying vec4 {var_name};")));
                upgraded_names.push(var_name.clone());
                replaced_decl = true;
                break;
            } else if trimmed == decl_out {
                out_lines.push(line.replace(&decl_out, &format!("out vec4 {var_name};")));
                upgraded_names.push(var_name.clone());
                replaced_decl = true;
                break;
            }
        }
        if replaced_decl {
            continue;
        }

        let mut modified_line = line.to_string();
        for var_name in &upgraded_names {
            if let Some(eq_idx) = trimmed.find('=') {
                let lhs = trimmed[..eq_idx].trim();
                let rhs = trimmed[eq_idx + 1..].trim().trim_end_matches(';').trim();
                if lhs == var_name && !rhs.starts_with("vec4(") && !rhs.starts_with("CAST4(") {
                    modified_line = format!("    {var_name} = vec4({rhs}, 0.0, 1.0);");
                    break;
                }
            }
        }
        out_lines.push(modified_line);
    }

    out_lines.join("\n")
}

/// Rule 5: When the fragment shader declares `varying vec4 v_TexCoord;` (or `vec3`)
/// and performs `v_TexCoord + CAST2(...)`, rewrites bare `v_TexCoord` in binary vec2 arithmetic to `v_TexCoord.xy`.
pub fn apply_fragment_texcoord_compatibility(frag_src: &str) -> String {
    let is_wide_texcoord = frag_src.lines().any(|l| {
        let t = l.trim();
        t.starts_with("varying vec4 v_TexCoord")
            || t.starts_with("varying vec3 v_TexCoord")
            || t.starts_with("in vec4 v_TexCoord")
            || t.starts_with("in vec3 v_TexCoord")
    });

    if !is_wide_texcoord {
        return frag_src.to_string();
    }

    frag_src
        .replace("v_TexCoord + CAST2(", "v_TexCoord.xy + CAST2(")
        .replace("v_TexCoord - CAST2(", "v_TexCoord.xy - CAST2(")
        .replace("v_TexCoord * CAST2(", "v_TexCoord.xy * CAST2(")
        .replace("v_TexCoord + vec2(", "v_TexCoord.xy + vec2(")
        .replace("v_TexCoord - vec2(", "v_TexCoord.xy - vec2(")
}

#[derive(Debug, Clone)]
pub struct ReflectedUniformField {
    pub name: String,
    pub glsl_type: String,
    pub byte_offset: usize,
    pub byte_size: usize,
}

pub struct CompiledWeShaderPair {
    pub vertex_module: wgpu::ShaderModule,
    pub fragment_module: wgpu::ShaderModule,
    pub uniform_fields: Vec<ReflectedUniformField>,
    pub uniform_buffer_size: usize,
}

/// Assigns deterministic locations, packs loose non-sampler uniforms into a std140 UBO at
/// (set = 0, binding = 0), assigns sampler2D slots to (set = 1, binding = N), and compiles
/// both shaders via `naga::front::glsl::Frontend` into `wgpu::ShaderModule`s.
pub fn compile_we_shader_pair(
    device: &wgpu::Device,
    label: &str,
    raw_vert_preprocessed: &str,
    raw_frag_preprocessed: &str,
) -> Result<CompiledWeShaderPair, String> {
    let linked_vert = apply_linked_varying_compatibility(raw_vert_preprocessed, raw_frag_preprocessed);
    let linked_frag = apply_fragment_texcoord_compatibility(raw_frag_preprocessed);

    let (vert_450, frag_450, uniform_fields, ubo_size) =
        upgrade_to_naga_glsl450(&linked_vert, &linked_frag);

    let mut frontend = naga::front::glsl::Frontend::default();

    let vert_naga = frontend
        .parse(
            &naga::front::glsl::Options::from(naga::ShaderStage::Vertex),
            &vert_450,
        )
        .map_err(|errs| format!("Vertex GLSL parse error in {label}: {errs:?}"))?;

    let frag_naga = frontend
        .parse(
            &naga::front::glsl::Options::from(naga::ShaderStage::Fragment),
            &frag_450,
        )
        .map_err(|errs| format!("Fragment GLSL parse error in {label}: {errs:?}"))?;

    let vertex_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&format!("{label}_vert")),
        source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(vert_naga)),
    });

    let fragment_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&format!("{label}_frag")),
        source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(frag_naga)),
    });

    Ok(CompiledWeShaderPair {
        vertex_module,
        fragment_module,
        uniform_fields,
        uniform_buffer_size: ubo_size.max(16),
    })
}

fn upgrade_to_naga_glsl450(
    vert_src: &str,
    frag_src: &str,
) -> (String, String, Vec<ReflectedUniformField>, usize) {
    let mut uniform_fields: Vec<ReflectedUniformField> = Vec::new();
    let mut current_offset = 0usize;
    let mut varying_locations: HashMap<String, u32> = HashMap::new();
    let mut next_varying_loc = 0u32;

    for src in [vert_src, frag_src] {
        for line in src.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("uniform ") {
                let decl = rest.split("//").next().unwrap_or(rest).trim().trim_end_matches(';');
                let parts: Vec<&str> = decl.split_whitespace().collect();
                if parts.len() >= 2 && parts[0] != "sampler2D" {
                    let ty = parts[0].to_string();
                    let raw_name = parts[1].split('=').next().unwrap().trim().to_string();
                    let clean_name = raw_name.split('[').next().unwrap().to_string();
                    if !uniform_fields.iter().any(|f| f.name == clean_name) {
                        let (align, size) = std140_align_and_size(&ty, &raw_name);
                        current_offset = (current_offset + align - 1) & !(align - 1);
                        uniform_fields.push(ReflectedUniformField {
                            name: raw_name,
                            glsl_type: ty,
                            byte_offset: current_offset,
                            byte_size: size,
                        });
                        current_offset += size;
                    }
                }
            }
        }
    }
    let total_ubo_size = (current_offset + 15) & !15;

    let rewrite_stage = |src: &str,
                         is_vert: bool,
                         varying_locs: &mut HashMap<String, u32>,
                         next_loc: &mut u32|
     -> String {
        let mut out = String::from("#version 450\n");
        if !uniform_fields.is_empty() {
            out.push_str("layout(std140, set = 0, binding = 0) uniform WePassUniformBlock {\n");
            for f in &uniform_fields {
                out.push_str(&format!("    {} {};\n", f.glsl_type, f.name));
            }
            out.push_str("};\n");
        }

        let mut attr_loc = 0u32;
        for line in src.lines() {
            let t = line.trim();
            if t.starts_with("#version")
                || t == "#define attribute in"
                || t == "#define varying out"
                || t == "#define varying in"
            {
                continue;
            }
            if t == "out vec4 out_FragColor;" {
                out.push_str("layout(location = 0) out vec4 out_FragColor;\n");
                continue;
            }
            if let Some(rest) = t.strip_prefix("attribute ") {
                out.push_str(&format!("layout(location = {attr_loc}) in {rest}\n"));
                attr_loc += 1;
                continue;
            }
            if let Some(rest) = t.strip_prefix("varying ") {
                let decl = rest.trim_end_matches(';');
                let var_name = decl.split_whitespace().last().unwrap_or("").to_string();
                let loc = *varying_locs.entry(var_name).or_insert_with(|| {
                    let l = *next_loc;
                    *next_loc += 1;
                    l
                });
                let qualifier = if is_vert { "out" } else { "in" };
                out.push_str(&format!("layout(location = {loc}) {qualifier} {rest}\n"));
                continue;
            }
            if let Some(rest) = t.strip_prefix("uniform ") {
                let decl = rest.split("//").next().unwrap_or(rest).trim();
                if decl.starts_with("sampler2D ") {
                    let slot = (0..8)
                        .find(|s| decl.contains(&format!("g_Texture{s}")))
                        .unwrap_or(0);
                    out.push_str(&format!(
                        "layout(set = 1, binding = {}) uniform {}\n",
                        slot, decl
                    ));
                }
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    };

    let v450 = rewrite_stage(vert_src, true, &mut varying_locations, &mut next_varying_loc);
    let f450 = rewrite_stage(frag_src, false, &mut varying_locations, &mut next_varying_loc);
    (v450, f450, uniform_fields, total_ubo_size)
}

fn std140_align_and_size(glsl_type: &str, raw_name: &str) -> (usize, usize) {
    if let Some(bracket_idx) = raw_name.find('[') {
        let len_str = raw_name[bracket_idx + 1..].trim_end_matches(']');
        let count = len_str.parse::<usize>().unwrap_or(1);
        return (16, 16 * count);
    }
    match glsl_type {
        "float" | "int" | "uint" | "bool" => (4, 4),
        "vec2" | "ivec2" | "float2" => (8, 8),
        "vec3" | "ivec3" | "float3" => (16, 12),
        "vec4" | "ivec4" | "float4" => (16, 16),
        "mat3" => (16, 48),
        "mat4" => (16, 64),
        _ => (16, 16),
    }
}
```

---

## 5. Exact Particle Vertex Layouts (`CParticle.cpp`)

### 5.1 Sprite Particle Vertex Buffer (`17 floats = 68 bytes/vertex`)
Used with `genericparticle.vert` (`THICKFORMAT = 1`):

| Attribute Name | Components | Byte Offset | Values |
| :--- | :--- | :--- | :--- |
| `a_Position` | `vec3` (`3 × f32`) | `0` | `(p.position.x, p.position.y, p.position.z)` |
| `a_TexCoordVec4` | `vec4` (`4 × f32`) | `12` | `(u, v, p.rotation.z, p.size)` |
| `a_Color` | `vec4` (`4 × f32`) | `28` | `(p.color.r, p.color.g, p.color.b, p.alpha)` |
| `a_TexCoordVec4C1` | `vec4` (`4 × f32`) | `44` | `(p.velocity.x, p.velocity.y, p.velocity.z, lifetime_or_frame)` |
| `a_TexCoordC2` | `vec2` (`2 × f32`) | `60` | `(p.rotation.x, p.rotation.y)` |

### 5.2 Rope / Trail Particle Vertex Buffer (`26 floats = 104 bytes/vertex`)
Used with `genericropeparticle.vert` (`THICKFORMAT = 1`):

| Attribute Name | Components | Byte Offset | Values |
| :--- | :--- | :--- | :--- |
| `a_PositionVec4` | `vec4` (`4 × f32`) | `0` | `(pos_start.x, pos_start.y, pos_start.z, size_start)` |
| `a_TexCoordVec4` | `vec4` (`4 × f32`) | `16` | `(pos_end.x, pos_end.y, pos_end.z, trail_length)` |
| `a_TexCoordVec4C1` | `vec4` (`4 × f32`) | `32` | `(pos_prev.x, pos_prev.y, pos_prev.z, trail_position)` |
| `a_TexCoordVec4C2` | `vec4` (`4 × f32`) | `48` | `(pos_after.x, pos_after.y, pos_after.z, size_end)` |
| `a_TexCoordVec4C3` | `vec4` (`4 × f32`) | `64` | `(color_end.r, color_end.g, color_end.b, color_end.a)` |
| `a_TexCoordC4` | `vec2` (`2 × f32`) | `80` | `(uv_x, uv_y)` |
| `a_Color` | `vec4` (`4 × f32`) | `88` | `(color_start.r, color_start.g, color_start.b, color_start.a)` |

### 5.3 Complete CPU Particle Simulation Engine & Vertex Builders (`crates/truongpaper-renderer/src/particles.rs`)

```rust
// crates/truongpaper-renderer/src/particles.rs
use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3, Vec4};
use serde::{Deserialize, Serialize};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct SpriteParticleVertex {
    pub position: [f32; 3],          // offset 0..12
    pub texcoord_rot_size: [f32; 4], // offset 12..28: (u, v, rotation.z, size)
    pub color: [f32; 4],             // offset 28..44: (r, g, b, a)
    pub velocity_life: [f32; 4],     // offset 44..60: (vx, vy, vz, normalized_age)
    pub rotation_xy: [f32; 2],       // offset 60..68: (rotation.x, rotation.y)
}
const _: () = assert!(std::mem::size_of::<SpriteParticleVertex>() == 68);

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct RopeParticleVertex {
    pub pos_start_size: [f32; 4],     // offset 0..16:  (start.xyz, size_start)
    pub pos_end_len: [f32; 4],        // offset 16..32: (end.xyz, trail_length)
    pub pos_prev_tpos: [f32; 4],      // offset 32..48: (prev.xyz, trail_position)
    pub pos_after_size_end: [f32; 4], // offset 48..64: (after.xyz, size_end)
    pub color_end: [f32; 4],          // offset 64..80: (r, g, b, a)
    pub uv: [f32; 2],                 // offset 80..88: (uv_x, uv_y)
    pub color_start: [f32; 4],        // offset 88..104: (r, g, b, a)
}
const _: () = assert!(std::mem::size_of::<RopeParticleVertex>() == 104);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticleFile {
    #[serde(default)]
    pub maxcount: u32,
    #[serde(default)]
    pub starttime: f32,
    #[serde(default)]
    pub material: String,
    #[serde(default)]
    pub emitter: Vec<ParticleEmitterSpec>,
    #[serde(default)]
    pub initializer: Vec<serde_json::Value>,
    #[serde(default)]
    pub operator: Vec<serde_json::Value>,
    #[serde(default)]
    pub renderer: Vec<ParticleRendererSpec>,
    #[serde(default)]
    pub controlpoint: Vec<ParticleControlPointSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticleEmitterSpec {
    pub name: String,
    #[serde(default = "default_rate")]
    pub rate: f32,
    #[serde(default)]
    pub instantaneous: u32,
    #[serde(default = "default_one_vec3")]
    pub directions: String,
    #[serde(default)]
    pub distancemin: serde_json::Value,
    #[serde(default)]
    pub distancemax: serde_json::Value,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub controlpoint: usize,
}
fn default_rate() -> f32 { 10.0 }
fn default_one_vec3() -> String { "1 1 1".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticleRendererSpec {
    pub name: String,
    #[serde(default = "default_one_f32")]
    pub length: f32,
    #[serde(default = "default_max_len")]
    pub maxlength: f32,
    #[serde(default)]
    pub subdivision: u32,
}
fn default_one_f32() -> f32 { 1.0 }
fn default_max_len() -> f32 { 10.0 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticleControlPointSpec {
    pub id: usize,
    #[serde(default)]
    pub offset: String,
    #[serde(default)]
    pub flags: u32, // bit 0 (1) = link to mouse cursor
    #[serde(default)]
    pub locktopointer: bool,
}

#[derive(Debug, Clone)]
pub struct SingleParticle {
    pub position: Vec3,
    pub initial_position: Vec3,
    pub velocity: Vec3,
    pub rotation: Vec3,
    pub angular_velocity: Vec3,
    pub color: Vec3,
    pub initial_color: Vec3,
    pub alpha: f32,
    pub initial_alpha: f32,
    pub size: f32,
    pub initial_size: f32,
    pub age: f32,
    pub lifetime: f32,
    pub noise_seed: f32,
    pub alive: bool,
}

pub struct ParticleSystemInstance {
    pub spec: ParticleFile,
    pub particles: Vec<SingleParticle>,
    pub emit_accumulators: Vec<f32>,
    pub control_points: [Vec3; 8],
    pub rng_state: u64,
    pub elapsed_time: f32,
    pub spawned_instantaneous: bool,
}

impl ParticleSystemInstance {
    pub fn new(spec: ParticleFile) -> Self {
        let max_count = spec.maxcount.clamp(1, 10_000) as usize;
        let mut control_points = [Vec3::ZERO; 8];
        for cp in &spec.controlpoint {
            if cp.id < 8 {
                control_points[cp.id] = parse_vec3_str(&cp.offset, Vec3::ZERO);
            }
        }
        let mut inst = Self {
            emit_accumulators: vec![0.0; spec.emitter.len()],
            particles: Vec::with_capacity(max_count),
            control_points,
            rng_state: 0x853C_49E6_748F_EA9B,
            elapsed_time: 0.0,
            spawned_instantaneous: false,
            spec,
        };
        let prewarm = inst.spec.starttime.min(10.0);
        if prewarm > 0.0 {
            let steps = (prewarm / 0.05).ceil() as usize;
            for _ in 0..steps {
                inst.step(0.05, Vec3::ZERO);
            }
        }
        inst
    }

    #[inline]
    fn rand01(&mut self) -> f32 {
        let old = self.rng_state;
        self.rng_state = old
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        let r = xorshifted.rotate_right(rot);
        (r as f32) / (u32::MAX as f32)
    }

    #[inline]
    fn rand_range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.rand01()
    }

    pub fn step(&mut self, dt: f32, cursor_layer_pos: Vec3) {
        if dt <= 0.0 {
            return;
        }
        self.elapsed_time += dt;

        // 1. Update cursor-bound control points
        for cp in &self.spec.controlpoint {
            if cp.id < 8 && (cp.locktopointer || (cp.flags & 1) != 0) {
                self.control_points[cp.id] =
                    cursor_layer_pos + parse_vec3_str(&cp.offset, Vec3::ZERO);
            }
        }

        // 2. Spawn new particles from emitters
        let max_count = self.spec.maxcount.clamp(1, 10_000) as usize;
        for (e_idx, emitter) in self.spec.emitter.clone().iter().enumerate() {
            let mut to_spawn = 0usize;
            if !self.spawned_instantaneous && emitter.instantaneous > 0 {
                to_spawn += emitter.instantaneous as usize;
            }
            self.emit_accumulators[e_idx] += emitter.rate * dt;
            let periodic = self.emit_accumulators[e_idx].floor() as usize;
            self.emit_accumulators[e_idx] -= periodic as f32;
            to_spawn += periodic;

            for _ in 0..to_spawn {
                if self.particles.len() >= max_count {
                    break;
                }
                let p = self.spawn_one_particle(emitter);
                self.particles.push(p);
            }
        }
        self.spawned_instantaneous = true;

        // 3. Run operators and integrate active particles
        let operators = self.spec.operator.clone();
        let cps = self.control_points;
        let time = self.elapsed_time;

        self.particles.retain_mut(|p| {
            p.age += dt;
            if p.age >= p.lifetime {
                return false;
            }
            let life_frac = (p.age / p.lifetime.max(0.0001)).clamp(0.0, 1.0);

            for op in &operators {
                let op_name = op.get("name").and_then(|v| v.as_str()).unwrap_or("");
                match op_name {
                    "movement" => {
                        let gravity = parse_vec3_json(op.get("gravity"), Vec3::ZERO);
                        let drag = op.get("drag").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        p.velocity += gravity * dt;
                        p.velocity *= (1.0 - drag * dt).max(0.0);
                        p.position += p.velocity * dt;
                    }
                    "angularmovement" => {
                        let drag = op.get("drag").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let force = parse_vec3_json(op.get("force"), Vec3::ZERO);
                        p.angular_velocity += force * dt;
                        p.angular_velocity *= (1.0 - drag * dt).max(0.0);
                        p.rotation += p.angular_velocity * dt;
                    }
                    "alphafade" => {
                        let fade_in = op.get("fadeintime").and_then(|v| v.as_f64()).unwrap_or(0.1) as f32;
                        let fade_out = op.get("fadeouttime").and_then(|v| v.as_f64()).unwrap_or(0.8) as f32;
                        let factor = if life_frac < fade_in && fade_in > 0.0 {
                            life_frac / fade_in
                        } else if life_frac > fade_out && fade_out < 1.0 {
                            1.0 - (life_frac - fade_out) / (1.0 - fade_out)
                        } else {
                            1.0
                        };
                        p.alpha = p.initial_alpha * factor.clamp(0.0, 1.0);
                    }
                    "sizechange" => {
                        let st = op.get("starttime").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let et = op.get("endtime").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                        let sv = op.get("startvalue").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                        let ev = op.get("endvalue").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let t = ((life_frac - st) / (et - st).max(0.0001)).clamp(0.0, 1.0);
                        p.size = p.initial_size * (sv + (ev - sv) * t);
                    }
                    "colorchange" => {
                        let st = op.get("starttime").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let et = op.get("endtime").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                        let sv = parse_vec3_json(op.get("startvalue"), Vec3::ONE);
                        let ev = parse_vec3_json(op.get("endvalue"), Vec3::ONE);
                        let t = ((life_frac - st) / (et - st).max(0.0001)).clamp(0.0, 1.0);
                        p.color = p.initial_color * sv.lerp(ev, t);
                    }
                    "oscillateposition" => {
                        let freq = op.get("frequencymax").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                        let scale = op.get("scalemax").and_then(|v| v.as_f64()).unwrap_or(5.0) as f32;
                        let mask = parse_vec3_json(op.get("mask"), Vec3::new(1.0, 1.0, 0.0));
                        let wave = (time * freq + p.noise_seed * 6.2831853).sin() * scale;
                        p.position += mask * wave * dt;
                    }
                    "turbulence" => {
                        let speed = op.get("speedmax").and_then(|v| v.as_f64()).unwrap_or(50.0) as f32;
                        let scale = op.get("scale").and_then(|v| v.as_f64()).unwrap_or(0.01) as f32;
                        let mask = parse_vec3_json(op.get("mask"), Vec3::new(1.0, 1.0, 0.0));
                        let nx = ((p.position.y * scale + time + p.noise_seed).sin()
                            + (p.position.x * scale * 0.7 - time).cos())
                            * 0.5;
                        let ny = ((p.position.x * scale + time * 1.3 + p.noise_seed).cos()
                            - (p.position.y * scale * 0.7 + time).sin())
                            * 0.5;
                        p.velocity += Vec3::new(nx, ny, 0.0) * mask * speed * dt;
                    }
                    "vortex" => {
                        let cp_id = op.get("controlpoint").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                        let center = cps.get(cp_id).copied().unwrap_or(Vec3::ZERO)
                            + parse_vec3_json(op.get("offset"), Vec3::ZERO);
                        let axis = parse_vec3_json(op.get("axis"), Vec3::Z).normalize_or_zero();
                        let r_vec = p.position - center;
                        let dist = r_vec.length().max(0.001);
                        let inner = op.get("distanceinner").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let outer = op.get("distanceouter").and_then(|v| v.as_f64()).unwrap_or(250.0) as f32;
                        let s_in = op.get("speedinner").and_then(|v| v.as_f64()).unwrap_or(100.0) as f32;
                        let s_out = op.get("speedouter").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let t = ((dist - inner) / (outer - inner).max(0.001)).clamp(0.0, 1.0);
                        let speed = s_in + (s_out - s_in) * t;
                        let tangent = axis.cross(r_vec / dist).normalize_or_zero();
                        p.velocity += tangent * speed * dt;
                    }
                    "controlpointattract" => {
                        let cp_id = op.get("controlpoint").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                        let center = cps.get(cp_id).copied().unwrap_or(Vec3::ZERO)
                            + parse_vec3_json(op.get("origin"), Vec3::ZERO);
                        let scale = op.get("scale").and_then(|v| v.as_f64()).unwrap_or(200.0) as f32;
                        let threshold = op.get("threshold").and_then(|v| v.as_f64()).unwrap_or(512.0) as f32;
                        let to_cp = center - p.position;
                        let dist = to_cp.length();
                        if dist > 0.001 && dist < threshold {
                            let falloff = 1.0 - (dist / threshold);
                            p.velocity += (to_cp / dist) * scale * falloff * dt;
                        }
                    }
                    _ => {}
                }
            }
            true
        });
    }

    fn spawn_one_particle(&mut self, emitter: &ParticleEmitterSpec) -> SingleParticle {
        let cp_pos = self
            .control_points
            .get(emitter.controlpoint)
            .copied()
            .unwrap_or(Vec3::ZERO);
        let origin = cp_pos + parse_vec3_str(&emitter.origin, Vec3::ZERO);
        let dirs = parse_vec3_str(&emitter.directions, Vec3::ONE);

        let pos = if emitter.name == "boxrandom" {
            let dmax = parse_vec3_json(Some(&emitter.distancemax), Vec3::splat(100.0));
            let rx = self.rand_range(-dmax.x, dmax.x);
            let ry = self.rand_range(-dmax.y, dmax.y);
            let rz = self.rand_range(-dmax.z, dmax.z);
            origin + Vec3::new(rx * dirs.x, ry * dirs.y, rz * dirs.z)
        } else {
            let r_min = emitter.distancemin.as_f64().unwrap_or(0.0) as f32;
            let r_max = emitter.distancemax.as_f64().unwrap_or(100.0) as f32;
            let radius = self.rand_range(r_min, r_max);
            let theta = self.rand_range(0.0, std::f32::consts::TAU);
            let phi = self.rand_range(-1.0, 1.0).acos();
            let dir = Vec3::new(
                phi.sin() * theta.cos() * dirs.x,
                phi.sin() * theta.sin() * dirs.y,
                phi.cos() * dirs.z,
            );
            origin + dir * radius
        };

        let mut p = SingleParticle {
            position: pos,
            initial_position: pos,
            velocity: Vec3::ZERO,
            rotation: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            color: Vec3::ONE,
            initial_color: Vec3::ONE,
            alpha: 1.0,
            initial_alpha: 1.0,
            size: 20.0,
            initial_size: 20.0,
            age: 0.0,
            lifetime: 5.0,
            noise_seed: self.rand01(),
            alive: true,
        };

        for init in &self.spec.initializer.clone() {
            let name = init.get("name").and_then(|v| v.as_str()).unwrap_or("");
            match name {
                "lifetimerandom" => {
                    let min = init.get("min").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                    let max = init.get("max").and_then(|v| v.as_f64()).unwrap_or(5.0) as f32;
                    p.lifetime = self.rand_range(min, max).max(0.05);
                }
                "sizerandom" => {
                    let min = init.get("min").and_then(|v| v.as_f64()).unwrap_or(10.0) as f32;
                    let max = init.get("max").and_then(|v| v.as_f64()).unwrap_or(30.0) as f32;
                    p.size = self.rand_range(min, max);
                    p.initial_size = p.size;
                }
                "alpharandom" => {
                    let min = init.get("min").and_then(|v| v.as_f64()).unwrap_or(0.5) as f32;
                    let max = init.get("max").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                    p.alpha = self.rand_range(min, max);
                    p.initial_alpha = p.alpha;
                }
                "colorrandom" => {
                    let cmin = parse_vec3_json(init.get("min"), Vec3::ZERO) / 255.0;
                    let cmax = parse_vec3_json(init.get("max"), Vec3::splat(255.0)) / 255.0;
                    let t = self.rand01();
                    p.color = cmin.lerp(cmax, t);
                    p.initial_color = p.color;
                }
                "velocityrandom" => {
                    let vmin = parse_vec3_json(init.get("min"), Vec3::splat(-20.0));
                    let vmax = parse_vec3_json(init.get("max"), Vec3::splat(20.0));
                    p.velocity = Vec3::new(
                        self.rand_range(vmin.x, vmax.x),
                        self.rand_range(vmin.y, vmax.y),
                        self.rand_range(vmin.z, vmax.z),
                    );
                }
                "rotationrandom" => {
                    let rmin = parse_vec3_json(init.get("min"), Vec3::ZERO);
                    let rmax = parse_vec3_json(init.get("max"), Vec3::new(0.0, 0.0, 6.2831853));
                    p.rotation = Vec3::new(
                        self.rand_range(rmin.x, rmax.x),
                        self.rand_range(rmin.y, rmax.y),
                        self.rand_range(rmin.z, rmax.z),
                    );
                }
                "angularvelocityrandom" => {
                    let amin = parse_vec3_json(init.get("min"), Vec3::ZERO);
                    let amax = parse_vec3_json(init.get("max"), Vec3::new(0.0, 0.0, 1.0));
                    p.angular_velocity = Vec3::new(
                        self.rand_range(amin.x, amax.x),
                        self.rand_range(amin.y, amax.y),
                        self.rand_range(amin.z, amax.z),
                    );
                }
                _ => {}
            }
        }

        p
    }

    /// Emits 4 vertices (68 bytes each) and 6 indices per alive particle for `genericparticle.vert`.
    pub fn build_sprite_vertices(&self) -> (Vec<SpriteParticleVertex>, Vec<u16>) {
        let count = self.particles.len().min((u16::MAX as usize) / 4);
        let mut verts = Vec::with_capacity(count * 4);
        let mut indices = Vec::with_capacity(count * 6);

        let uvs = [[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];

        for (i, p) in self.particles.iter().take(count).enumerate() {
            let base = (i * 4) as u16;
            let norm_age = (p.age / p.lifetime.max(0.0001)).clamp(0.0, 1.0);
            for uv in uvs {
                verts.push(SpriteParticleVertex {
                    position: p.position.to_array(),
                    texcoord_rot_size: [uv[0], uv[1], p.rotation.z, p.size * 0.5],
                    color: [p.color.x, p.color.y, p.color.z, p.alpha],
                    velocity_life: [p.velocity.x, p.velocity.y, p.velocity.z, norm_age],
                    rotation_xy: [p.rotation.x, p.rotation.y],
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        (verts, indices)
    }

    /// Emits 4 vertices (104 bytes each) and 6 indices per segment for `genericropeparticle.vert`.
    pub fn build_rope_vertices(&self) -> (Vec<RopeParticleVertex>, Vec<u16>) {
        if self.particles.len() < 2 {
            return (Vec::new(), Vec::new());
        }
        let seg_count = (self.particles.len() - 1).min((u16::MAX as usize) / 4);
        let mut verts = Vec::with_capacity(seg_count * 4);
        let mut indices = Vec::with_capacity(seg_count * 6);
        let total = seg_count as f32;

        for i in 0..seg_count {
            let p_start = &self.particles[i];
            let p_end = &self.particles[i + 1];
            let p_prev = if i > 0 { &self.particles[i - 1] } else { p_start };
            let p_after = if i + 2 < self.particles.len() {
                &self.particles[i + 2]
            } else {
                p_end
            };

            let base = (i * 4) as u16;
            let t0 = (i as f32) / total;
            let t1 = ((i + 1) as f32) / total;
            let corner_uvs = [[0.0f32, t0], [1.0, t0], [1.0, t1], [0.0, t1]];

            for (c_idx, uv) in corner_uvs.into_iter().enumerate() {
                let trail_pos = if c_idx < 2 { t0 } else { t1 };
                verts.push(RopeParticleVertex {
                    pos_start_size: [
                        p_start.position.x,
                        p_start.position.y,
                        p_start.position.z,
                        p_start.size * 0.5,
                    ],
                    pos_end_len: [
                        p_end.position.x,
                        p_end.position.y,
                        p_end.position.z,
                        total,
                    ],
                    pos_prev_tpos: [
                        p_prev.position.x,
                        p_prev.position.y,
                        p_prev.position.z,
                        trail_pos,
                    ],
                    pos_after_size_end: [
                        p_after.position.x,
                        p_after.position.y,
                        p_after.position.z,
                        p_end.size * 0.5,
                    ],
                    color_end: [p_end.color.x, p_end.color.y, p_end.color.z, p_end.alpha],
                    uv,
                    color_start: [
                        p_start.color.x,
                        p_start.color.y,
                        p_start.color.z,
                        p_start.alpha,
                    ],
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        (verts, indices)
    }
}

pub fn parse_vec3_str(s: &str, default: Vec3) -> Vec3 {
    let mut it = s.split_whitespace().filter_map(|p| p.parse::<f32>().ok());
    match (it.next(), it.next(), it.next()) {
        (Some(x), Some(y), Some(z)) => Vec3::new(x, y, z),
        (Some(x), Some(y), None) => Vec3::new(x, y, 0.0),
        (Some(x), None, None) => Vec3::splat(x),
        _ => default,
    }
}

fn parse_vec3_json(val: Option<&serde_json::Value>, default: Vec3) -> Vec3 {
    match val {
        Some(serde_json::Value::String(s)) => parse_vec3_str(s, default),
        Some(serde_json::Value::Number(n)) => Vec3::splat(n.as_f64().unwrap_or(0.0) as f32),
        _ => default,
    }
}
```

---

## 6. `wpe://` Custom Protocol Handler (`206 Partial Content`), `Video` Host & `Web` Host

> [!CAUTION]
> **Critical `<video>` / `<audio>` Custom Protocol Requirement (`HTTP 206 Partial Content`)**:  
> Both `WKWebView` (macOS) and `WebView2` (Windows) **refuse to play or loop `<video>` or `<audio>` elements** over a custom URI scheme (`wpe://localhost/...`) unless the protocol handler parses `Range: bytes=start-end` requests and responds with `206 Partial Content`, `Accept-Ranges: bytes`, `Content-Range: bytes start-end/total`, and `Content-Length`. Returning a plain `200 OK` to a `Range` request causes `WKWebView` to immediately abort playback with `MEDIA_ERR_SRC_NOT_SUPPORTED`!

### 6.1 Complete `wpe://` Custom Protocol Handler with Byte-Range Streaming
```rust
// crates/truongpaper-platform/src/wpe_protocol.rs
use http::{header, Request, Response, StatusCode};
use std::borrow::Cow;
use std::path::Path;
use truongpaper_we_formats::vfs::WallpaperVfs;

pub fn handle_wpe_protocol_request(
    vfs: &WallpaperVfs,
    request: Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    let uri_path = percent_encoding::percent_decode_str(request.uri().path())
        .decode_utf8_lossy()
        .into_owned();
    let rel_path = uri_path.trim_start_matches('/');

    let bytes = match vfs.read_bytes(rel_path) {
        Ok(b) => b,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .body(Cow::Borrowed(&b"Not Found"[..]))
                .unwrap();
        }
    };

    let total_len = bytes.len();
    let mime = guess_mime_type(Path::new(rel_path));

    // Check for HTTP Range header ("bytes=start-end") required by WKWebView & WebView2 media players
    if let Some(range_val) = request.headers().get(header::RANGE).and_then(|v| v.to_str().ok()) {
        if let Some((start, end)) = parse_byte_range(range_val, total_len) {
            let chunk_len = end - start + 1;
            let slice = bytes[start..=end].to_vec();
            return Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, mime)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_LENGTH, chunk_len.to_string())
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{total_len}"),
                )
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .body(Cow::Owned(slice))
                .unwrap();
        }
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, total_len.to_string())
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Cow::Owned(bytes))
        .unwrap()
}

fn parse_byte_range(header_val: &str, total_len: usize) -> Option<(usize, usize)> {
    if total_len == 0 {
        return None;
    }
    let spec = header_val.trim().strip_prefix("bytes=")?;
    let (start_str, end_str) = spec.split_once('-')?;
    let start: usize = start_str.trim().parse().ok()?;
    if start >= total_len {
        return None;
    }
    let end: usize = if end_str.trim().is_empty() {
        total_len - 1
    } else {
        end_str.trim().parse::<usize>().ok()?.min(total_len - 1)
    };
    if start <= end {
        Some((start, end))
    } else {
        None
    }
}

fn guess_mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("ogg" | "ogv") => "video/ogg",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("wasm") => "application/wasm",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        _ => "application/octet-stream",
    }
}
```

### 6.2 Standalone `Video` Wallpaper Host & Embedded `.tex` MP4 (`free_image_format == 35`) Support
1. **Standalone `"type": "video"` Wallpapers (`.mp4`, `.webm`, `.mov`, `.m4v`)**:
   - Loaded into the per-monitor desktop host via a built-in hardware-accelerated player document (`wpe://localhost/__video_host__.html?file=<encoded_project_file>`) that renders:
     - Background `<video id="bg-blur">` (active when `fitMode === 'fit_blur'`, styled with `filter: blur(48px) saturate(180%)`) + Foreground `<video id="fg-video">` (styled according to `fitMode`: `cover` for `fill`, `contain` for `fit_blur`/`fit_black`, `fill` for `stretch`).
     - Exposes `window.__truongpaper_apply_video_adjustments(adj)` called directly by `monitor_update_adjustments`:
       - Sets `fgVideo.volume = adj.muted ? 0 : adj.volume`, `fgVideo.muted = adj.muted`, and `fgVideo.playbackRate = adj.playbackSpeed`.
       - Applies `transform: scale(${adj.zoom}) translate(${adj.offsetX * 50}%, ${adj.offsetY * 50}%)` and `filter: brightness(${1 + adj.brightness}) contrast(${adj.contrast}) saturate(${adj.saturation}) hue-rotate(${adj.hueShift}rad)`.
2. **Embedded `.tex` MP4 Video Textures (`free_image_format == 35` / `TexFlags::IS_VIDEO_TEXTURE`)**:
   - When `parse_tex` encounters `free_image_format == 35` (where `mipmap.raw_bytes` contains a raw MP4 container starting with an `ftyp` box), write the bytes once to `<wallpaper_dir>/.cache/<tex_name>.mp4` and decode hardware video frames into the layer's `wgpu::Texture` at the video's native frame rate.

#### Complete `video-wallpaper-host.html` Implementation (`crates/truongpaper-platform/src/video-wallpaper-host.html`)
```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <title>TruongPaper Video Host</title>
  <style>
    * { margin: 0; padding: 0; box-sizing: border-box; user-select: none; }
    html, body {
      width: 100vw;
      height: 100vh;
      overflow: hidden;
      background: #000;
    }
    #bg-blur {
      position: absolute;
      inset: -10%;
      width: 120%;
      height: 120%;
      object-fit: cover;
      filter: blur(48px) saturate(180%) brightness(0.65);
      opacity: 0;
      transition: opacity 0.25s ease;
      pointer-events: none;
      z-index: 1;
    }
    #fg-video {
      position: absolute;
      inset: 0;
      width: 100%;
      height: 100%;
      object-fit: cover;
      transform-origin: center center;
      pointer-events: none;
      z-index: 2;
    }
  </style>
</head>
<body>
  <video id="bg-blur" autoplay loop muted playsinline></video>
  <video id="fg-video" autoplay loop muted playsinline></video>

  <script>
    (function () {
      'use strict';
      const params = new URLSearchParams(window.location.search);
      const rawFile = params.get('file') || '';
      const videoUrl = new URL('./' + encodeURI(rawFile.replace(/^\/+/, '')), window.location.href).href;

      const bgBlur = document.getElementById('bg-blur');
      const fgVideo = document.getElementById('fg-video');

      fgVideo.src = videoUrl;
      bgBlur.src = videoUrl;

      fgVideo.addEventListener('play', () => {
        if (bgBlur.style.opacity === '1') bgBlur.play().catch(() => {});
      });
      fgVideo.addEventListener('pause', () => bgBlur.pause());
      fgVideo.addEventListener('seeked', () => {
        if (Math.abs(bgBlur.currentTime - fgVideo.currentTime) > 0.25) {
          bgBlur.currentTime = fgVideo.currentTime;
        }
      });

      window.__truongpaper_apply_video_adjustments = function (adj) {
        if (!adj) return;
        const fitMode = adj.fitMode || 'fill';
        if (fitMode === 'fit_blur') {
          fgVideo.style.objectFit = 'contain';
          bgBlur.style.opacity = '1';
          if (!fgVideo.paused && bgBlur.paused) bgBlur.play().catch(() => {});
        } else if (fitMode === 'fit_black') {
          fgVideo.style.objectFit = 'contain';
          bgBlur.style.opacity = '0';
          bgBlur.pause();
        } else if (fitMode === 'stretch') {
          fgVideo.style.objectFit = 'fill';
          bgBlur.style.opacity = '0';
          bgBlur.pause();
        } else if (fitMode === 'center') {
          fgVideo.style.objectFit = 'none';
          bgBlur.style.opacity = '0';
          bgBlur.pause();
        } else {
          fgVideo.style.objectFit = 'cover';
          bgBlur.style.opacity = '0';
          bgBlur.pause();
        }

        const muted = Boolean(adj.muted) || (adj.volume ?? 0) <= 0.001;
        fgVideo.muted = muted;
        fgVideo.volume = muted ? 0 : Math.min(1, Math.max(0, Number(adj.volume ?? 1)));

        const rate = Math.min(4.0, Math.max(0.1, Number(adj.playbackSpeed ?? 1.0)));
        fgVideo.playbackRate = rate;
        bgBlur.playbackRate = rate;

        const zoom = Number(adj.zoom ?? 1.0);
        const ox = Number(adj.offsetX ?? 0.0) * 50;
        const oy = Number(adj.offsetY ?? 0.0) * 50;
        fgVideo.style.transform = `scale(${zoom}) translate(${ox}%, ${oy}%)`;

        const br = 1 + Number(adj.brightness ?? 0.0);
        const ct = Number(adj.contrast ?? 1.0);
        const sat = Number(adj.saturation ?? 1.0);
        const hue = Number(adj.hueShift ?? 0.0);
        fgVideo.style.filter = `brightness(${br}) contrast(${ct}) saturate(${sat}) hue-rotate(${hue}rad)`;
      };

      window.__truongpaper_set_paused = function (paused) {
        if (paused) {
          fgVideo.pause();
          bgBlur.pause();
        } else {
          fgVideo.play().catch(() => {});
          if (bgBlur.style.opacity === '1') bgBlur.play().catch(() => {});
        }
      };

      fgVideo.play().catch(() => {});
    })();
  </script>
</body>
</html>
```

### 6.3 Web & Video Wallpaper Autoplay & Occlusion Prevention (`wry` Host)

To prevent `Web` and `Video` wallpapers from stalling or throwing `NotAllowedError` when playing audio/video behind desktop icons:
- **Windows (`WebView2` Additional Browser Arguments)**:
  ```text
  --autoplay-policy=no-user-gesture-required
  --disable-background-timer-throttling
  --disable-backgrounding-occluded-windows
  --disable-background-media-suspend
  --disable-renderer-backgrounding
  --disable-features=WebContentsOcclusion,IsolateOrigins,site-per-process
  ```
- **macOS (`WKWebView` Configuration)**:
  - Set `configuration.mediaTypesRequiringUserActionForPlayback = WKAudiovisualMediaTypeNone` (`0`).
  - Set `preferences.javaScriptCanOpenWindowsAutomatically = false`.
  - Override parent `NSWindow.occlusionState` to report `NSWindowOcclusionStateVisible` while in `Playing` state.

---

## 7. `scene.json` Schema, Dynamic User Property Bindings & Multi-Pass FBO Chain

### 7.1 How `project.json` User Properties Bind to `scene.json` (`UserBoundValue<T>`)
In Wallpaper Engine `scene.json` files, any property (`visible`, `alpha`, `brightness`, `color`, `origin`, `scale`, `angles`, or a shader pass's `constantshadervalues` entry) can either be a plain scalar/string **or** a user-property binding object:
- Form A (Direct value): `"alpha": 1.0`
- Form B (Simple user property binding): `"alpha": { "user": "opacity_slider", "value": 1.0 }`
- Form C (Conditional visibility binding): `"visible": { "user": { "name": "weather_mode", "condition": "2" }, "value": true }`
- Form D (Scripted property): `"origin": { "script": "'use strict'; export function update(value) { ... }", "value": "960 540 0" }`

#### Copy-Pasteable Rust Deserializer for `UserBoundValue<T>` (`crates/truongpaper-we-formats/src/scene_schema.rs`)
```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserConditionSpec {
    PropertyName(String),
    Conditional {
        name: String,
        #[serde(default)]
        condition: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserBoundValue<T> {
    Bound {
        #[serde(default)]
        user: Option<UserConditionSpec>,
        #[serde(default)]
        script: Option<String>,
        #[serde(default)]
        scriptproperties: Option<HashMap<String, serde_json::Value>>,
        value: T,
    },
    Plain(T),
}

impl<T: Clone> UserBoundValue<T> {
    pub fn default_value(&self) -> &T {
        match self {
            Self::Plain(v) => v,
            Self::Bound { value, .. } => value,
        }
    }
}

/// Evaluates a UserBoundValue<bool> (such as layer or effect `visible`) against current project.json values
pub fn resolve_bound_bool(
    bound: &UserBoundValue<bool>,
    user_props: &HashMap<String, serde_json::Value>,
) -> bool {
    match bound {
        UserBoundValue::Plain(b) => *b,
        UserBoundValue::Bound { user, value, .. } => {
            let Some(user_spec) = user else { return *value };
            match user_spec {
                UserConditionSpec::PropertyName(prop_key) => {
                    user_props
                        .get(prop_key)
                        .and_then(|v| v.as_bool())
                        .unwrap_or(*value)
                }
                UserConditionSpec::Conditional { name, condition } => {
                    let Some(actual) = user_props.get(name) else { return *value };
                    if let Some(expected_cond) = condition {
                        let actual_str = match actual {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        actual_str == *expected_cond
                    } else {
                        actual.as_bool().unwrap_or(*value)
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneFile {
    #[serde(default)]
    pub camera: SceneCamera,
    pub general: SceneGeneral,
    #[serde(default)]
    pub objects: Vec<SceneObject>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneCamera {
    #[serde(default = "default_zero_vec3")]
    pub center: String,
    #[serde(default = "default_eye_vec3")]
    pub eye: String,
    #[serde(default = "default_up_vec3")]
    pub up: String,
}
fn default_zero_vec3() -> String { "0 0 0".into() }
fn default_eye_vec3() -> String { "0 0 1".into() }
fn default_up_vec3() -> String { "0 1 0".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrthogonalProjection {
    #[serde(default = "default_width")]
    pub width: f32,
    #[serde(default = "default_height")]
    pub height: f32,
    #[serde(default)]
    pub auto: bool,
}
fn default_width() -> f32 { 1920.0 }
fn default_height() -> f32 { 1080.0 }
impl Default for OrthogonalProjection {
    fn default() -> Self {
        Self { width: 1920.0, height: 1080.0, auto: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneGeneral {
    #[serde(default)]
    pub orthogonalprojection: OrthogonalProjection,
    #[serde(default = "default_clear_color")]
    pub clearcolor: UserBoundValue<String>,
    #[serde(default)]
    pub clearenabled: Option<bool>,
    #[serde(default)]
    pub cameraparallax: Option<UserBoundValue<bool>>,
    #[serde(default)]
    pub cameraparallaxamount: Option<UserBoundValue<f32>>,
    #[serde(default)]
    pub cameraparallaxdelay: Option<UserBoundValue<f32>>,
    #[serde(default)]
    pub cameraparallaxmouseinfluence: Option<UserBoundValue<f32>>,
    #[serde(default)]
    pub bloom: Option<UserBoundValue<bool>>,
    #[serde(default)]
    pub bloomstrength: Option<UserBoundValue<f32>>,
    #[serde(default)]
    pub bloomthreshold: Option<UserBoundValue<f32>>,
    #[serde(default)]
    pub ambientcolor: Option<String>,
    #[serde(default)]
    pub skylightcolor: Option<String>,
}
fn default_clear_color() -> UserBoundValue<String> {
    UserBoundValue::Plain("0 0 0".into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneObject {
    #[serde(default)]
    pub id: Option<i32>,
    #[serde(default)]
    pub parent: Option<i32>,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_visible")]
    pub visible: UserBoundValue<bool>,
    #[serde(default = "default_origin")]
    pub origin: UserBoundValue<String>,
    #[serde(default = "default_scale")]
    pub scale: UserBoundValue<String>,
    #[serde(default = "default_angles")]
    pub angles: UserBoundValue<String>,
    #[serde(default)]
    pub size: Option<UserBoundValue<String>>,
    #[serde(default)]
    pub alignment: Option<String>,
    #[serde(default = "default_alpha")]
    pub alpha: UserBoundValue<f32>,
    #[serde(default = "default_brightness")]
    pub brightness: UserBoundValue<f32>,
    #[serde(default = "default_color")]
    pub color: UserBoundValue<String>,
    #[serde(default, rename = "colorBlendMode")]
    pub color_blend_mode: Option<i32>,
    #[serde(default, rename = "parallaxDepth")]
    pub parallax_depth: Option<String>,

    // Layer kind payloads
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub particle: Option<String>,
    #[serde(default)]
    pub sound: Option<Vec<String>>,
    #[serde(default)]
    pub playbackmode: Option<String>,
    #[serde(default)]
    pub volume: Option<UserBoundValue<f32>>,
    #[serde(default)]
    pub mintime: Option<f32>,
    #[serde(default)]
    pub maxtime: Option<f32>,
    #[serde(default)]
    pub text: Option<UserBoundValue<String>>,
    #[serde(default)]
    pub effects: Vec<SceneEffectRef>,
}
fn default_visible() -> UserBoundValue<bool> { UserBoundValue::Plain(true) }
fn default_origin() -> UserBoundValue<String> { UserBoundValue::Plain("0 0 0".into()) }
fn default_scale() -> UserBoundValue<String> { UserBoundValue::Plain("1 1 1".into()) }
fn default_angles() -> UserBoundValue<String> { UserBoundValue::Plain("0 0 0".into()) }
fn default_alpha() -> UserBoundValue<f32> { UserBoundValue::Plain(1.0) }
fn default_brightness() -> UserBoundValue<f32> { UserBoundValue::Plain(1.0) }
fn default_color() -> UserBoundValue<String> { UserBoundValue::Plain("1 1 1".into()) }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneEffectRef {
    pub file: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_visible")]
    pub visible: UserBoundValue<bool>,
    #[serde(default)]
    pub passes: Vec<ScenePassOverride>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScenePassOverride {
    #[serde(default)]
    pub combos: HashMap<String, i32>,
    #[serde(default)]
    pub constantshadervalues: HashMap<String, UserBoundValue<serde_json::Value>>,
    #[serde(default)]
    pub textures: Vec<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelJsonFile {
    #[serde(default)]
    pub material: String,
    #[serde(default)]
    pub puppet: Option<String>,
    #[serde(default)]
    pub width: Option<i32>,
    #[serde(default)]
    pub height: Option<i32>,
    #[serde(default)]
    pub autosize: Option<bool>,
    #[serde(default)]
    pub fullscreen: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectFile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub fbos: Vec<EffectFboSpec>,
    #[serde(default)]
    pub passes: Vec<EffectPassSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectFboSpec {
    pub name: String,
    #[serde(default = "default_scale_u32")]
    pub scale: u32,
    #[serde(default)]
    pub format: String,
}
fn default_scale_u32() -> u32 { 1 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectPassSpec {
    #[serde(default)]
    pub material: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub bind: Vec<EffectBindSlot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectBindSlot {
    pub name: String,
    pub index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialFile {
    pub passes: Vec<MaterialPassSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialPassSpec {
    pub shader: String,
    #[serde(default = "default_blending")]
    pub blending: String,
    #[serde(default = "default_disabled")]
    pub depthtest: String,
    #[serde(default = "default_disabled")]
    pub depthwrite: String,
    #[serde(default = "default_cull_nocull")]
    pub cullmode: String,
    #[serde(default)]
    pub textures: Vec<Option<String>>,
    #[serde(default)]
    pub combos: HashMap<String, i32>,
    #[serde(default)]
    pub constantshadervalues: HashMap<String, serde_json::Value>,
}
fn default_blending() -> String { "normal".into() }
fn default_disabled() -> String { "disabled".into() }
fn default_cull_nocull() -> String { "nocull".into() }
```

### 7.2 Coordinate System & `g_ModelViewProjectionMatrix` Construction
1. **Orthogonal Projection**:
   - Read `scene.general.orthogonalprojection` (`{ width, height }`, defaulting to `1920 × 1080`).
   - Build orthographic projection matrix:
     `glam::Mat4::orthographic_rh(0.0, scene_width, 0.0, scene_height, -1000.0, 1000.0)`.
   - Notice that in Wallpaper Engine scene space, `(0, 0)` is the **bottom-left** corner and `(scene_width, scene_height)` is the **top-right** corner, with the default center at `(scene_width / 2.0, scene_height / 2.0)`.
2. **Layer Model Matrix**:
   - Parse `"origin": "X Y Z"`, `"scale": "SX SY SZ"`, `"angles": "RX RY RZ"` (in radians or degrees depending on shader), and `"size": "W H"`.
   - Handle `"alignment"` anchor offset (`"center"` = `(0, 0)`, `"top-left"` = `(+w/2, -h/2)`, `"bottom-left"` = `(+w/2, +h/2)`, etc.).
   - Apply camera parallax (`scene.general.cameraparallax`, `cameraparallaxamount`, `cameraparallaxdelay`, `cameraparallaxmouseinfluence`) by offsetting the camera eye position toward `(g_PointerPosition - 0.5) * amount * influence`.
3. **Multi-Pass Ping-Pong FBO Effect Chain**:
   - Each image/composition layer allocates two ping-pong render targets (`FBO_A`, `FBO_B`) sized to the layer's `size`.
   - Before rendering the first effect pass, render the base `.tex` texture into `FBO_A`.
   - For each visible `effect` in `object.effects` and each `pass` in `effect.passes`:
     - Resolve material JSON (`pass.material`) $\to$ shader name (`passes[0].shader`) + `combos` + `blending` (`normal`, `translucent`, `additive`, `multiply`, `screen`) + `constantshadervalues`.
     - If `pass.bind` specifies a custom target or binds `"_rt_FullFrameBuffer"`, bind the scene-wide full-frame color copy texture to that sampler slot.
     - Draw into `FBO_B`, then swap `FBO_A` $\leftrightarrow$ `FBO_B`.

### 7.3 Sound Layer Playback Engine via `kira` (`crates/truongpaper-audio/src/sound_layer.rs`)

```rust
// crates/truongpaper-audio/src/sound_layer.rs
use kira::manager::{AudioManager, AudioManagerSettings, backend::DefaultBackend};
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::tween::Tween;
use std::io::Cursor;
use truongpaper_we_formats::scene_schema::SceneObject;
use truongpaper_we_formats::vfs::WallpaperVfs;

pub struct SceneSoundPlayer {
    manager: AudioManager<DefaultBackend>,
    active_handles: Vec<StaticSoundHandle>,
}

impl SceneSoundPlayer {
    pub fn new() -> anyhow::Result<Self> {
        let manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())?;
        Ok(Self {
            manager,
            active_handles: Vec::new(),
        })
    }

    pub fn start_scene_sounds(
        &mut self,
        vfs: &WallpaperVfs,
        objects: &[SceneObject],
        master_volume: f32,
    ) {
        for obj in objects {
            let Some(sound_paths) = &obj.sound else { continue };
            let Some(first_rel) = sound_paths.first() else { continue };
            let Ok(bytes) = vfs.read_bytes(first_rel) else { continue };

            let layer_vol = obj
                .volume
                .as_ref()
                .map(|v| *v.default_value())
                .unwrap_or(1.0);
            let is_loop = obj.playbackmode.as_deref().unwrap_or("loop") == "loop";

            if let Ok(mut sound_data) = StaticSoundData::from_cursor(Cursor::new(bytes)) {
                if is_loop {
                    sound_data = sound_data.loop_region(..);
                }
                sound_data = sound_data.volume((layer_vol * master_volume).clamp(0.0, 1.0) as f64);
                if let Ok(handle) = self.manager.play(sound_data) {
                    self.active_handles.push(handle);
                }
            }
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        for handle in &mut self.active_handles {
            if paused {
                let _ = handle.pause(Tween::default());
            } else {
                let _ = handle.resume(Tween::default());
            }
        }
    }
}
```

---

## 8. SceneScript Engine Specification (`truongpaper-scenescript` via `rquickjs`)

Wallpaper Engine `Scene` wallpapers can attach JavaScript snippets (`'use strict'; export function update(value) { ... }`) to any layer property or event.

### 8.1 Supported Lifecycle Callbacks
| Exported Function | When Called | Return Value |
| :--- | :--- | :--- |
| `init(value)` | Once when the scene finishes loading | Initial property value (`Number`, `Boolean`, `String`, `Vec2`, `Vec3`, `Vec4`) |
| `update(value)` | Every frame (`dt = engine.frametime`) | Updated property value |
| `cursorMove(event)` | When desktop cursor moves on that monitor | `void` |
| `cursorDown(event)` / `cursorUp(event)` / `cursorClick(event)` | When user clicks on unoccluded desktop | `void` |
| `applyUserProperties(changedProps)` | Whenever a `project.json` property is changed in the UI | `void` |

### 8.2 Global Objects Exposed in the `rquickjs` Context
- **`engine`**:
  - `engine.frametime`: Delta time in seconds (`f32`) since last frame.
  - `engine.runtime`: Total elapsed seconds (`f32`) since scene start.
  - `engine.screenResolution`: `new Vec2(monitor_w, monitor_h)`.
  - `engine.canvasSize`: `new Vec2(scene_w, scene_h)`.
  - `engine.timeOfDay`: Normalized local time $[0.0, 1.0]$.
  - `engine.registerAudioBuffers(resolution: 16 | 32 | 64)`: Returns `{ average: Float32Array, left: Float32Array, right: Float32Array }` updated in-place every frame from `AudioSpectrumFrame`.
- **`input`**:
  - `input.cursorWorldPosition`: `Vec3(cursor_x * scene_w, (1.0 - cursor_y) * scene_h, 0.0)`.
  - `input.cursorScreenPosition`: `Vec2(cursor_x * monitor_w, cursor_y * monitor_h)`.
  - `input.cursorLeftDown`: `boolean`.
- **`thisLayer`**:
  - Getters/setters for `.origin` (`Vec3`), `.scale` (`Vec3`), `.angles` (`Vec3`), `.size` (`Vec2`), `.alpha` (`f32`), `.visible` (`bool`), `.color` (`Vec3`), `.name` (`String`), and `.getEffect(nameOrIndex)`.
- **`thisScene`**:
  - `thisScene.getLayer(name: string)` $\to$ returns layer proxy object.
  - `thisScene.enumerateLayers()` $\to$ returns array of all layer proxies.
- **`Vec2(x, y)`, `Vec3(x, y, z)`, `Vec4(x, y, z, w)`, `Mat4`**:
  - Built-in vector classes with `.add()`, `.subtract()`, `.multiply()`, `.divide()`, `.dot()`, `.cross()`, `.length()`, `.lengthSqr()`, `.normalize()`, `.mix(other, t)`, `.copy()`.

### 8.3 Complete `rquickjs` SceneScript Bootstrap & Host Implementation (`crates/truongpaper-scenescript/src/lib.rs`)

```rust
// crates/truongpaper-scenescript/src/lib.rs
use rquickjs::{Context, Runtime};
use std::collections::HashMap;

pub const SCENESCRIPT_BOOTSTRAP_JS: &str = r#"
class Vec2 {
    constructor(x = 0, y = x) {
        if (typeof x === 'string') {
            const p = x.trim().split(/\s+/).map(Number);
            this.x = p[0] || 0; this.y = p[1] ?? this.x;
        } else if (x && typeof x === 'object') {
            this.x = x.x || 0; this.y = x.y || 0;
        } else {
            this.x = Number(x) || 0; this.y = Number(y) || 0;
        }
    }
    add(v) { return new Vec2(this.x + (v.x ?? v), this.y + (v.y ?? v)); }
    subtract(v) { return new Vec2(this.x - (v.x ?? v), this.y - (v.y ?? v)); }
    multiply(v) { return new Vec2(this.x * (v.x ?? v), this.y * (v.y ?? v)); }
    divide(v) { return new Vec2(this.x / (v.x ?? v), this.y / (v.y ?? v)); }
    dot(v) { return this.x * v.x + this.y * v.y; }
    length() { return Math.hypot(this.x, this.y); }
    lengthSqr() { return this.x * this.x + this.y * this.y; }
    normalize() { const l = this.length() || 1; return new Vec2(this.x / l, this.y / l); }
    mix(v, t) { return new Vec2(this.x + (v.x - this.x) * t, this.y + (v.y - this.y) * t); }
    copy() { return new Vec2(this.x, this.y); }
    toString() { return `${this.x} ${this.y}`; }
}

class Vec3 {
    constructor(x = 0, y = x, z = x) {
        if (typeof x === 'string') {
            const p = x.trim().split(/\s+/).map(Number);
            this.x = p[0] || 0; this.y = p[1] ?? this.x; this.z = p[2] ?? 0;
        } else if (x && typeof x === 'object') {
            this.x = x.x || 0; this.y = x.y || 0; this.z = x.z || 0;
        } else {
            this.x = Number(x) || 0; this.y = Number(y) || 0; this.z = Number(z) || 0;
        }
    }
    add(v) { return new Vec3(this.x + (v.x ?? v), this.y + (v.y ?? v), this.z + (v.z ?? v)); }
    subtract(v) { return new Vec3(this.x - (v.x ?? v), this.y - (v.y ?? v), this.z - (v.z ?? v)); }
    multiply(v) { return new Vec3(this.x * (v.x ?? v), this.y * (v.y ?? v), this.z * (v.z ?? v)); }
    divide(v) { return new Vec3(this.x / (v.x ?? v), this.y / (v.y ?? v), this.z / (v.z ?? v)); }
    dot(v) { return this.x * v.x + this.y * v.y + this.z * v.z; }
    cross(v) {
        return new Vec3(
            this.y * v.z - this.z * v.y,
            this.z * v.x - this.x * v.z,
            this.x * v.y - this.y * v.x
        );
    }
    length() { return Math.hypot(this.x, this.y, this.z); }
    lengthSqr() { return this.x * this.x + this.y * this.y + this.z * this.z; }
    normalize() { const l = this.length() || 1; return new Vec3(this.x / l, this.y / l, this.z / l); }
    mix(v, t) {
        return new Vec3(
            this.x + (v.x - this.x) * t,
            this.y + (v.y - this.y) * t,
            this.z + (v.z - this.z) * t
        );
    }
    copy() { return new Vec3(this.x, this.y, this.z); }
    toString() { return `${this.x} ${this.y} ${this.z}`; }
}

class Vec4 {
    constructor(x = 0, y = x, z = x, w = 1) {
        this.x = Number(x) || 0; this.y = Number(y) || 0;
        this.z = Number(z) || 0; this.w = Number(w) || 0;
    }
    copy() { return new Vec4(this.x, this.y, this.z, this.w); }
    toString() { return `${this.x} ${this.y} ${this.z} ${this.w}`; }
}

globalThis.Vec2 = Vec2;
globalThis.Vec3 = Vec3;
globalThis.Vec4 = Vec4;
globalThis.shared = {};
globalThis.__layers = {};
globalThis.__scripts = {};
globalThis.__audioBuffers = {
    16: { left: new Float32Array(16), right: new Float32Array(16), average: new Float32Array(16) },
    32: { left: new Float32Array(32), right: new Float32Array(32), average: new Float32Array(32) },
    64: { left: new Float32Array(64), right: new Float32Array(64), average: new Float32Array(64) },
};

globalThis.engine = {
    frametime: 0.0166,
    runtime: 0.0,
    timeOfDay: 0.5,
    screenResolution: new Vec2(1920, 1080),
    canvasSize: new Vec2(1920, 1080),
    userProperties: {},
    registerAudioBuffers(res) {
        return globalThis.__audioBuffers[res] || globalThis.__audioBuffers[64];
    },
    isDesktopDevice() { return true; },
    isWallpaper() { return true; },
};

globalThis.input = {
    cursorWorldPosition: new Vec3(960, 540, 0),
    cursorScreenPosition: new Vec2(960, 540),
    cursorLeftDown: false,
};

globalThis.thisScene = {
    getLayer(name) { return globalThis.__layers[name] || null; },
    enumerateLayers() { return Object.values(globalThis.__layers); },
};
"#;

pub struct SceneScriptEngine {
    _runtime: Runtime,
    context: Context,
}

impl SceneScriptEngine {
    pub fn new(scene_w: f32, scene_h: f32, monitor_w: f32, monitor_h: f32) -> anyhow::Result<Self> {
        let runtime = Runtime::new()?;
        let context = Context::full(&runtime)?;

        context.with(|ctx| -> anyhow::Result<()> {
            ctx.eval::<(), _>(SCENESCRIPT_BOOTSTRAP_JS)?;
            ctx.eval::<(), _>(format!(
                "engine.canvasSize = new Vec2({scene_w}, {scene_h});
                 engine.screenResolution = new Vec2({monitor_w}, {monitor_h});"
            ))?;
            Ok(())
        })?;

        Ok(Self {
            _runtime: runtime,
            context,
        })
    }

    pub fn register_layer(
        &self,
        layer_name: &str,
        origin: [f32; 3],
        scale: [f32; 3],
        angles: [f32; 3],
        alpha: f32,
        visible: bool,
    ) -> anyhow::Result<()> {
        self.context.with(|ctx| -> anyhow::Result<()> {
            let js = format!(
                "globalThis.__layers[{name:?}] = {{
                    name: {name:?},
                    origin: new Vec3({}, {}, {}),
                    scale: new Vec3({}, {}, {}),
                    angles: new Vec3({}, {}, {}),
                    alpha: {},
                    visible: {},
                    getEffect() {{ return {{ visible: true }}; }}
                }};",
                origin[0], origin[1], origin[2],
                scale[0], scale[1], scale[2],
                angles[0], angles[1], angles[2],
                alpha,
                visible,
                name = layer_name,
            );
            ctx.eval::<(), _>(js)?;
            Ok(())
        })
    }

    pub fn register_property_script(
        &self,
        slot_key: &str,
        layer_name: &str,
        raw_es_script: &str,
        initial_value_expr: &str,
    ) -> anyhow::Result<String> {
        let transformed = raw_es_script
            .replace("export function ", "exports.")
            .replace("export var ", "exports.")
            .replace("export let ", "exports.")
            .replace("export const ", "exports.");

        let valid_body = rewrite_export_functions(&transformed);

        self.context.with(|ctx| -> anyhow::Result<String> {
            let wrapper = format!(
                "(function() {{
                    const exports = {{}};
                    const thisLayer = globalThis.__layers[{layer_name:?}] || {{}};
                    const scriptProperties = {{}};
                    const createScriptProperties = function() {{
                        const builder = {{
                            addSlider(o) {{ scriptProperties[o.name] = o.value; return builder; }},
                            addCheckbox(o) {{ scriptProperties[o.name] = o.value; return builder; }},
                            addColor(o) {{ scriptProperties[o.name] = new Vec3(o.value); return builder; }},
                            addText(o) {{ scriptProperties[o.name] = o.value; return builder; }},
                            addCombo(o) {{ scriptProperties[o.name] = o.options?.[0]?.value ?? 0; return builder; }},
                            finish() {{ return scriptProperties; }}
                        }};
                        return builder;
                    }};
                    {valid_body}
                    let val = {initial_value_expr};
                    if (typeof exports.init === 'function') {{
                        const r = exports.init(val);
                        if (r !== undefined) val = r;
                    }}
                    globalThis.__scripts[{slot_key:?}] = {{ exports, thisLayer, currentValue: val }};
                    return String(val);
                }})()"
            );
            let res: String = ctx.eval(wrapper)?;
            Ok(res)
        })
    }

    pub fn step_frame(
        &self,
        dt: f32,
        runtime: f32,
        cursor_world: [f32; 3],
        left_64: &[f32; 64],
        right_64: &[f32; 64],
    ) -> anyhow::Result<HashMap<String, String>> {
        self.context.with(|ctx| -> anyhow::Result<HashMap<String, String>> {
            let left_csv = left_64.iter().map(|v| format!("{v:.4}")).collect::<Vec<_>>().join(",");
            let right_csv = right_64.iter().map(|v| format!("{v:.4}")).collect::<Vec<_>>().join(",");
            let step_js = format!(
                "(function() {{
                    engine.frametime = {dt};
                    engine.runtime = {runtime};
                    input.cursorWorldPosition.x = {};
                    input.cursorWorldPosition.y = {};
                    const L = [{left_csv}];
                    const R = [{right_csv}];
                    for (const res of [16, 32, 64]) {{
                        const stride = 64 / res;
                        const buf = globalThis.__audioBuffers[res];
                        for (let i = 0; i < res; i++) {{
                            const l = L[i * stride] || 0;
                            const r = R[i * stride] || 0;
                            buf.left[i] = l;
                            buf.right[i] = r;
                            buf.average[i] = (l + r) * 0.5;
                        }}
                    }}
                    const updated = {{}};
                    for (const [k, slot] of Object.entries(globalThis.__scripts)) {{
                        if (typeof slot.exports.update === 'function') {{
                            const next = slot.exports.update(slot.currentValue);
                            if (next !== undefined) slot.currentValue = next;
                        }}
                        updated[k] = String(slot.currentValue);
                    }}
                    return JSON.stringify(updated);
                }})()",
                cursor_world[0], cursor_world[1]
            );
            let json_out: String = ctx.eval(step_js)?;
            let map: HashMap<String, String> = serde_json::from_str(&json_out)?;
            Ok(map)
        })
    }
}

fn rewrite_export_functions(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 64);
    for line in src.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("exports.") {
            if let Some(paren_idx) = rest.find('(') {
                let fn_name = rest[..paren_idx].trim();
                if !fn_name.contains('=') && !fn_name.contains(' ') {
                    let args_and_body = &rest[paren_idx..];
                    out.push_str(&format!("exports.{fn_name} = function{args_and_body}\n"));
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}
```

---

## 9. Complete `web-wallpaper-preload.js` Injection Script (`Web` Wallpapers)

Inject this exact preload script via `wry::WebViewBuilder::with_initialization_script` before loading `<wallpaper_dir>/<project.file>` via `wpe://localhost/`:

```javascript
// src-tauri/crates/truongpaper-platform/src/web-wallpaper-preload.js
(function () {
  'use strict';

  const audioCallbacks = [];
  window.__truongpaper_fps_limit = 60;

  // 1. Wallpaper Engine Audio Listener Registration (128-float array: 0..63 Left, 64..127 Right)
  window.wallpaperRegisterAudioListener = function (callback) {
    if (typeof callback === 'function' && !audioCallbacks.includes(callback)) {
      audioCallbacks.push(callback);
    }
  };

  // Called by Rust backend at 60 Hz with a 128-element Array<number>
  window.__truongpaper_dispatch_audio = function (stereo128Array) {
    for (let i = 0; i < audioCallbacks.length; i++) {
      try {
        audioCallbacks[i](stereo128Array);
      } catch (err) {
        console.error('[TruongPaper WebBridge] Audio callback error:', err);
      }
    }
  };

  // 2. Called by Rust backend on startup and whenever user changes a property in PropertyInspector
  window.__truongpaper_dispatch_properties = function (propsPayload, generalSettings) {
    if (window.wallpaperPropertyListener) {
      if (
        propsPayload &&
        typeof window.wallpaperPropertyListener.applyUserProperties === 'function'
      ) {
        try {
          window.wallpaperPropertyListener.applyUserProperties(propsPayload);
        } catch (err) {
          console.error('[TruongPaper WebBridge] applyUserProperties error:', err);
        }
      }
      if (
        generalSettings &&
        typeof window.wallpaperPropertyListener.applyGeneralProperties === 'function'
      ) {
        try {
          window.wallpaperPropertyListener.applyGeneralProperties(generalSettings);
        } catch (err) {
          console.error('[TruongPaper WebBridge] applyGeneralProperties error:', err);
        }
      }
    }
  };

  // 3. Random file helper for directory properties
  window.wallpaperRequestRandomFileForProperty = function (propertyName, callback) {
    if (window.__truongpaper_files_by_prop && window.__truongpaper_files_by_prop[propertyName]) {
      const list = window.__truongpaper_files_by_prop[propertyName];
      if (list.length > 0 && typeof callback === 'function') {
        const pick = list[Math.floor(Math.random() * list.length)];
        callback(propertyName, pick);
      }
    }
  };
})();
```

---

## 10. Windows `Application` Wallpaper Host (`truongpaper-platform`)

When a Workshop item has `"type": "application"`:
- On **macOS**: Flag the item with `windowsOnly: true` in `WallpaperItem` and display a `"Windows Only"` badge in the UI.
- On **Windows**: Launch the application binary via `std::process::Command` with standard window bounds arguments (`-popupwindow -screen-fullscreen 0 -screen-width <W> -screen-height <H>`) and terminate the `std::process::Child` handle cleanly when switching wallpapers or shutting down the app:

```rust
#[cfg(target_os = "windows")]
pub fn spawn_application_wallpaper(
    exe_path: &std::path::Path,
    width: i32,
    height: i32,
) -> anyhow::Result<std::process::Child> {
    let child = std::process::Command::new(exe_path)
        .current_dir(exe_path.parent().unwrap_or_else(|| std::path::Path::new(".")))
        .args([
            "-popupwindow",
            "-screen-fullscreen",
            "0",
            "-screen-width",
            &width.to_string(),
            "-screen-height",
            &height.to_string(),
        ])
        .spawn()?;
    Ok(child)
}
```
