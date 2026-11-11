//! All 13 `photo_*` Tauri IPC commands (docs/02 §6).
//!
//! The doc's inline types (`FilterMode`, `PhotoItem`, `SavedAlbum`, `PublicCatalogAlbum`,
//! `PhotoStatus`) and SQL helpers (`query_photo_item_by_id`,
//! `pop_next_queued_photo_with_exclusions`, `evict_old_cached_files_preserving_history_unlocked`)
//! are provided by `truongpaper-photo-api` / `truongpaper-core` and reused here. Those helpers
//! bind every value as a SQL parameter instead of interpolating strings.

use crate::commands::monitor::{
    live_monitor_orientations, lock_monitor_applies, set_photo_host_source,
};
use crate::host::WallpaperHost;
use crate::models::{
    IpcTransitionType, WallpaperChangedPayload, WallpaperSource, EVENT_PHOTO_QUEUE_UPDATED,
    EVENT_WALLPAPER_CHANGED,
};
use crate::state::{AppState, SETTING_MULTI_MONITOR_MODE, SETTING_SKIP_MISMATCHED_ORIENTATION};
use parking_lot::Mutex;
use reqwest::Client;
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use truongpaper_core::AppSettings;
use truongpaper_photo_api::*;
use truongpaper_renderer::{load_rgba_image, RgbaImageData, MAX_PHOTO_TEXTURE_DIMENSION};

/// How many queued photos `photo_rotate_next` tries per monitor when decoding fails.
const MAX_DECODE_ATTEMPTS: usize = 3;

fn e(err: impl std::fmt::Display) -> String {
    err.to_string()
}

#[derive(Clone)]
pub struct PhotoServiceState {
    pub db: Arc<Mutex<Connection>>,
    pub app_data_dir: PathBuf,
    pub http: Client,
    pub limiter: Arc<SlidingRateLimiter>,
    pub is_refilling: Arc<AtomicBool>,
    pub refill_mutex: Arc<tokio::sync::Mutex<()>>,
    /// Shared with `AppState` (for `transitionUsed` and the cache budget mirror).
    pub settings: Arc<Mutex<AppSettings>>,
}

impl PhotoServiceState {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        app_data_dir: PathBuf,
        http: Client,
        settings: Arc<Mutex<AppSettings>>,
    ) -> Self {
        Self {
            db,
            app_data_dir,
            http,
            limiter: Arc::new(SlidingRateLimiter::new()),
            is_refilling: Arc::new(AtomicBool::new(false)),
            refill_mutex: Arc::new(tokio::sync::Mutex::new(())),
            settings,
        }
    }

    /// `(mode, filter, max_cache_bytes, displayed_ids)` as in the doc.
    pub fn load_active_filter(
        &self,
        required_orientation: Option<String>,
    ) -> Result<(FilterMode, PhotoFilter, u64, Vec<String>), String> {
        let conn = self.db.lock();
        let (settings, filter, displayed) =
            truongpaper_photo_api::load_active_filter(&conn, required_orientation).map_err(e)?;
        Ok((
            settings.filter_mode,
            filter,
            settings.max_cache_bytes,
            displayed,
        ))
    }

    /// Builds `PhotoStatus` synchronously (no I/O beyond SQLite).
    pub fn status(&self) -> Result<PhotoStatus, String> {
        let (mode, filter, max_cache_bytes, _) = self.load_active_filter(None)?;
        let conn = self.db.lock();
        Ok(PhotoStatus {
            filter_mode: mode,
            featured_only: filter.featured_only,
            unused_queue: list_unused_queue(&conn, &self.app_data_dir, &filter).map_err(e)?,
            current_photo_by_monitor: current_photo_by_monitor(&conn, &self.app_data_dir)
                .map_err(e)?,
            cached_bytes: cached_bytes_total(&conn).map_err(e)?,
            max_cache_bytes,
            history_count: history_count(&conn).map_err(e)?,
            blacklist_count: blacklist_count(&conn).map_err(e)?,
            is_refilling: self.is_refilling.load(Ordering::Relaxed),
            rate_limit_remaining: self.limiter.remaining(),
        })
    }

    fn transition_used(&self) -> &'static str {
        IpcTransitionType::from_setting(&self.settings.lock().transition_type).as_str()
    }

    fn emit_status(&self, app: &AppHandle) {
        if let Ok(status) = self.status() {
            let _ = app.emit(EVENT_PHOTO_QUEUE_UPDATED, status);
        }
    }

    fn show_prepared(
        &self,
        app: &AppHandle,
        monitor_id: &str,
        item: &PhotoItem,
        decoded: Option<Arc<RgbaImageData>>,
    ) -> Result<(), String> {
        let source = if let Some(state) = app.try_state::<AppState>() {
            let source = set_photo_host_source(&state, monitor_id)?;
            state
                .host
                .set_photo(monitor_id, PathBuf::from(&item.file_path), decoded);
            source
        } else {
            WallpaperSource::PhotoApi
        };
        let _ = app.emit(
            EVENT_WALLPAPER_CHANGED,
            WallpaperChangedPayload {
                monitor_id,
                source: &source,
                current_item: Some(item),
                transition_used: self.transition_used(),
            },
        );
        Ok(())
    }

    fn pop(&self, filter: &PhotoFilter, displayed: &[String]) -> Result<Option<PhotoItem>, String> {
        let conn = self.db.lock();
        pop_next_queued_photo_with_exclusions(&conn, &self.app_data_dir, filter, displayed)
            .map_err(e)
    }

    async fn refill(
        &self,
        filter: &PhotoFilter,
        max_cache_bytes: u64,
        displayed: &[String],
    ) -> PhotoApiResult<RefillOutcome> {
        refill_queue(
            &self.http,
            &self.limiter,
            &self.db,
            &self.app_data_dir,
            filter,
            max_cache_bytes,
            displayed,
        )
        .await
    }
}

