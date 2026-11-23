//! Photo API and local photo cache IPC commands.
//!
//! The doc's inline types (`FilterMode`, `PhotoItem`, `SavedAlbum`, `PublicCatalogAlbum`,
//! `PhotoStatus`) and SQL helpers (`query_photo_item_by_id`,
//! `pick_cached_rotation_photo`, `evict_old_cached_files_preserving_history_unlocked`)
//! are provided by `truongpaper-photo-api` / `truongpaper-core` and reused here. Those helpers
//! bind every value as a SQL parameter instead of interpolating strings.

use crate::commands::monitor::{
    live_monitor_orientations, lock_monitor_applies, lock_monitor_apply, set_photo_host_source,
    validate_monitor_id,
};
use crate::host::WallpaperHost;
use crate::models::{
    IpcTransitionType, WallpaperChangedPayload, WallpaperSource, EVENT_PHOTO_QUEUE_UPDATED,
    EVENT_WALLPAPER_CHANGED,
};
use crate::state::{AppState, SETTING_SKIP_MISMATCHED_ORIENTATION};
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
use truongpaper_renderer::{RgbaImageData, MAX_PHOTO_TEXTURE_DIMENSION};

/// How many queued photos `photo_rotate_next` tries per monitor when decoding fails.
const MAX_DECODE_ATTEMPTS: usize = 3;
/// Photos kept on disk per filter and display orientation before the daily download budget
/// applies (a new install or a new album is not stuck on one photo for a day).
const MIN_CACHED_PHOTOS: usize = 3;
/// How long automatic refills leave a filter alone after it had nothing new.
const QUIET_REFILL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// Refill outcomes after which asking again soon would only repeat the same API calls.
fn found_nothing_new(outcome: &RefillOutcome) -> bool {
    matches!(
        outcome,
        RefillOutcome::FilterExhaustedHoldingLast
            | RefillOutcome::SmallPoolFullyQueued { .. }
            | RefillOutcome::RetryCapReached { .. }
    )
}

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
    /// Filters whose last refill found nothing new to download, with when. Automatic refills
    /// skip them for [`QUIET_REFILL`] instead of asking the API again after every photo.
    pub quiet_filters: Arc<Mutex<HashMap<String, std::time::Instant>>>,
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
            quiet_filters: Arc::default(),
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
        let unused_queue = list_unused_queue(&conn, &self.app_data_dir, &filter).map_err(e)?;
        let offline = self.limiter.is_offline();
        Ok(PhotoStatus {
            filter_mode: mode,
            featured_only: filter.featured_only,
            showing_cached_only: offline && unused_queue.is_empty(),
            offline,
            unused_queue,
            favorite_photos: list_favorite_photos(&conn, &self.app_data_dir).map_err(e)?,
            current_photo_by_monitor: current_photo_by_monitor(&conn, &self.app_data_dir)
                .map_err(e)?,
            cached_bytes: cached_bytes_total(&conn).map_err(e)?,
            max_cache_bytes,
            history_count: history_count(&conn).map_err(e)?,
            blacklist_count: blacklist_count(&conn).map_err(e)?,
            is_refilling: self.is_refilling.load(Ordering::Relaxed),
            rate_limit_remaining: self.limiter.remaining(),
            downloads_today: photo_downloads_on(&conn, &local_day()).unwrap_or(0),
            downloads_per_day: self.settings.lock().photo_downloads_per_day,
            album_titles: crate::photo_caption::all_album_titles(&conn),
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
            set_host_caption(app, &state, item);
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

    fn pop(
        &self,
        filter: &PhotoFilter,
        displayed: &[String],
        pool: RotationPhotoPool,
    ) -> Result<Option<PhotoItem>, String> {
        loop {
            let item = {
                let conn = self.db.lock();
                // Favorites / PreviouslyShown keep the last N shown photos out of the pick so
                // the same few never alternate; if that leaves nothing, retry without it.
                let mut excluded = displayed.to_vec();
                let recent = recent_shown_exclusions(&conn, filter, pool).map_err(e)?;
                excluded.extend(recent.iter().cloned());
                let mut item =
                    pick_cached_rotation_photo(&conn, &self.app_data_dir, filter, &excluded, pool)
                        .map_err(e)?;
                if item.is_none() && !recent.is_empty() {
                    item = pick_cached_rotation_photo(
                        &conn,
                        &self.app_data_dir,
                        filter,
                        displayed,
                        pool,
                    )
                    .map_err(e)?;
                }
                item
            };
            let Some(mut item) = item else {
                return Ok(None);
            };
            let Some((width, height)) = inspect_display_dimensions(Path::new(&item.file_path))
            else {
                return Ok(Some(item)); // The decoder will handle unreadable files.
            };
            let orientation = Orientation::from_dimensions(width, height);
            if item.width != Some(width)
                || item.height != Some(height)
                || filter
                    .orientation()
                    .is_some_and(|required| required != orientation)
            {
                self.db.lock().execute(
                    "UPDATE photos_cache SET width = ?1, height = ?2, orientation = ?3 WHERE id = ?4",
                    rusqlite::params![width, height, orientation.as_str(), item.id],
                ).map_err(e)?;
                item.width = Some(width);
                item.height = Some(height);
            }
            if filter
                .orientation()
                .is_none_or(|required| required == orientation)
            {
                return Ok(Some(item));
            }
        }
    }

    fn rotation_candidate(
        &self,
        filter: &PhotoFilter,
        displayed: &[String],
        prefer_favorite: bool,
    ) -> Result<Option<PhotoItem>, String> {
        self.rotation_candidate_with(filter, displayed, prefer_favorite)
    }

    /// Full-size downloads a refill for `filter` may make now: `usize::MAX` when unlimited (`0`),
    /// otherwise what is left of today's budget or enough to have [`MIN_CACHED_PHOTOS`] on disk.
    fn download_budget(&self, filter: &PhotoFilter) -> usize {
        let per_day = self.settings.lock().photo_downloads_per_day as usize;
        if per_day == 0 {
            return usize::MAX;
        }
        let conn = self.db.lock();
        let today = photo_downloads_on(&conn, &local_day()).unwrap_or(0) as usize;
        let cached = count_cached_for_filter(&conn, filter).unwrap_or(0);
        per_day
            .saturating_sub(today)
            .max(MIN_CACHED_PHOTOS.saturating_sub(cached))
    }

    /// Unseen photos first (or a favorite when `prefer_favorite` is set), without falling back
    /// to already-shown cache entries so rotation can refill unseen photos from the API first.
    fn fresh_rotation_candidate(
        &self,
        filter: &PhotoFilter,
        displayed: &[String],
        prefer_favorite: bool,
    ) -> Result<Option<PhotoItem>, String> {
        use RotationPhotoPool::{Favorites, Unused};
        for pool in [prefer_favorite.then_some(Favorites), Some(Unused)]
            .into_iter()
            .flatten()
        {
            if let Some(item) = self.pop(filter, displayed, pool)? {
                return Ok(Some(item));
            }
        }
        Ok(None)
    }

    /// New photos first, then photos already on disk in a shuffled order, so rotation never
    /// runs dry while anything matching is cached.
    fn rotation_candidate_with(
        &self,
        filter: &PhotoFilter,
        displayed: &[String],
        prefer_favorite: bool,
    ) -> Result<Option<PhotoItem>, String> {
        use RotationPhotoPool::{Favorites, PreviouslyShown, Unused};
        for pool in [
            prefer_favorite.then_some(Favorites),
            Some(Unused),
            Some(PreviouslyShown),
            Some(Favorites),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(item) = self.pop(filter, displayed, pool)? {
                return Ok(Some(item));
            }
        }
        Ok(None)
    }

    async fn refill(
        &self,
        filter: &PhotoFilter,
        max_cache_bytes: u64,
        displayed: &[String],
        force_deck_refresh: bool,
    ) -> PhotoApiResult<RefillOutcome> {
        let key = format!("{filter:?}");
        if !force_deck_refresh {
            let quiet = self
                .quiet_filters
                .lock()
                .get(&key)
                .is_some_and(|at| at.elapsed() < QUIET_REFILL);
            if quiet {
                return Ok(RefillOutcome::FilterExhaustedHoldingLast);
            }
        }
        let budget = self.download_budget(filter);
        let outcome = refill_queue_with_deck(
            &self.http,
            &self.limiter,
            &self.db,
            &self.app_data_dir,
            filter,
            max_cache_bytes,
            displayed,
            budget,
            force_deck_refresh,
        )
        .await;
        if outcome.as_ref().is_ok_and(found_nothing_new) {
            self.quiet_filters
                .lock()
                .insert(key, std::time::Instant::now());
        } else if outcome.is_ok() {
            self.quiet_filters.lock().remove(&key);
        }
        outcome
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
    max_display: Option<(u32, u32)>,
) -> Result<Option<Arc<RgbaImageData>>, String> {
    if !renderer_attached {
        return Ok(None);
    }
    // Shrunk to the display size before it is rotated and converted, so the full-size picture
    // exists only once, briefly.
    decode_photo_for_wgpu_fitted(path, MAX_PHOTO_TEXTURE_DIMENSION, |src| match max_display {
        Some(cap) => truongpaper_renderer::photo_display_target(src, cap),
        None => src,
    })
    .map(|image| {
        Some(Arc::new(RgbaImageData {
            width: image.width,
            height: image.height,
            pixels: image.rgba.into_raw(),
        }))
    })
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
            let _ = tokio::task::spawn_blocking(move || remove_cached_photo_files(&path)).await;
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
    let cap = host.and_then(|h| h.photo_decode_cap());
    tokio::task::spawn_blocking(move || decode_photo_for_display(true, &path, cap))
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
    let cap = host.photo_decode_cap();
    match tokio::task::spawn_blocking(move || decode_photo_for_display(true, &path, cap)).await {
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
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
) -> Result<Vec<PublicCatalogAlbum>, String> {
    let albums = PhotoApiClient::new(&state.http)
        .fetch_full_catalog()
        .await
        .map_err(e)?;
    remember_album_titles(&app, &albums);
    Ok(albums)
}

fn display_orientation(is_portrait: bool) -> Orientation {
    if is_portrait {
        Orientation::Portrait
    } else {
        Orientation::Landscape
    }
}

pub(crate) fn photo_matches_display(app: &AppHandle, monitor_id: &str, photo: &PhotoItem) -> bool {
    let is_portrait = live_monitor_orientations(app)
        .into_iter()
        .find(|(id, _)| id == monitor_id)
        .is_some_and(|(_, portrait)| portrait);
    inspect_display_dimensions(Path::new(&photo.file_path)).is_some_and(|(width, height)| {
        Orientation::from_dimensions(width, height) == display_orientation(is_portrait)
    })
}

/// 3. `photo_rotate_next` (cloned or independent multi-monitor rotation).
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
    rotate_photo_targets(app, state, targets, false).await
}

/// Only called by `show_photo` while its caller holds this monitor's apply lock.
pub(crate) async fn rotate_photo_for_locked_monitor(
    app: &AppHandle,
    state: &PhotoServiceState,
    monitor_id: &str,
    mix_favorites: bool,
) -> Result<PhotoItem, String> {
    let live = live_monitor_orientations(app);
    let targets = select_rotation_targets(&live, &HashMap::new(), Some(monitor_id));
    rotate_photo_targets(app, state, targets, mix_favorites).await
}

fn photo_rotation_allowed(source: &WallpaperSource, explicit: bool, photo_runtime: bool) -> bool {
    matches!(source, WallpaperSource::PhotoApi)
        || (explicit && photo_runtime && matches!(source, WallpaperSource::Playlist { .. }))
}

async fn rotate_photo_targets(
    app: &AppHandle,
    state: &PhotoServiceState,
    target_monitors: Vec<(String, bool)>,
    mix_favorites: bool,
) -> Result<PhotoItem, String> {
    if target_monitors.is_empty() {
        return Err("No monitor is showing the Photos API source".into());
    }
    let (_, mut base_filter, max_cache_bytes, mut displayed_ids) =
        state.load_active_filter(None)?;
    let host = app.try_state::<AppState>().map(|s| Arc::clone(&s.host));
    // Every display draws its own photo; "span" is a presentation mode handled by the renderer
    // (the source display's runtime is stretched), so there is no clone branch here.
    let skip_mismatched: bool = {
        let conn = state.db.lock();
        get_setting(&conn, SETTING_SKIP_MISMATCHED_ORIENTATION)
            .map_err(e)?
            .unwrap_or(true)
    };
    let mut last_applied_item: Option<PhotoItem> = None;
    let mut refill_needed = false;

    for (mid, is_portrait) in &target_monitors {
        base_filter.required_orientation =
            skip_mismatched.then(|| display_orientation(*is_portrait).as_str().into());

        // Keep the API queue first for manual Next and occasionally mix in a favorite
        // for automatic rotation. A low queue still triggers a background refill.
        let prefer_favorite = mix_favorites && rand::random::<u8>() < 64;
        let mut item = photo_for_rotation(
            state,
            &base_filter,
            max_cache_bytes,
            &displayed_ids,
            prefer_favorite,
        )
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
                        item = state.rotation_candidate(&base_filter, &displayed_ids, false)?;
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
        refill_needed |= needs_deck_refill(
            &state.db.lock(),
            &state.app_data_dir,
            &base_filter,
            &displayed_ids,
            QUEUE_REFILL_THRESHOLD,
            QUEUE_TARGET_UNUSED,
        )
        .unwrap_or(false);
    }

    state.emit_status(app);
    if refill_needed && last_applied_item.is_some() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(state) = app.try_state::<PhotoServiceState>() {
                let _ = trigger_refill(&app, &state, false).await;
            }
        });
    }

    last_applied_item.ok_or_else(no_other_photo_ready)
}

