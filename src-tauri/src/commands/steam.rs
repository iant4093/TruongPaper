//! Steam Workshop & Wallpaper Engine commands (docs/04 §5.1, docs/03 §1):
//! `steam_inspect_url`, `steam_download_workshop_item`, `steam_login_qr`,
//! `steam_login_credentials`, `steam_submit_2fa_code`, `steam_logout`, `steam_sync_local_folders`.
//!
//! Wraps `truongpaper-steam`: argument builders (`commands`), the DepotDownloader sidecar runner,
//! public metadata client and local Steam folder sync. The saved account name lives in the
//! single keychain vault (`secrets.rs`, key `steam_username`); passwords are never stored.

use crate::commands::library::list_items;
use crate::db;
use crate::models::{
    Need2FaPayload, QrMatrixPayload, SteamDownloadProgress, WallpaperItem,
    EVENT_STEAM_DOWNLOAD_PROGRESS, EVENT_STEAM_NEED_2FA, EVENT_STEAM_QR_MATRIX,
};
use crate::secrets::VAULT;
use crate::state::AppState;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;
use truongpaper_steam::{
    core_assets_bootstrap_args, core_assets_bootstrapped, fetch_workshop_preview,
    find_local_we_core_assets_dir, parse_workshop_url_or_id, password_login_args,
    pubfile_download_args, qr_login_args, run_depot_downloader_sidecar, scan_workshop_directories,
    write_core_assets_filelist, SidecarEvent, WorkshopPreview, DEFAULT_MAX_DOWNLOADS,
};

const SECRET_STEAM_USERNAME: &str = "steam_username";
const SECRET_STEAM_REFRESH_TOKEN: &str = "steam_refresh_token";
/// Sidecar name as declared in `tauri.conf.json` `bundle.externalBin`.
const SIDECAR_NAME: &str = "DepotDownloader";

/// Running DepotDownloader processes and the 2FA code channel of the interactive one.
#[derive(Default)]
pub struct SteamSession {
    working_dir: PathBuf,
    downloads: Mutex<HashSet<String>>,
    two_fa_tx: Mutex<Option<(u64, mpsc::Sender<String>)>>,
    tasks: Mutex<HashMap<u64, tokio::task::AbortHandle>>,
    next_id: AtomicU64,
}

struct DownloadGuard<'a> {
    downloads: &'a Mutex<HashSet<String>>,
    id: String,
}

impl Drop for DownloadGuard<'_> {
    fn drop(&mut self) {
        self.downloads.lock().remove(&self.id);
    }
}

impl SteamSession {
    fn track_download(&self, id: &str) -> Result<DownloadGuard<'_>, String> {
        if !self.downloads.lock().insert(id.to_owned()) {
            return Err("This wallpaper is already downloading.".into());
        }
        Ok(DownloadGuard {
            downloads: &self.downloads,
            id: id.to_owned(),
        })
    }

    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir,
            ..Self::default()
        }
    }

    fn register(
        &self,
        handle: tokio::task::AbortHandle,
        two_fa: Option<mpsc::Sender<String>>,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.tasks.lock().insert(id, handle);
        if let Some(two_fa) = two_fa {
            *self.two_fa_tx.lock() = Some((id, two_fa));
        }
        id
    }

    fn unregister(&self, id: u64) {
        self.tasks.lock().remove(&id);
        let mut tx = self.two_fa_tx.lock();
        if tx.as_ref().is_some_and(|(owner, _)| *owner == id) {
            *tx = None;
        }
    }

    fn two_fa_sender(&self) -> Option<mpsc::Sender<String>> {
        self.two_fa_tx.lock().as_ref().map(|(_, tx)| tx.clone())
    }

    pub fn running_count(&self) -> usize {
        self.tasks.lock().len()
    }

    /// Kills every sidecar (`kill_on_drop`) by aborting its task.
    pub fn abort_all(&self) {
        for (_, handle) in self.tasks.lock().drain() {
            handle.abort();
        }
        *self.two_fa_tx.lock() = None;
    }
}

/// Candidate sidecar locations, in priority order:
/// 1. next to the app executable (bundled app, and `tauri dev` which copies `externalBin` into
///    `target/<profile>/` with the target-triple suffix stripped);
/// 2. the un-bundled `src-tauri/bin/DepotDownloader-<triple>` produced by
///    `npm run bundle:depotdownloader` (debug builds only).
pub fn sidecar_candidates(
    exe_dir: Option<&Path>,
    manifest_dir: &Path,
    target_triple: Option<&str>,
    include_dev_path: bool,
) -> Vec<PathBuf> {
    let suffix = std::env::consts::EXE_SUFFIX;
    let mut out = Vec::new();
    if let Some(dir) = exe_dir {
        out.push(dir.join(format!("{SIDECAR_NAME}{suffix}")));
    }
    if include_dev_path {
        if let Some(triple) = target_triple {
            out.push(
                manifest_dir
                    .join("bin")
                    .join(format!("{SIDECAR_NAME}-{triple}{suffix}")),
            );
        }
    }
    out
}