/// Monitors that `photo_rotate_next` updates.
///
/// - `requested = Some(id)`: just that monitor (orientation from the live list, else landscape).
/// - Otherwise every live monitor whose source is `photo_api` (unassigned monitors default to
///   `photo_api`); without live monitors, assigned `photo_api` rows; finally `DISPLAY-1`.
pub fn select_rotation_targets(
    live: &[(String, bool)],
    assigned: &HashMap<String, WallpaperSource>,
    requested: Option<&str>,
) -> Vec<(String, bool)> {
    if let Some(id) = requested {
        let portrait = live
            .iter()
            .find(|(m, _)| m == id)
            .map(|(_, p)| *p)
            .unwrap_or(false);
        return vec![(id.to_string(), portrait)];
    }
    let is_photo = |id: &str| matches!(assigned.get(id), None | Some(WallpaperSource::PhotoApi));
    if !live.is_empty() {
        return live
            .iter()
            .filter(|(id, _)| is_photo(id))
            .cloned()
            .collect();
    }
    let mut rows: Vec<(String, bool)> = assigned
        .iter()
        .filter(|(_, s)| matches!(s, WallpaperSource::PhotoApi))
        .map(|(id, _)| (id.clone(), false))
        .collect();
    rows.sort();
    if rows.is_empty() && assigned.is_empty() {
        rows.push(("DISPLAY-1".to_string(), false));
    }
    rows
}

/// Decodes a photo for the desktop renderer *before* it is recorded in `photos_history`
/// (roadmap rule: history only after the desktop swap succeeds). Without an attached renderer
/// nothing is decoded and `Ok(None)` is returned (history is recorded as before).
pub(crate) fn decode_photo_for_display(
    renderer_attached: bool,
    path: &Path,
) -> Result<Option<Arc<RgbaImageData>>, String> {
    if !renderer_attached {
        return Ok(None);
    }
    load_rgba_image(path, MAX_PHOTO_TEXTURE_DIMENSION)
        .map(|img| Some(Arc::new(img)))
        .map_err(|err| format!("photo {} could not be decoded: {err}", path.display()))
}

/// Takes a cached photo that failed to decode out of the unused queue (`file_present = 0`, the
/// same state as a cache eviction; `photos_history` is untouched) and returns its file for
/// deletion, so later rotations do not pick and decode it again. A later refill may download
/// it again, which replaces a truncated or corrupt download.
pub(crate) fn evict_undecodable_photo(
    conn: &Connection,
    item: &PhotoItem,
) -> Result<PathBuf, String> {
    truongpaper_core::mark_file_evicted(conn, &item.id).map_err(e)?;
    Ok(PathBuf::from(&item.file_path))
}