fn no_other_photo_ready() -> String {
    "No photo for this display yet. Try another photo source.".into()
}

async fn photo_for_rotation(
    state: &PhotoServiceState,
    filter: &PhotoFilter,
    max_cache_bytes: u64,
    displayed_ids: &[String],
    prefer_favorite: bool,
) -> Result<Option<PhotoItem>, String> {
    // Serve unseen photos first; only fall back to PreviouslyShown after a refill attempt
    // confirms no unseen photo can be fetched (offline, daily limit reached, or auto-reset off).
    if let Some(item) = state.fresh_rotation_candidate(filter, displayed_ids, prefer_favorite)? {
        return Ok(Some(item));
    }
    let can_refill = !state.limiter.is_offline()
        && (state.download_budget(filter) > 0 || filter.auto_reset_history_on_exhaustion);
    if can_refill {
        let _refill = state.refill_mutex.lock().await;
        if let Some(item) =
            state.fresh_rotation_candidate(filter, displayed_ids, prefer_favorite)?
        {
            return Ok(Some(item));
        }
        if let Err(err) = state
            .refill(filter, max_cache_bytes, displayed_ids, false)
            .await
        {
            tracing::warn!("photo refill failed: {err}");
        }
        if let Some(item) =
            state.fresh_rotation_candidate(filter, displayed_ids, prefer_favorite)?
        {
            return Ok(Some(item));
        }
    }
    state.rotation_candidate(filter, displayed_ids, prefer_favorite)
}