fn resolve_sidecar() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().ok();
    let triple = tauri::utils::platform::target_triple().ok();
    let candidates = sidecar_candidates(
        exe.as_deref().and_then(Path::parent),
        Path::new(env!("CARGO_MANIFEST_DIR")),
        triple.as_deref(),
        cfg!(debug_assertions),
    );
    candidates
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .ok_or_else(|| {
            "DepotDownloader sidecar not found; run `npm run bundle:depotdownloader`".to_string()
        })
}

/// Runs DepotDownloader with `args`, feeding every [`SidecarEvent`] to `on_event` on the
/// calling task. The process is killed if the app shuts down (`SteamSession::abort_all`).
async fn run_sidecar(
    session: &Arc<SteamSession>,
    args: Vec<String>,
    signing_in: bool,
    mut on_event: impl FnMut(SidecarEvent) + Send,
) -> Result<(), String> {
    let bin = resolve_sidecar()?;
    let working_dir = session.working_dir.clone();
    let (two_fa_tx, two_fa_rx) = mpsc::channel::<String>(4);
    let (event_tx, mut event_rx) = mpsc::channel::<SidecarEvent>(64);
    let task = tokio::spawn(async move {
        run_depot_downloader_sidecar(
            &bin,
            &working_dir,
            &args,
            signing_in.then_some(two_fa_rx),
            event_tx,
        )
        .await
        .map_err(|e| e.to_string())
    });
    let id = session.register(task.abort_handle(), signing_in.then_some(two_fa_tx));
    while let Some(ev) = event_rx.recv().await {
        if signing_in && ev == SidecarEvent::Authenticated {
            // The token is saved before Steam confirms the session. End the
            // manifest probe here so sign-in does not depend on a depot license.
            task.abort();
            let _ = task.await;
            session.unregister(id);
            return Ok(());
        }
        on_event(ev);
    }
    let res = task.await;
    session.unregister(id);
    match res {
        Ok(Ok(())) if signing_in => {
            Err("Steam ended the sign-in without confirming the session.".into())
        }
        Ok(r) => r,
        Err(err) if err.is_cancelled() => Err("Steam task was cancelled".into()),
        Err(err) => Err(err.to_string()),
    }
}

/// Maps sidecar auth events to the `steam-auth://*` channels.
pub fn auth_event_payload(ev: &SidecarEvent) -> Option<(&'static str, serde_json::Value)> {
    match ev {
        SidecarEvent::QrMatrix(matrix) => Some((
            EVENT_STEAM_QR_MATRIX,
            serde_json::to_value(QrMatrixPayload {
                matrix: matrix.clone(),
            })
            .ok()?,
        )),
        SidecarEvent::Need2Fa { kind, prompt } => Some((
            EVENT_STEAM_NEED_2FA,
            serde_json::to_value(Need2FaPayload {
                kind,
                prompt: prompt.clone(),
            })
            .ok()?,
        )),
        _ => None,
    }
}

fn emit_auth(app: &AppHandle, ev: &SidecarEvent) {
    if let Some((name, payload)) = auth_event_payload(ev) {
        let _ = app.emit(name, payload);
    }
}

fn saved_username() -> Result<String, String> {
    VAULT
        .get(SECRET_STEAM_USERNAME)?
        .ok_or_else(|| "Sign in to Steam first (QR code or username/password)".to_string())
}

fn emit_progress(
    app: &AppHandle,
    workshop_id: &str,
    title: &str,
    pct: f32,
    state: &'static str,
    error: Option<String>,
) {
    let _ = app.emit(
        EVENT_STEAM_DOWNLOAD_PROGRESS,
        SteamDownloadProgress {
            workshop_id: workshop_id.to_string(),
            title: title.to_string(),
            progress_percent: pct.clamp(0.0, 100.0),
            state,
            error,
        },
    );
}

/// Scans `<app_data>/wallpapers/workshop` + local Steam `431960` folders into the library.
pub async fn sync_library(state: &AppState) -> Result<Vec<WallpaperItem>, String> {
    let root = state.workshop_dir();
    let found = tokio::task::spawn_blocking(move || scan_workshop_directories(&root))
        .await
        .map_err(|e| e.to_string())?;
    {
        let conn = state.db.lock();
        for w in &found {
            db::upsert_discovered(&conn, w)?;
        }
    }
    list_items(state).await
}