/// [`evict_undecodable_photo`] plus the file removal (outside the SQLite mutex).
async fn drop_undecodable_photo(state: &PhotoServiceState, item: &PhotoItem) {
    let evicted = {
        let conn = state.db.lock();
        evict_undecodable_photo(&conn, item)
    };
    match evicted {
        Ok(path) => {
            let _ = tokio::fs::remove_file(path).await;
        }
        Err(err) => tracing::warn!("could not evict undecodable photo {}: {err}", item.id),
    }
}

async fn prepare_photo(
    host: Option<&Arc<WallpaperHost>>,
    item: &PhotoItem,
) -> Result<Option<Arc<RgbaImageData>>, String> {
    let attached = host.is_some_and(|h| h.live_renderer_attached());
    if !attached {
        return Ok(None);
    }
    let path = PathBuf::from(&item.file_path);
    tokio::task::spawn_blocking(move || decode_photo_for_display(true, &path))
        .await
        .map_err(e)?
}

/// Decodes `path` on a blocking worker for the desktop renderer (keeps photo decodes off the
/// render thread). `None` without a renderer or on failure; the render thread then decodes the
/// path itself as a fallback.
pub(crate) async fn decode_for_desktop(
    host: &WallpaperHost,
    path: &Path,
) -> Option<Arc<RgbaImageData>> {
    if !host.live_renderer_attached() {
        return None;
    }
    let path = path.to_path_buf();
    match tokio::task::spawn_blocking(move || decode_photo_for_display(true, &path)).await {
        Ok(Ok(decoded)) => decoded,
        Ok(Err(err)) => {
            tracing::warn!("{err}");
            None
        }
        Err(err) => {
            tracing::warn!("photo decode task: {err}");
            None
        }
    }
}

/// 1. `photo_get_status`
#[tauri::command]
pub async fn photo_get_status(state: State<'_, PhotoServiceState>) -> Result<PhotoStatus, String> {
    state.status()
}

/// 2. `photo_fetch_public_catalog`
#[tauri::command]
pub async fn photo_fetch_public_catalog(
    state: State<'_, PhotoServiceState>,
) -> Result<Vec<PublicCatalogAlbum>, String> {
    PhotoApiClient::new(&state.http)
        .fetch_full_catalog()
        .await
        .map_err(e)
}

/// 3. `photo_rotate_next` (Cloned vs. Independent multi-monitor + orientation fallback)
#[tauri::command]
pub async fn photo_rotate_next(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    monitor_id: Option<String>,
) -> Result<PhotoItem, String> {
    let item = rotate_photo_next(&app, &state, monitor_id.clone()).await?;
    crate::scheduler::note_manual_rotation(monitor_id.as_deref(), false);
    Ok(item)
}

/// Body of [`photo_rotate_next`], shared with the rotation scheduler (which must not reset its
/// own timers through [`crate::scheduler::note_manual_rotation`]).
pub(crate) async fn rotate_photo_next(
    app: &AppHandle,
    state: &PhotoServiceState,
    monitor_id: Option<String>,
) -> Result<PhotoItem, String> {
    let live = live_monitor_orientations(app);
    let assigned = crate::db::monitor_sources(&state.db.lock())?;
    let targets = select_rotation_targets(&live, &assigned, monitor_id.as_deref());
    // Acquire every target in stable order, including clone-mode draws. Re-read the source
    // after waiting: a queued photo command must not overwrite a Workshop apply.
    let _apply = lock_monitor_applies(targets.iter().map(|(id, _)| id.as_str())).await;
    let current = crate::db::monitor_sources(&state.db.lock())?;
    let host = app.try_state::<AppState>().map(|s| Arc::clone(&s.host));
    let targets = targets
        .into_iter()
        .filter(|(id, _)| {
            let source = current.get(id).cloned().unwrap_or_default();
            let unchanged = assigned.get(id).cloned().unwrap_or_default() == source;
            let photo_runtime = host
                .as_ref()
                .and_then(|h| h.monitor(id))
                .is_some_and(|rt| matches!(rt.active, crate::host::ActiveRuntime::Photo { .. }));
            unchanged && photo_rotation_allowed(&source, monitor_id.is_some(), photo_runtime)
        })
        .collect();
    rotate_photo_targets(app, state, monitor_id, targets).await
}