/// Gives the host the caption of the photo about to be shown. Album titles that are not known
/// yet come from the public catalog, fetched once per run in the background.
pub(crate) fn set_host_caption(app: &AppHandle, state: &AppState, item: &PhotoItem) {
    // Nothing is drawn: skip the file read. Turning a caption on refreshes the photos on screen
    // ([`refresh_photo_captions`]).
    if !state.host.overlay_prefs().any() {
        return;
    }
    let path = PathBuf::from(&item.file_path);
    let known = crate::photo_caption::known_album_title(&state.db.lock(), &item.album_slug);
    let title_known = known.is_some();
    let caption = crate::photo_caption::caption_for(&item.album_slug, known, &path);
    state.host.set_photo_caption(path, caption);
    static CATALOG_FETCHED: AtomicBool = AtomicBool::new(false);
    if !title_known
        && !item.album_slug.is_empty()
        && state.settings.lock().photo_show_album
        && !CATALOG_FETCHED.swap(true, Ordering::AcqRel)
    {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let Some(photo) = app.try_state::<PhotoServiceState>() else {
                return;
            };
            match PhotoApiClient::new(&photo.http).fetch_full_catalog().await {
                Ok(albums) => remember_album_titles(&app, &albums),
                Err(err) => {
                    tracing::debug!("album titles unavailable: {err}");
                    CATALOG_FETCHED.store(false, Ordering::Release);
                }
            }
        });
    }
}