/// First-run Wallpaper Engine core `assets/` bootstrap (skipped when a local WE install exists).
async fn bootstrap_core_assets_if_needed(session: Arc<SteamSession>, root: PathBuf, user: String) {
    let needed = {
        let root = root.clone();
        tokio::task::spawn_blocking(move || {
            !core_assets_bootstrapped(&root) && find_local_we_core_assets_dir().is_none()
        })
        .await
        .unwrap_or(false)
    };
    if !needed {
        return;
    }
    let args = match write_core_assets_filelist(&root)
        .map_err(|e| e.to_string())
        .and_then(|list| core_assets_bootstrap_args(&user, &list, &root).map_err(|e| e.to_string()))
    {
        Ok(a) => a,
        Err(err) => {
            tracing::warn!("core assets bootstrap skipped: {err}");
            return;
        }
    };
    if let Err(err) = run_sidecar(&session, args, false, |_| {}).await {
        tracing::warn!("core assets bootstrap failed: {err}");
    }
}

#[tauri::command]
pub async fn steam_inspect_url(
    state: State<'_, AppState>,
    url_or_id: String,
) -> Result<WorkshopPreview, String> {
    fetch_workshop_preview(&state.http, &url_or_id)
        .await
        .map_err(|e| e.to_string())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopDownloadSource {
    Steam,
    #[serde(rename = "ggnetwork")]
    GgNetwork,
}

/// Downloads a Workshop item into `<app_data>/wallpapers/workshop/<id>` and adds it to the
/// library. Returns the new `wallpaperId` (`workshop:<id>`).
#[tauri::command]
pub async fn steam_download_workshop_item(
    app: AppHandle,
    state: State<'_, AppState>,
    url_or_id: String,
    source: Option<WorkshopDownloadSource>,
) -> Result<String, String> {
    let workshop_id = parse_workshop_url_or_id(&url_or_id).map_err(|e| e.to_string())?;
    let _download = state.steam.track_download(&workshop_id)?;
    let username = match source.unwrap_or(WorkshopDownloadSource::Steam) {
        WorkshopDownloadSource::Steam => Some(saved_username()?),
        WorkshopDownloadSource::GgNetwork => None,
    };
    let title = match fetch_workshop_preview(&state.http, &workshop_id).await {
        Ok(preview) => preview.title,
        Err(err) if username.is_none() => return Err(err.to_string()),
        Err(_) => workshop_id.clone(),
    };
    let out_dir = state.workshop_dir().join(&workshop_id);
    emit_progress(&app, &workshop_id, &title, 0.0, "downloading", None);

    let download = async {
        if let Some(username) = username.as_deref() {
            tokio::fs::create_dir_all(&out_dir)
                .await
                .map_err(|e| e.to_string())?;
            let args =
                pubfile_download_args(&workshop_id, username, &out_dir, DEFAULT_MAX_DOWNLOADS)
                    .map_err(|e| e.to_string())?;
            run_sidecar(&state.steam, args, false, |ev| match &ev {
                SidecarEvent::ProgressPercent(pct) => {
                    emit_progress(&app, &workshop_id, &title, *pct, "downloading", None);
                }
                other => emit_auth(&app, other),
            })
            .await?;
        } else {
            truongpaper_steam::guest_download::download_guest_workshop(
                &workshop_id,
                &out_dir,
                |pct| {
                    emit_progress(&app, &workshop_id, &title, pct, "downloading", None);
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        }

        let wallpaper_id = format!("workshop:{workshop_id}");
        let items = sync_library(&state).await?;
        if !items.iter().any(|w| w.wallpaper_id == wallpaper_id) {
            return Err("Download finished but the item has no readable project.json".to_string());
        }
        Ok(wallpaper_id)
    }
    .await;

    match download {
        Ok(wallpaper_id) => {
            emit_progress(&app, &workshop_id, &title, 100.0, "completed", None);
            if let Some(username) = username {
                tauri::async_runtime::spawn(bootstrap_core_assets_if_needed(
                    Arc::clone(&state.steam),
                    state.core_assets_root(),
                    username,
                ));
            }
            Ok(wallpaper_id)
        }
        Err(err) => {
            emit_progress(&app, &workshop_id, &title, 0.0, "error", Some(err.clone()));
            Err(err)
        }
    }
}

/// `DepotDownloader -qr`: streams `steam-auth://qr-matrix`, saves the account name on success.
#[tauri::command]
pub async fn steam_login_qr(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let mut account = None;
    run_sidecar(&state.steam, qr_login_args(), true, |ev| match &ev {
        SidecarEvent::LoggedInUsername(user) => account = Some(user.clone()),
        other => emit_auth(&app, other),
    })
    .await?;
    let account = account.ok_or_else(|| "Steam signed in, but the downloader did not report the account name. Try Username & 2FA.".to_string())?;
    VAULT.put(SECRET_STEAM_USERNAME, Some(&account))
}

/// `-username -password` login; `steam-auth://need-2fa` is emitted when Steam Guard asks.
#[tauri::command]
pub async fn steam_login_credentials(
    app: AppHandle,
    state: State<'_, AppState>,
    username: String,
    password: String,
) -> Result<(), String> {
    let args = password_login_args(&username, &password).map_err(|e| e.to_string())?;
    drop(password);
    run_sidecar(&state.steam, args, true, |ev| emit_auth(&app, &ev)).await?;
    VAULT.put(SECRET_STEAM_USERNAME, Some(username.trim()))
}

#[tauri::command]
pub async fn steam_submit_2fa_code(state: State<'_, AppState>, code: String) -> Result<(), String> {
    let code = code.trim().to_string();
    if code.is_empty() || code.len() > 32 || code.chars().any(char::is_whitespace) {
        return Err("Invalid Steam Guard code".into());
    }
    let tx = state
        .steam
        .two_fa_sender()
        .ok_or_else(|| "No Steam sign-in is waiting for a code".to_string())?;
    tx.send(code)
        .await
        .map_err(|_| "The Steam sign-in process already exited".to_string())
}

#[tauri::command]
pub async fn steam_logout(state: State<'_, AppState>) -> Result<(), String> {
    state.steam.abort_all();
    VAULT.put(SECRET_STEAM_USERNAME, None)?;
    VAULT.put(SECRET_STEAM_REFRESH_TOKEN, None)
}

#[tauri::command]
pub async fn steam_sync_local_folders(
    state: State<'_, AppState>,
) -> Result<Vec<WallpaperItem>, String> {
    sync_library(&state).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sidecar_candidate_order() {
        let exe = Path::new("/Applications/TruongPaper.app/Contents/MacOS");
        let manifest = Path::new("/repo/src-tauri");
        let suffix = std::env::consts::EXE_SUFFIX;
        let c = sidecar_candidates(Some(exe), manifest, Some("aarch64-apple-darwin"), true);
        assert_eq!(c[0], exe.join(format!("DepotDownloader{suffix}")));
        assert_eq!(
            c[1],
            manifest.join(format!("bin/DepotDownloader-aarch64-apple-darwin{suffix}"))
        );
        let release = sidecar_candidates(Some(exe), manifest, Some("x"), false);
        assert_eq!(release.len(), 1);
        assert!(sidecar_candidates(None, manifest, None, true).is_empty());
    }

    #[test]
    fn auth_events_map_to_channels() {
        let (name, payload) =
            auth_event_payload(&SidecarEvent::QrMatrix(vec![vec![true, false]])).unwrap();
        assert_eq!(name, "steam-auth://qr-matrix");
        assert_eq!(payload, json!({"matrix": [[true, false]]}));
        let (name, payload) = auth_event_payload(&SidecarEvent::Need2Fa {
            kind: "email",
            prompt: "STEAM GUARD!".into(),
        })
        .unwrap();
        assert_eq!(name, "steam-auth://need-2fa");
        assert_eq!(payload, json!({"kind": "email", "prompt": "STEAM GUARD!"}));
        assert!(auth_event_payload(&SidecarEvent::Completed).is_none());
        assert!(auth_event_payload(&SidecarEvent::ProgressPercent(3.0)).is_none());
    }

    #[tokio::test]
    async fn session_tracks_and_aborts_tasks() {
        let session = SteamSession::default();
        let task = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        let (tx, mut rx) = mpsc::channel(1);
        let id = session.register(task.abort_handle(), Some(tx));
        assert_eq!(session.running_count(), 1);
        session
            .two_fa_sender()
            .unwrap()
            .send("123".into())
            .await
            .unwrap();
        assert_eq!(rx.recv().await.as_deref(), Some("123"));
        session.abort_all();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(session.two_fa_sender().is_none());
        session.unregister(id);
        assert_eq!(session.running_count(), 0);
    }
}

/// Public, guest-visible wallpapers. Download authentication is a separate flow.
#[tauri::command]
pub async fn steam_browse_workshop(
    state: State<'_, AppState>,
    query: truongpaper_steam::WorkshopBrowseQuery,
) -> Result<truongpaper_steam::WorkshopBrowsePage, String> {
    truongpaper_steam::browse_workshop(&state.http, &query)
        .await
        .map_err(|e| e.to_string())
}