/// Only called by `show_photo` while its caller holds this monitor's apply lock.
pub(crate) async fn rotate_photo_for_locked_monitor(
    app: &AppHandle,
    state: &PhotoServiceState,
    monitor_id: &str,
) -> Result<PhotoItem, String> {
    let live = live_monitor_orientations(app);
    let targets = select_rotation_targets(&live, &HashMap::new(), Some(monitor_id));
    rotate_photo_targets(app, state, Some(monitor_id.to_string()), targets).await
}

fn photo_rotation_allowed(source: &WallpaperSource, explicit: bool, photo_runtime: bool) -> bool {
    matches!(source, WallpaperSource::PhotoApi)
        || (explicit && photo_runtime && matches!(source, WallpaperSource::Playlist { .. }))
}

async fn rotate_photo_targets(
    app: &AppHandle,
    state: &PhotoServiceState,
    monitor_id: Option<String>,
    target_monitors: Vec<(String, bool)>,
) -> Result<PhotoItem, String> {
    if target_monitors.is_empty() {
        return Err("No monitor is showing the Photos API source".into());
    }
    let (_, mut base_filter, max_cache_bytes, mut displayed_ids) =
        state.load_active_filter(None)?;
    let host = app.try_state::<AppState>().map(|s| Arc::clone(&s.host));
    let (skip_mismatched, multi_monitor_mode) = {
        let conn = state.db.lock();
        let skip: bool = get_setting(&conn, SETTING_SKIP_MISMATCHED_ORIENTATION)
            .map_err(e)?
            .unwrap_or(true);
        let mode: String = get_setting(&conn, SETTING_MULTI_MONITOR_MODE)
            .map_err(e)?
            .unwrap_or_else(|| "independent".to_string());
        (skip, mode)
    };

    let mut last_applied_item: Option<PhotoItem> = None;

    if multi_monitor_mode == "clone" && monitor_id.is_none() {
        base_filter.required_orientation = None;
        let mut item =
            photo_for_rotation(state, &mut base_filter, max_cache_bytes, &displayed_ids).await?;
        let mut attempts = 0;
        let (chosen, decoded) = loop {
            let chosen = item.ok_or_else(|| "No eligible photos available in queue".to_string())?;
            match prepare_photo(host.as_ref(), &chosen).await {
                Ok(decoded) => break (chosen, decoded),
                Err(err) => {
                    // Not recorded as shown: the desktop never displayed it. Evicted so the
                    // next rotation does not pick it again.
                    tracing::warn!("{err}; skipping");
                    drop_undecodable_photo(state, &chosen).await;
                    attempts += 1;
                    if attempts >= MAX_DECODE_ATTEMPTS {
                        return Err(err);
                    }
                    displayed_ids.push(chosen.id.clone());
                    item = state.pop(&base_filter, &displayed_ids)?;
                }
            }
        };
        {
            let mut conn = state.db.lock();
            mark_photo_shown(&mut conn, &chosen.id).map_err(e)?;
            for (mid, _) in &target_monitors {
                assign_photo_to_monitor(&conn, mid, &chosen.id).map_err(e)?;
            }
        }
        for (mid, _) in &target_monitors {
            state.show_prepared(app, mid, &chosen, decoded.clone())?;
        }
        last_applied_item = Some(chosen);
    } else {
        for (mid, is_portrait) in &target_monitors {
            base_filter.required_orientation = skip_mismatched.then(|| {
                if *is_portrait {
                    "portrait".to_string()
                } else {
                    "landscape".to_string()
                }
            });

            let mut item =
                photo_for_rotation(state, &mut base_filter, max_cache_bytes, &displayed_ids)
                    .await?;

            let mut attempts = 0;
            while let Some(chosen) = item.take() {
                let decoded = match prepare_photo(host.as_ref(), &chosen).await {
                    Ok(decoded) => decoded,
                    Err(err) => {
                        // Not recorded as shown: the desktop never displayed it. Evicted so the
                        // next rotation does not pick it again.
                        tracing::warn!("{err}; skipping for {mid}");
                        drop_undecodable_photo(state, &chosen).await;
                        displayed_ids.push(chosen.id.clone());
                        attempts += 1;
                        if attempts < MAX_DECODE_ATTEMPTS {
                            item = state.pop(&base_filter, &displayed_ids)?;
                        }
                        continue;
                    }
                };
                {
                    let mut conn = state.db.lock();
                    mark_photo_shown(&mut conn, &chosen.id).map_err(e)?;
                    assign_photo_to_monitor(&conn, mid, &chosen.id).map_err(e)?;
                }
                displayed_ids.push(chosen.id.clone());
                state.show_prepared(app, mid, &chosen, decoded)?;
                last_applied_item = Some(chosen);
            }
        }
    }

    let unused_remaining = {
        let conn = state.db.lock();
        base_filter.required_orientation = None;
        count_unused_in_queue(&conn, &base_filter).unwrap_or(0)
    };
    state.emit_status(app);
    if needs_refill(unused_remaining) && last_applied_item.is_some() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(state) = app.try_state::<PhotoServiceState>() {
                let _ = trigger_refill(&app, &state).await;
            }
        });
    }

    last_applied_item.ok_or_else(|| "No eligible photo found".to_string())
}