/// Builds the captions of the photos on screen (after a caption setting was turned on).
pub(crate) fn refresh_photo_captions(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let current = {
        let conn = state.db.lock();
        current_photo_by_monitor(&conn, &state.app_data_dir).unwrap_or_default()
    };
    for item in current.into_values().flatten() {
        set_host_caption(app, &state, &item);
    }
}

/// Stores catalog titles and updates the captions already on screen.
fn remember_album_titles(app: &AppHandle, albums: &[PublicCatalogAlbum]) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    {
        let conn = state.db.lock();
        crate::photo_caption::store_album_titles(
            &conn,
            albums.iter().map(|a| (a.slug.as_str(), a.title.as_str())),
        );
    }
    let db = Arc::clone(&state.db);
    state
        .host
        .refresh_album_titles(|slug| crate::photo_caption::known_album_title(&db.lock(), slug));
}

/// 4. `photo_trigger_refill`
#[tauri::command]
pub async fn photo_trigger_refill(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
) -> Result<(), String> {
    // Asked for by the user: try again even where the last refill found nothing new.
    state.quiet_filters.lock().clear();
    trigger_refill(&app, &state, true).await
}

/// `photo_retry_now`: ends the offline / throttling backoff and refills right away.
#[tauri::command]
pub async fn photo_retry_now(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
) -> Result<(), String> {
    state.limiter.clear_backoff();
    state.quiet_filters.lock().clear();
    trigger_refill(&app, &state, true).await
}