fn cached_photo_for_rotation(
    state: &PhotoServiceState,
    filter: &mut PhotoFilter,
    displayed_ids: &[String],
) -> Result<Option<PhotoItem>, String> {
    let mut item = state.pop(filter, displayed_ids)?;
    if item.is_none() && filter.required_orientation.is_some() {
        // Fill modes handle the aspect difference when no matching cached photo is ready.
        let mut fallback = filter.clone();
        fallback.required_orientation = None;
        item = state.pop(&fallback, displayed_ids)?;
        if item.is_some() {
            *filter = fallback;
        }
    }
    Ok(item)
}

async fn photo_for_rotation(
    state: &PhotoServiceState,
    filter: &mut PhotoFilter,
    max_cache_bytes: u64,
    displayed_ids: &[String],
) -> Result<Option<PhotoItem>, String> {
    if let Some(item) = cached_photo_for_rotation(state, filter, displayed_ids)? {
        return Ok(Some(item));
    }
    let _refill = state.refill_mutex.lock().await;
    if let Some(item) = cached_photo_for_rotation(state, filter, displayed_ids)? {
        return Ok(Some(item));
    }
    let _ = state.refill(filter, max_cache_bytes, displayed_ids).await;
    cached_photo_for_rotation(state, filter, displayed_ids)
}

/// 4. `photo_trigger_refill`
#[tauri::command]
pub async fn photo_trigger_refill(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
) -> Result<(), String> {
    trigger_refill(&app, &state).await
}

/// Body of [`photo_trigger_refill`] (callable without a `State` guard).
pub(crate) async fn trigger_refill(
    app: &AppHandle,
    state: &PhotoServiceState,
) -> Result<(), String> {
    let Ok(_guard) = state.refill_mutex.try_lock() else {
        return Ok(());
    };
    state.is_refilling.store(true, Ordering::Relaxed);
    state.emit_status(app);

    let res = match state.load_active_filter(None) {
        Ok((_, filter, max_cache_bytes, displayed_ids)) => state
            .refill(&filter, max_cache_bytes, &displayed_ids)
            .await
            .map(|_| ())
            .map_err(e),
        Err(err) => Err(err),
    };

    state.is_refilling.store(false, Ordering::Relaxed);
    state.emit_status(app);
    res
}

/// 5. `photo_add_album`
#[tauri::command]
pub async fn photo_add_album(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    url_or_slug: String,
    display_name: Option<String>,
) -> Result<SavedAlbum, String> {
    let slug =
        parse_album_slug(&url_or_slug).ok_or_else(|| "Invalid album URL or slug".to_string())?;

    if !state.limiter.try_acquire() {
        return Err("Photo API request budget reached; try again in a few minutes".into());
    }
    let client = PhotoApiClient::new(&state.http);
    let sample = client
        .fetch_random(&RandomPhotoQuery {
            album: Some(slug.clone()),
            featured: false,
        })
        .await
        .map_err(|err| match err {
            PhotoApiError::NoPhotos | PhotoApiError::InvalidParameters => {
                format!("Album '{slug}' not found or has no public photos")
            }
            other => other.to_string(),
        })?;

    let catalog_entry = client
        .fetch_full_catalog()
        .await
        .ok()
        .and_then(|items| items.into_iter().find(|a| a.slug == slug));

    let title = display_name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| catalog_entry.as_ref().map(|c| c.title.clone()))
        .unwrap_or_else(|| slug.replace('-', " "));
    let cover_thumb = catalog_entry
        .as_ref()
        .map(|c| c.cover_thumbnail_url.clone())
        .or_else(|| Some(sample.image_url.clone()));
    let image_count = catalog_entry.as_ref().map(|c| c.image_count);
    let added_at = now_ms();

    {
        let conn = state.db.lock();
        upsert_saved_album(
            &conn,
            &SavedAlbumUpsert {
                album_slug: slug.clone(),
                title: title.clone(),
                description: catalog_entry.as_ref().map(|c| c.description.clone()),
                category: catalog_entry.as_ref().map(|c| c.category.clone()),
                album_url: sample.album_url.clone(),
                cover_thumbnail_url: cover_thumb.clone(),
                cover_blurhash: catalog_entry.as_ref().map(|c| c.cover_blurhash.clone()),
                image_count,
                added_at,
            },
        )
        .map_err(e)?;
    }

    state.emit_status(&app);

    Ok(SavedAlbum {
        album_slug: slug,
        title,
        album_url: sample.album_url,
        is_enabled: true,
        cover_thumbnail_url: cover_thumb,
        image_count,
        added_at,
    })
}

/// 6. `photo_toggle_album`
#[tauri::command]
pub async fn photo_toggle_album(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    album_slug: String,
    enabled: bool,
) -> Result<(), String> {
    {
        let conn = state.db.lock();
        set_album_enabled(&conn, &album_slug, enabled).map_err(e)?;
    }
    let _ = photo_trigger_refill(app, state).await;
    Ok(())
}

/// 7. `photo_remove_album`
#[tauri::command]
pub async fn photo_remove_album(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    album_slug: String,
) -> Result<(), String> {
    {
        let conn = state.db.lock();
        remove_saved_album(&conn, &album_slug).map_err(e)?;
    }
    state.emit_status(&app);
    Ok(())
}

/// 8. `photo_set_filter`
#[tauri::command]
pub async fn photo_set_filter(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    mode: FilterMode,
    featured_only: bool,
) -> Result<(), String> {
    {
        let conn = state.db.lock();
        save_photo_filter(&conn, mode, featured_only).map_err(e)?;
    }
    let _ = photo_trigger_refill(app, state).await;
    Ok(())
}

/// 9. `photo_set_favorite`
#[tauri::command]
pub async fn photo_set_favorite(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    photo_id: String,
    favorite: bool,
) -> Result<(), String> {
    {
        let conn = state.db.lock();
        set_favorite(&conn, &photo_id, favorite).map_err(e)?;
    }
    state.emit_status(&app);
    Ok(())
}

/// 10. `photo_blacklist_and_skip`
#[tauri::command]
pub async fn photo_blacklist_and_skip(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    photo_id: String,
) -> Result<(), String> {
    let (file_to_delete, affected_monitors) = {
        let conn = state.db.lock();
        let path = blacklist_photo(&conn, &state.app_data_dir, &photo_id).map_err(e)?;
        let mons = monitors_showing_photo(&conn, &photo_id).map_err(e)?;
        (path, mons)
    };

    if let Some(path) = file_to_delete {
        let _ = tokio::fs::remove_file(path).await;
    }

    for mid in affected_monitors {
        let _ = photo_rotate_next(app.clone(), state.clone(), Some(mid)).await;
    }

    state.emit_status(&app);
    Ok(())
}