/// Body of [`photo_trigger_refill`] (callable without a `State` guard).
pub(crate) async fn trigger_refill(
    app: &AppHandle,
    state: &PhotoServiceState,
    force_deck_refresh: bool,
) -> Result<(), String> {
    let Ok(_guard) = state.refill_mutex.try_lock() else {
        return Ok(());
    };
    state.is_refilling.store(true, Ordering::Relaxed);
    state.emit_status(app);

    let res = async {
        let (_, mut filter, max_cache_bytes, displayed_ids) = state.load_active_filter(None)?;
        let orientations = if state.settings.lock().skip_mismatched_orientation {
            let live = live_monitor_orientations(app);
            [false, true]
                .into_iter()
                .filter(|portrait| {
                    live.is_empty() && !portrait || live.iter().any(|(_, p)| p == portrait)
                })
                .map(|portrait| Some(display_orientation(portrait).as_str().to_string()))
                .collect::<Vec<_>>()
        } else {
            vec![None]
        };
        let mut first = true;
        for orientation in orientations {
            filter.required_orientation = orientation;
            // Refresh the shared filter deck at most once across portrait/landscape passes.
            state
                .refill(
                    &filter,
                    max_cache_bytes,
                    &displayed_ids,
                    force_deck_refresh && first,
                )
                .await
                .map_err(e)?;
            first = false;
        }
        Ok(())
    }
    .await;

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

/// `photo_list_albums`: the saved albums, oldest first.
#[tauri::command]
pub async fn photo_list_albums(
    state: State<'_, PhotoServiceState>,
) -> Result<Vec<SavedAlbum>, String> {
    let conn = state.db.lock();
    let rows = list_saved_albums(&conn).map_err(e)?;
    Ok(rows.iter().map(SavedAlbum::from).collect())
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

/// Explicitly reuses a pinned photo on one display, without drawing from the rotation pool.
#[tauri::command]
pub async fn photo_apply_favorite(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    runtime: State<'_, AppState>,
    monitor_id: String,
    photo_id: String,
) -> Result<(), String> {
    validate_monitor_id(&monitor_id)?;
    let _apply = lock_monitor_apply(&monitor_id).await;
    let item = {
        let conn = state.db.lock();
        let item = query_photo_item_by_id(&conn, &state.app_data_dir, &photo_id)
            .map_err(e)?
            .filter(|photo| photo.is_favorite && Path::new(&photo.file_path).is_file());
        item.ok_or("This favorite is no longer available on disk")?
    };
    let decoded = prepare_photo(Some(&runtime.host), &item).await?;
    {
        let mut conn = state.db.lock();
        let tx = conn.transaction().map_err(e)?;
        let still_available = get_cached_photo(&tx, &photo_id)
            .map_err(e)?
            .is_some_and(|photo| photo.file_present && photo.is_favorite);
        if !still_available {
            return Err("This favorite is no longer available on disk".into());
        }
        crate::db::set_monitor_source(&tx, &monitor_id, &WallpaperSource::PhotoApi)?;
        mark_photo_shown_in_transaction(&tx, &photo_id).map_err(e)?;
        assign_photo_to_monitor(&tx, &monitor_id, &photo_id).map_err(e)?;
        tx.commit().map_err(e)?;
    }
    state.show_prepared(&app, &monitor_id, &item, decoded)?;
    crate::scheduler::note_manual_rotation(Some(&monitor_id), true);
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
        let _ = tokio::task::spawn_blocking(move || remove_cached_photo_files(&path)).await;
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
        let _ = tokio::task::spawn_blocking(move || remove_cached_photo_files(&full_path)).await;
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
    fn a_filter_with_nothing_new_is_left_alone_by_automatic_refills() {
        let svc = service();
        let filter = PhotoFilter::default();
        svc.quiet_filters
            .lock()
            .insert(format!("{filter:?}"), std::time::Instant::now());
        // Answered without touching the network.
        let outcome = tauri::async_runtime::block_on(svc.refill(&filter, 0, &[], false)).unwrap();
        assert_eq!(outcome, RefillOutcome::FilterExhaustedHoldingLast);
        assert!(found_nothing_new(&RefillOutcome::RetryCapReached {
            unused_count: 0
        }));
        assert!(!found_nothing_new(&RefillOutcome::Offline));
        assert!(!found_nothing_new(&RefillOutcome::DailyLimitReached {
            unused_count: 0
        }));
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
            "favoritePhotos",
            "currentPhotoByMonitor",
            "cachedBytes",
            "maxCacheBytes",
            "historyCount",
            "blacklistCount",
            "isRefilling",
            "rateLimitRemaining",
            "offline",
            "showingCachedOnly",
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
    fn photo_rotation_repairs_cached_orientation_and_skips_a_mismatch() {
        let dir = std::env::temp_dir().join(format!(
            "truongpaper-photo-orientation-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("square.png"), TINY_PNG).unwrap();
        let mut state = service();
        state.app_data_dir = dir.clone();
        upsert_cached_photo(
            &state.db.lock(),
            &NewCachedPhoto {
                id: "square".into(),
                rel_path: "square.png".into(),
                album_slug: "album".into(),
                album_url: "https://example.com/album".into(),
                featured: true,
                fallback_used: false,
                width: 1,
                height: 2,
                bytes: TINY_PNG.len() as u64,
                downloaded_at: 1,
            },
        )
        .unwrap();
        let mut filter = PhotoFilter {
            required_orientation: Some("portrait".into()),
            ..Default::default()
        };
        assert!(state
            .rotation_candidate(&filter, &[], false)
            .unwrap()
            .is_none());
        filter.required_orientation = Some("landscape".into());
        let photo = state
            .rotation_candidate(&filter, &[], false)
            .unwrap()
            .unwrap();
        assert_eq!((photo.width, photo.height), (Some(1), Some(1)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn next_from_a_favorite_uses_fresh_photos_then_reuses_eligible_cache() {
        let dir = std::env::temp_dir().join(format!(
            "truongpaper-photo-favorite-rotation-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = service();
        state.app_data_dir = dir.clone();
        for (id, favorite, shown) in [
            ("current", true, true),
            ("favorite", true, true),
            ("cached", false, true),
            ("fresh", false, false),
        ] {
            let rel_path = format!("{id}.png");
            std::fs::write(dir.join(&rel_path), TINY_PNG).unwrap();
            let mut conn = state.db.lock();
            upsert_cached_photo(
                &conn,
                &NewCachedPhoto {
                    id: id.into(),
                    rel_path,
                    album_slug: "album".into(),
                    album_url: "https://example.com/album".into(),
                    featured: true,
                    fallback_used: false,
                    width: 1,
                    height: 1,
                    bytes: TINY_PNG.len() as u64,
                    downloaded_at: 1,
                },
            )
            .unwrap();
            if shown {
                mark_photo_shown(&mut conn, id).unwrap();
            }
            set_favorite(&conn, id, favorite).unwrap();
        }
        let mut filter = PhotoFilter {
            required_orientation: Some("landscape".into()),
            auto_reset_history_on_exhaustion: true,
            ..Default::default()
        };
        let mut displayed = vec!["current".into()];
        let pick = |filter: &PhotoFilter, displayed: &[String], prefer_favorite| {
            state
                .rotation_candidate(filter, displayed, prefer_favorite)
                .unwrap()
                .map(|photo| photo.id)
        };
        assert_eq!(pick(&filter, &displayed, false).as_deref(), Some("fresh"));
        assert_eq!(pick(&filter, &displayed, true).as_deref(), Some("favorite"));
        displayed.extend(["fresh".into(), "favorite".into()]);
        assert_eq!(pick(&filter, &displayed, false).as_deref(), Some("cached"));
        // Without "repeat", photos already on disk still rotate (only new downloads stop).
        filter.auto_reset_history_on_exhaustion = false;
        assert_eq!(pick(&filter, &displayed, false).as_deref(), Some("cached"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Seeds `n` shown favorite photos (`f00`..) with strictly increasing `last_shown_at`.
    fn seed_shown_favorites(state: &PhotoServiceState, dir: &Path, n: usize, favorite: bool) {
        for i in 0..n {
            let id = format!("f{i:02}");
            let rel_path = format!("{id}.png");
            std::fs::write(dir.join(&rel_path), TINY_PNG).unwrap();
            let mut conn = state.db.lock();
            upsert_cached_photo(
                &conn,
                &NewCachedPhoto {
                    id: id.clone(),
                    rel_path,
                    album_slug: "album".into(),
                    album_url: "https://example.com/album".into(),
                    featured: false,
                    fallback_used: false,
                    width: 1,
                    height: 1,
                    bytes: TINY_PNG.len() as u64,
                    downloaded_at: i as i64,
                },
            )
            .unwrap();
            mark_photo_shown(&mut conn, &id).unwrap();
            set_favorite(&conn, &id, favorite).unwrap();
            drop(conn);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    fn temp_state(tag: &str) -> (PhotoServiceState, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("truongpaper-photo-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = service();
        state.app_data_dir = dir.clone();
        (state, dir)
    }

    #[test]
    fn favorites_do_not_repeat_within_the_recent_window() {
        let (state, dir) = temp_state("recent-window");
        seed_shown_favorites(&state, &dir, 5, true);
        let filter = PhotoFilter::default();
        // Rotate 12 times through 5 favorites, showing each pick like the real flow.
        let mut shown: Vec<String> = Vec::new();
        let mut displayed: Vec<String> = Vec::new();
        for _ in 0..12 {
            let item = state
                .rotation_candidate(&filter, &displayed, true)
                .unwrap()
                .expect("a favorite is always available");
            mark_photo_shown(&mut state.db.lock(), &item.id).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
            displayed = vec![item.id.clone()];
            shown.push(item.id);
        }
        // Pool of 5 -> window of 4: no id repeats within any 5 consecutive picks.
        for w in shown.windows(5) {
            let mut ids = w.to_vec();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), 5, "repeat inside window: {shown:?}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn small_pools_still_rotate() {
        let (state, dir) = temp_state("small-pool");
        seed_shown_favorites(&state, &dir, 2, true);
        let filter = PhotoFilter::default();
        let mut displayed = vec!["f01".to_string()];
        let mut picks = Vec::new();
        for _ in 0..4 {
            let item = state
                .rotation_candidate(&filter, &displayed, true)
                .unwrap()
                .expect("two favorites keep rotating");
            mark_photo_shown(&mut state.db.lock(), &item.id).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
            displayed = vec![item.id.clone()];
            picks.push(item.id);
        }
        assert_eq!(picks, ["f00", "f01", "f00", "f01"]);
        // A single favorite that is on screen is still served rather than emptying the pool.
        let (single, dir1) = temp_state("single-pool");
        seed_shown_favorites(&single, &dir1, 1, true);
        assert!(single
            .rotation_candidate(&filter, &[], true)
            .unwrap()
            .is_some());
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(dir1).unwrap();
    }

    #[test]
    fn next_reuses_cached_photos_once_every_photo_was_shown() {
        // Every photo was shown and "repeat" is off: Next still shuffles what is on disk
        // instead of leaving the display empty (new downloads are limited per day).
        let (state, dir) = temp_state("reuse-cached");
        seed_shown_favorites(&state, &dir, 3, false);
        let filter = PhotoFilter {
            auto_reset_history_on_exhaustion: false,
            ..Default::default()
        };
        let displayed = vec!["f02".to_string()];
        let item = state
            .rotation_candidate(&filter, &displayed, false)
            .unwrap()
            .expect("a previously shown photo is reused");
        // The two most recent (f01, f02) are held back.
        assert_eq!(item.id, "f00");

        // Status reports offline + cached-only once a network error was recorded.
        assert!(!state.status().unwrap().offline);
        state.limiter.note_network_error();
        let status = state.status().unwrap();
        assert!(status.offline && status.showing_cached_only);
        state.limiter.note_success();
        assert!(!state.status().unwrap().offline);
        std::fs::remove_dir_all(dir).unwrap();
    }

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
        assert!(decode_photo_for_display(false, &bad, None)
            .unwrap()
            .is_none());
        assert!(
            decode_photo_for_display(false, &dir.join("missing.jpg"), None)
                .unwrap()
                .is_none()
        );

        // Renderer attached: decode first; a failure means "do not mark shown".
        let img = decode_photo_for_display(true, &good, None)
            .unwrap()
            .unwrap();
        assert_eq!((img.width, img.height), (1, 1));
        assert_eq!(img.pixels.len(), 4);
        assert!(decode_photo_for_display(true, &bad, None).is_err());
        assert!(decode_photo_for_display(true, &dir.join("missing.jpg"), None).is_err());
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