/// 11. `photo_set_cache_limit`
#[tauri::command]
pub async fn photo_set_cache_limit(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    max_bytes: u64,
) -> Result<(), String> {
    let displayed_ids = {
        let conn = state.db.lock();
        save_max_cache_bytes(&conn, max_bytes).map_err(e)?;
        list_displayed_photo_ids(&conn).map_err(e)?
    };
    state.settings.lock().max_photo_cache_bytes = max_bytes;

    let db = Arc::clone(&state.db);
    let dir = state.app_data_dir.clone();
    tokio::task::spawn_blocking(move || {
        evict_old_cached_files_preserving_history_unlocked(&db, &dir, 0, max_bytes, &displayed_ids)
    })
    .await
    .map_err(e)?
    .map_err(e)?;

    state.emit_status(&app);
    Ok(())
}

/// 12. `photo_clear_cache` (deletes non-favorite, non-displayed files; NEVER touches photos_history)
#[tauri::command]
pub async fn photo_clear_cache(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
) -> Result<(), String> {
    let files_to_remove = {
        let conn = state.db.lock();
        let displayed_ids = list_displayed_photo_ids(&conn).map_err(e)?;
        plan_clear_cache(&conn, &state.app_data_dir, &displayed_ids).map_err(e)?
    };

    for (id, full_path) in files_to_remove {
        let _ = tokio::fs::remove_file(&full_path).await;
        let conn = state.db.lock();
        let _ = mark_file_evicted(&conn, &id);
    }

    state.emit_status(&app);
    Ok(())
}

/// 13. `photo_reset_history` (one album or all, preserving on-screen ids; re-queues cached files)
#[tauri::command]
pub async fn photo_reset_history(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    album_slug: Option<String>,
) -> Result<(), String> {
    {
        let mut conn = state.db.lock();
        let displayed_ids = list_displayed_photo_ids(&conn).map_err(e)?;
        let filter = PhotoFilter {
            album_slugs: album_slug.into_iter().collect(),
            featured_only: false,
            required_orientation: None,
            auto_reset_history_on_exhaustion: true,
        };
        reset_exhausted_filter_history(&mut conn, &filter, &displayed_ids).map_err(e)?;
    }

    let _ = photo_trigger_refill(app, state).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_db;

    fn service() -> PhotoServiceState {
        PhotoServiceState::new(
            Arc::new(Mutex::new(test_db())),
            std::env::temp_dir().join("truongpaper-photo-cmd-test"),
            Client::new(),
            Arc::new(Mutex::new(AppSettings::default())),
        )
    }

    #[test]
    fn photo_rotation_does_not_replace_workshop_or_playlist_scene() {
        let workshop = WallpaperSource::Workshop {
            wallpaper_id: "w".into(),
        };
        let playlist = WallpaperSource::Playlist {
            playlist_id: "p".into(),
        };
        assert!(!photo_rotation_allowed(&workshop, true, true));
        assert!(!photo_rotation_allowed(&playlist, true, false));
        assert!(!photo_rotation_allowed(&playlist, false, true));
        assert!(photo_rotation_allowed(&playlist, true, true));
        assert!(photo_rotation_allowed(
            &WallpaperSource::PhotoApi,
            false,
            false
        ));
    }

    #[test]
    fn status_reflects_filter_and_counters() {
        let svc = service();
        {
            let conn = svc.db.lock();
            save_photo_filter(&conn, FilterMode::Featured, false).unwrap();
            conn.execute("INSERT INTO photos_blacklist VALUES ('x', NULL, 1)", [])
                .unwrap();
            assign_photo_to_monitor(&conn, "DISPLAY-1", "missing").unwrap();
        }
        let status = svc.status().unwrap();
        assert_eq!(status.filter_mode, FilterMode::Featured);
        assert!(status.featured_only);
        assert_eq!(status.blacklist_count, 1);
        assert_eq!(status.history_count, 0);
        assert!(status.unused_queue.is_empty());
        assert_eq!(
            status.current_photo_by_monitor.get("DISPLAY-1"),
            Some(&None)
        );
        assert_eq!(status.rate_limit_remaining, RATE_LIMIT_MAX_REQUESTS);
        assert_eq!(status.max_cache_bytes, DEFAULT_MAX_CACHE_BYTES);

        let json = serde_json::to_value(&status).unwrap();
        for key in [
            "filterMode",
            "featuredOnly",
            "unusedQueue",
            "currentPhotoByMonitor",
            "cachedBytes",
            "maxCacheBytes",
            "historyCount",
            "blacklistCount",
            "isRefilling",
            "rateLimitRemaining",
        ] {
            assert!(json.get(key).is_some(), "PhotoStatus missing {key}");
        }
    }

    #[test]
    fn rotation_targets() {
        let live = vec![("A".to_string(), false), ("B".to_string(), true)];
        let mut assigned = HashMap::new();
        assert_eq!(select_rotation_targets(&live, &assigned, None), live);
        assigned.insert(
            "A".to_string(),
            WallpaperSource::Workshop {
                wallpaper_id: "w".into(),
            },
        );
        assert_eq!(
            select_rotation_targets(&live, &assigned, None),
            vec![("B".to_string(), true)]
        );
        assert_eq!(
            select_rotation_targets(&live, &assigned, Some("B")),
            vec![("B".to_string(), true)]
        );
        assert_eq!(
            select_rotation_targets(&[], &assigned, Some("Z")),
            vec![("Z".to_string(), false)]
        );
        // Headless: fall back to assigned photo rows, then DISPLAY-1.
        assert!(select_rotation_targets(&[], &assigned, None).is_empty());
        assigned.insert("C".to_string(), WallpaperSource::PhotoApi);
        assert_eq!(
            select_rotation_targets(&[], &assigned, None),
            vec![("C".to_string(), false)]
        );
        assert_eq!(
            select_rotation_targets(&[], &HashMap::new(), None),
            vec![("DISPLAY-1".to_string(), false)]
        );
    }

    /// 1x1 RGBA PNG.
    const TINY_PNG: [u8; 70] = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 100, 96, 248, 95, 15,
        0, 2, 135, 1, 128, 235, 71, 186, 146, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    #[test]
    fn photos_are_decoded_before_history_only_when_the_renderer_is_attached() {
        let dir =
            std::env::temp_dir().join(format!("truongpaper-photo-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("ok.png");
        let bad = dir.join("bad.jpg");
        std::fs::write(&good, TINY_PNG).unwrap();
        std::fs::write(&bad, b"not a jpeg").unwrap();

        // No renderer: nothing is decoded, history is recorded as before.
        assert!(decode_photo_for_display(false, &bad).unwrap().is_none());
        assert!(decode_photo_for_display(false, &dir.join("missing.jpg"))
            .unwrap()
            .is_none());

        // Renderer attached: decode first; a failure means "do not mark shown".
        let img = decode_photo_for_display(true, &good).unwrap().unwrap();
        assert_eq!((img.width, img.height), (1, 1));
        assert_eq!(img.pixels.len(), 4);
        assert!(decode_photo_for_display(true, &bad).is_err());
        assert!(decode_photo_for_display(true, &dir.join("missing.jpg")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn undecodable_photos_leave_the_queue_without_entering_history() {
        let svc = service();
        let conn = svc.db.lock();
        upsert_cached_photo(
            &conn,
            &NewCachedPhoto {
                id: "corrupt".into(),
                rel_path: "photos/corrupt.jpg".into(),
                album_slug: "a".into(),
                album_url: "https://example.invalid/a".into(),
                featured: false,
                fallback_used: false,
                width: 1920,
                height: 1080,
                bytes: 10,
                downloaded_at: 1,
            },
        )
        .unwrap();
        let item = query_photo_item_by_id(&conn, &svc.app_data_dir, "corrupt")
            .unwrap()
            .unwrap();
        let unused = |conn: &Connection| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM photos_cache WHERE id = 'corrupt' AND file_present = 1",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(unused(&conn), 1);
        let path = evict_undecodable_photo(&conn, &item).unwrap();
        assert_eq!(path, PathBuf::from(&item.file_path));
        assert_eq!(unused(&conn), 0, "never popped again");
        let history: i64 = conn
            .query_row("SELECT COUNT(*) FROM photos_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(history, 0, "it was never shown");
    }
}
