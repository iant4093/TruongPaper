//! Shared SQLite data-access helpers for the photo cache (docs/02 Sections 2-4 and 6).
//!
//! Every function takes a plain `&Connection` / `&mut Connection` (or an
//! `Arc<Mutex<Connection>>` for the `_unlocked` variants) so the photo-api crate and the
//! Tauri command layer can share one implementation. All user-influenced values are bound as
//! SQL parameters; no value is ever interpolated into SQL text.

use crate::error::{CoreError, CoreResult};
use crate::models::*;
use parking_lot::Mutex;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Settings keys in the `settings` table.
pub const SETTING_FILTER_MODE: &str = "photo_filter_mode";
pub const SETTING_FEATURED_ONLY: &str = "photo_featured_only";
pub const SETTING_EXHAUSTION_AUTO_RESET: &str = "exhaustion_auto_reset";
pub const SETTING_MAX_CACHE_BYTES: &str = "max_photo_cache_bytes";

/// Unix epoch milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// "Unused" means not in the persistent shown history. `last_shown_at` is kept across history
/// resets (it only orders rotation), so it is deliberately not part of this predicate.
const UNUSED_PREDICATE: &str = "file_present = 1
    AND id NOT IN (SELECT photo_id FROM photos_history)
    AND id NOT IN (SELECT photo_id FROM photos_blacklist)";

/// Builds the ` AND ...` fragment for a [`PhotoFilter`] with bound parameters.
///
/// Album slugs are normalized with [`parse_album_slug`]. If the caller supplied slugs but none
/// is valid, the clause matches nothing (so a malformed slug can never widen a destructive
/// operation such as a history reset to "all albums").
fn filter_clause(filter: &PhotoFilter) -> (String, Vec<Value>) {
    let mut sql = String::new();
    let mut values = Vec::new();
    if filter.featured_only {
        sql.push_str(" AND featured = 1");
    }
    if let Some(orientation) = filter.orientation() {
        sql.push_str(" AND orientation = ?");
        values.push(Value::Text(orientation.as_str().to_string()));
    }
    if !filter.album_slugs.is_empty() {
        let slugs: Vec<String> = filter
            .album_slugs
            .iter()
            .filter_map(|s| parse_album_slug(s))
            .collect();
        if slugs.is_empty() {
            sql.push_str(" AND 0");
        } else {
            let marks = vec!["?"; slugs.len()].join(",");
            sql.push_str(&format!(" AND album_slug IN ({marks})"));
            values.extend(slugs.into_iter().map(Value::Text));
        }
    }
    (sql, values)
}

/// ` AND <column> NOT IN (?, ...)` for a list of ids (empty list = no clause).
fn exclusion_clause(column: &str, ids: &[String]) -> (String, Vec<Value>) {
    if ids.is_empty() {
        return (String::new(), Vec::new());
    }
    let marks = vec!["?"; ids.len()].join(",");
    (
        format!(" AND {column} NOT IN ({marks})"),
        ids.iter().cloned().map(Value::Text).collect(),
    )
}

fn to_u32(v: i64) -> u32 {
    v.clamp(0, u32::MAX as i64) as u32
}

fn to_u64(v: i64) -> u64 {
    v.max(0) as u64
}

// ---------------------------------------------------------------------------------------------
// Queue reads
// ---------------------------------------------------------------------------------------------

/// Counts unused photos on disk matching the filter.
pub fn count_unused_in_queue(conn: &Connection, filter: &PhotoFilter) -> CoreResult<usize> {
    let (clause, values) = filter_clause(filter);
    let sql = format!("SELECT COUNT(*) FROM photos_cache WHERE {UNUSED_PREDICATE}{clause}");
    let n: i64 = conn.query_row(&sql, params_from_iter(values), |r| r.get(0))?;
    Ok(n.max(0) as usize)
}

/// Loads a cached photo as an IPC item with an absolute `file_path`.
pub fn query_photo_item_by_id(
    conn: &Connection,
    app_data_dir: &Path,
    photo_id: &str,
) -> CoreResult<Option<PhotoItem>> {
    let item = conn
        .query_row(
            &format!("SELECT {PHOTO_ITEM_COLUMNS} FROM photos_cache WHERE id = ?1"),
            params![photo_id],
            |r| photo_item_from_row(r, app_data_dir),
        )
        .optional()?;
    Ok(item)
}

/// Column list read by [`photo_item_from_row`].
const PHOTO_ITEM_COLUMNS: &str = "id, album_slug, album_url, file_path, width, height, bytes,
    featured, is_favorite, downloaded_at, last_shown_at";

/// Maps a row selected with [`PHOTO_ITEM_COLUMNS`].
fn photo_item_from_row(r: &rusqlite::Row<'_>, app_data_dir: &Path) -> rusqlite::Result<PhotoItem> {
    photo_item_from_row_at(r, 0, app_data_dir)
}

/// Like [`photo_item_from_row`] with the photo columns starting at index `o`.
fn photo_item_from_row_at(
    r: &rusqlite::Row<'_>,
    o: usize,
    app_data_dir: &Path,
) -> rusqlite::Result<PhotoItem> {
    let rel_path: String = r.get(o + 3)?;
    Ok(PhotoItem {
        id: r.get(o)?,
        album_slug: r.get(o + 1)?,
        album_url: r.get(o + 2)?,
        file_path: app_data_dir.join(rel_path).to_string_lossy().into_owned(),
        width: Some(to_u32(r.get(o + 4)?)),
        height: Some(to_u32(r.get(o + 5)?)),
        bytes: to_u64(r.get(o + 6)?),
        featured: r.get::<_, i64>(o + 7)? != 0,
        is_favorite: r.get::<_, i64>(o + 8)? != 0,
        downloaded_at: r.get(o + 9)?,
        last_shown_at: r.get(o + 10)?,
    })
}

fn cached_row(r: &Row<'_>) -> rusqlite::Result<CachedPhotoRow> {
    Ok(CachedPhotoRow {
        id: r.get(0)?,
        file_path: r.get(1)?,
        file_present: r.get::<_, i64>(2)? != 0,
        album_slug: r.get(3)?,
        album_url: r.get(4)?,
        featured: r.get::<_, i64>(5)? != 0,
        fallback_used: r.get::<_, i64>(6)? != 0,
        width: to_u32(r.get(7)?),
        height: to_u32(r.get(8)?),
        orientation: r.get(9)?,
        bytes: to_u64(r.get(10)?),
        is_favorite: r.get::<_, i64>(11)? != 0,
        downloaded_at: r.get(12)?,
        last_shown_at: r.get(13)?,
        shown_count: to_u32(r.get(14)?),
    })
}

/// Loads the full `photos_cache` row for an id.
pub fn get_cached_photo(conn: &Connection, photo_id: &str) -> CoreResult<Option<CachedPhotoRow>> {
    let row = conn
        .query_row(
            "SELECT id, file_path, file_present, album_slug, album_url, featured, fallback_used,
                    width, height, orientation, bytes, is_favorite, downloaded_at,
                    last_shown_at, shown_count
             FROM photos_cache WHERE id = ?1",
            params![photo_id],
            cached_row,
        )
        .optional()?;
    Ok(row)
}

#[derive(Clone, Copy)]
pub enum RotationPhotoPool {
    Unused,
    Favorites,
    PreviouslyShown,
}

fn pool_predicate_and_order(pool: RotationPhotoPool) -> (&'static str, &'static str) {
    match pool {
        RotationPhotoPool::Unused => (UNUSED_PREDICATE, "last_shown_at ASC, downloaded_at ASC, id"),
        RotationPhotoPool::Favorites => (
            "file_present = 1 AND is_favorite = 1 AND id NOT IN (SELECT photo_id FROM photos_blacklist)",
            "last_shown_at ASC, downloaded_at ASC, id",
        ),
        RotationPhotoPool::PreviouslyShown => (
            "file_present = 1 AND last_shown_at IS NOT NULL AND id NOT IN (SELECT photo_id FROM photos_blacklist)",
            "last_shown_at ASC, downloaded_at ASC, id",
        ),
    }
}

/// Largest number of recently shown photos kept out of the Favorites / PreviouslyShown pools.
pub const RECENT_SHOWN_WINDOW: usize = 20;

/// Ids to keep out of a repeat-prone pool: the last `min(20, pool_size - 1)` shown photos of
/// that pool, so at least one candidate always remains. The `Unused` pool needs no window.
pub fn recent_shown_exclusions(
    conn: &Connection,
    filter: &PhotoFilter,
    pool: RotationPhotoPool,
) -> CoreResult<Vec<String>> {
    if matches!(pool, RotationPhotoPool::Unused) {
        return Ok(Vec::new());
    }
    let (predicate, _) = pool_predicate_and_order(pool);
    let (clause, values) = filter_clause(filter);
    let size: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM photos_cache WHERE {predicate}{clause}"),
        params_from_iter(values.iter()),
        |r| r.get(0),
    )?;
    let window = (size.max(0) as usize)
        .saturating_sub(1)
        .min(RECENT_SHOWN_WINDOW);
    if window == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT id FROM photos_cache WHERE {predicate}{clause}
         AND last_shown_at IS NOT NULL ORDER BY last_shown_at DESC, id LIMIT {window}"
    ))?;
    let ids = stmt
        .query_map(params_from_iter(values.iter()), |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Picks the oldest unused or least recently shown eligible photo. Missing files are skipped
/// and their cache rows are marked unavailable.
fn pop_candidate(
    conn: &Connection,
    app_data_dir: &Path,
    filter: &PhotoFilter,
    exclude_ids: &[String],
    pool: RotationPhotoPool,
) -> CoreResult<Option<(String, PathBuf)>> {
    let (clause, mut values) = filter_clause(filter);
    let (excl, excl_values) = exclusion_clause("id", exclude_ids);
    values.extend(excl_values);
    let (predicate, order) = pool_predicate_and_order(pool);
    let sql = format!(
        "SELECT id, file_path FROM photos_cache WHERE {predicate}{clause}{excl} ORDER BY {order} LIMIT 1"
    );
    loop {
        let row: Option<(String, String)> = conn
            .query_row(&sql, params_from_iter(values.iter()), |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let Some((id, rel_path)) = row else {
            return Ok(None);
        };
        let abs_path = app_data_dir.join(&rel_path);
        if abs_path.exists() {
            return Ok(Some((id, abs_path)));
        }
        conn.execute(
            "UPDATE photos_cache SET file_present = 0 WHERE id = ?1",
            params![id],
        )?;
    }
}

/// Step 1: pick the next unused photo from the local disk queue (zero API calls).
/// Returns `None` if the local unused queue for this filter is empty.
pub fn pop_next_queued_photo(
    conn: &Connection,
    app_data_dir: &Path,
    filter: &PhotoFilter,
) -> CoreResult<Option<(String, PathBuf)>> {
    pop_candidate(conn, app_data_dir, filter, &[], RotationPhotoPool::Unused)
}

/// Like [`pop_next_queued_photo`] but skips ids already on screen and returns an IPC item.
pub fn pop_next_queued_photo_with_exclusions(
    conn: &Connection,
    app_data_dir: &Path,
    filter: &PhotoFilter,
    currently_displayed_ids: &[String],
) -> CoreResult<Option<PhotoItem>> {
    pick_cached_rotation_photo(
        conn,
        app_data_dir,
        filter,
        currently_displayed_ids,
        RotationPhotoPool::Unused,
    )
}

/// Reads a rotation candidate from disk without updating its shown history.
pub fn pick_cached_rotation_photo(
    conn: &Connection,
    app_data_dir: &Path,
    filter: &PhotoFilter,
    exclude_ids: &[String],
    pool: RotationPhotoPool,
) -> CoreResult<Option<PhotoItem>> {
    match pop_candidate(conn, app_data_dir, filter, exclude_ids, pool)? {
        Some((id, _)) => query_photo_item_by_id(conn, app_data_dir, &id),
        None => Ok(None),
    }
}

/// All unused photos matching the filter, oldest download first (for the status panel).
pub fn list_unused_queue(
    conn: &Connection,
    app_data_dir: &Path,
    filter: &PhotoFilter,
) -> CoreResult<Vec<PhotoItem>> {
    let (clause, values) = filter_clause(filter);
    let sql = format!(
        "SELECT {PHOTO_ITEM_COLUMNS} FROM photos_cache
         WHERE {UNUSED_PREDICATE}{clause} ORDER BY downloaded_at ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt
        .query_map(params_from_iter(values), |r| {
            photo_item_from_row(r, app_data_dir)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(items)
}

/// Favorites belong to the local collection regardless of the active rotation filter.
pub fn list_favorite_photos(conn: &Connection, app_data_dir: &Path) -> CoreResult<Vec<PhotoItem>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PHOTO_ITEM_COLUMNS} FROM photos_cache WHERE file_present = 1 AND is_favorite = 1
         ORDER BY last_shown_at DESC, downloaded_at DESC, id"
    ))?;
    let photos = stmt
        .query_map([], |r| photo_item_from_row(r, app_data_dir))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(photos)
}

/// A derived thumbnail stays beside its cached original and follows the same eviction rules.
pub fn photo_preview_path(path: &Path) -> PathBuf {
    path.with_extension("preview-v1.jpg")
}

pub fn remove_cached_photo_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(photo_preview_path(path));
}

// ---------------------------------------------------------------------------------------------
// Queue writes
// ---------------------------------------------------------------------------------------------

/// Inserts a freshly downloaded photo into the unused queue (`last_shown_at = NULL`) and
/// refreshes all metadata when the id already exists.
pub fn upsert_cached_photo(conn: &Connection, photo: &NewCachedPhoto) -> CoreResult<()> {
    conn.execute(
        "INSERT INTO photos_cache (
            id, file_path, file_present, album_slug, album_url,
            featured, fallback_used, width, height, orientation,
            bytes, is_favorite, downloaded_at, last_shown_at, shown_count
        ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, NULL, 0)
        ON CONFLICT(id) DO UPDATE SET
            file_path = excluded.file_path,
            file_present = 1,
            album_slug = excluded.album_slug,
            album_url = excluded.album_url,
            featured = excluded.featured,
            fallback_used = excluded.fallback_used,
            width = excluded.width,
            height = excluded.height,
            orientation = excluded.orientation,
            bytes = excluded.bytes,
            downloaded_at = excluded.downloaded_at,
            last_shown_at = NULL",
        params![
            photo.id,
            photo.rel_path,
            photo.album_slug,
            photo.album_url,
            i32::from(photo.featured),
            i32::from(photo.fallback_used),
            photo.width,
            photo.height,
            photo.orientation().as_str(),
            photo.bytes as i64,
            photo.downloaded_at,
        ],
    )?;
    Ok(())
}

/// Step 2: call ONLY AFTER the wallpaper change / GPU transition succeeded on the desktop.
/// Marks the cache row shown and upserts the persistent history row in one transaction.
pub fn mark_photo_shown(conn: &mut Connection, photo_id: &str) -> CoreResult<()> {
    let tx = conn.transaction()?;
    mark_photo_shown_in_transaction(&tx, photo_id)?;
    tx.commit()?;
    Ok(())
}

pub fn mark_photo_shown_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    photo_id: &str,
) -> CoreResult<()> {
    let ts = now_ms();
    let meta: Option<(String, i64, String)> = tx
        .query_row(
            "SELECT album_slug, featured, orientation FROM photos_cache WHERE id = ?1",
            params![photo_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((album_slug, featured, orientation)) = meta {
        tx.execute(
            "UPDATE photos_cache
             SET last_shown_at = ?1, shown_count = shown_count + 1
             WHERE id = ?2",
            params![ts, photo_id],
        )?;
        tx.execute(
            "INSERT INTO photos_history
                (photo_id, album_slug, featured, orientation, first_shown_at, last_shown_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(photo_id) DO UPDATE SET last_shown_at = excluded.last_shown_at",
            params![photo_id, album_slug, featured, orientation, ts],
        )?;
    }
    Ok(())
}

/// Checks API metadata against local state BEFORE the image is downloaded.
///
/// Order: blacklist, orientation (only when both dimensions are known), unused queue (also
/// counts photos currently on screen, docs/02 Section 2), persistent history.
pub fn classify_candidate(
    conn: &Connection,
    photo_id: &str,
    width: Option<u32>,
    height: Option<u32>,
    required_orientation: Option<&str>,
    currently_displayed_ids: &[String],
) -> CoreResult<CandidateStatus> {
    let exists = |sql: &str| -> CoreResult<bool> {
        Ok(conn
            .query_row(sql, params![photo_id], |_| Ok(true))
            .optional()?
            .unwrap_or(false))
    };

    if exists("SELECT 1 FROM photos_blacklist WHERE photo_id = ?1")? {
        return Ok(CandidateStatus::Blacklisted);
    }
    if let (Some(required), Some(w), Some(h)) = (required_orientation, width, height) {
        if Orientation::from_dimensions(w, h).as_str() != required {
            return Ok(CandidateStatus::OrientationMismatch);
        }
    }
    // A photo on screen is treated like a queued one so its history row cannot trigger an
    // exhaustion reset.
    if exists(
        "SELECT 1 FROM photos_cache WHERE id = ?1 AND file_present = 1
         AND id NOT IN (SELECT photo_id FROM photos_history)",
    )? || currently_displayed_ids.iter().any(|d| d == photo_id)
    {
        return Ok(CandidateStatus::AlreadyInUnusedQueue);
    }
    if exists("SELECT 1 FROM photos_history WHERE photo_id = ?1")? {
        return Ok(CandidateStatus::AlreadyInHistory);
    }
    Ok(CandidateStatus::UniqueAndUnseen)
}

/// Step 4: resets shown history for an exhausted album/filter (preserving on-screen ids) and
/// immediately re-queues still-cached files with zero API calls. Returns the number of cache
/// rows returned to the unused queue.
pub fn reset_exhausted_filter_history(
    conn: &mut Connection,
    filter: &PhotoFilter,
    currently_displayed_ids: &[String],
) -> CoreResult<usize> {
    let tx = conn.transaction()?;
    let (clause, filter_values) = filter_clause(filter);

    let (hist_excl, hist_excl_values) = exclusion_clause("photo_id", currently_displayed_ids);
    let mut hist_values = filter_values.clone();
    hist_values.extend(hist_excl_values);
    tx.execute(
        &format!("DELETE FROM photos_history WHERE 1=1{clause}{hist_excl}"),
        params_from_iter(hist_values),
    )?;

    // `last_shown_at` is kept: it orders rotation, so a photo shown moments ago does not come
    // straight back. Cached files return to the unused queue because their history is gone.
    let (unused_sql, unused_values) = filter_clause(filter);
    let requeued: i64 = tx.query_row(
        &format!("SELECT COUNT(*) FROM photos_cache WHERE {UNUSED_PREDICATE}{unused_sql}"),
        params_from_iter(unused_values),
        |r| r.get(0),
    )?;
    tx.commit()?;
    let requeued = requeued.max(0) as usize;
    Ok(requeued)
}

// ---------------------------------------------------------------------------------------------
// Cache eviction
// ---------------------------------------------------------------------------------------------

/// Bytes currently on disk for non-favorite cached photos.
pub fn evictable_cache_bytes(conn: &Connection) -> CoreResult<u64> {
    let n: i64 = conn.query_row(
        "SELECT COALESCE(SUM(bytes), 0) FROM photos_cache WHERE file_present = 1 AND is_favorite = 0",
        [],
        |r| r.get(0),
    )?;
    Ok(to_u64(n))
}

/// Total bytes of all photos currently on disk (favorites included).
pub fn cached_bytes_total(conn: &Connection) -> CoreResult<u64> {
    let n: i64 = conn.query_row(
        "SELECT COALESCE(SUM(bytes), 0) FROM photos_cache WHERE file_present = 1",
        [],
        |r| r.get(0),
    )?;
    Ok(to_u64(n))
}

/// Chooses which files must go to fit `incoming_bytes` under `max_cache_bytes`: oldest
/// already-shown, non-favorite, non-displayed first. Unshown (queued) photos are never chosen.
pub fn plan_cache_eviction(
    conn: &Connection,
    app_data_dir: &Path,
    incoming_bytes: u64,
    max_cache_bytes: u64,
    currently_displayed_ids: &[String],
) -> CoreResult<Vec<(String, PathBuf)>> {
    let current_bytes = evictable_cache_bytes(conn)?;
    if current_bytes.saturating_add(incoming_bytes) <= max_cache_bytes {
        return Ok(Vec::new());
    }
    let mut bytes_to_free = current_bytes
        .saturating_add(incoming_bytes)
        .saturating_sub(max_cache_bytes);

    let mut stmt = conn.prepare(
        "SELECT id, file_path, bytes FROM photos_cache
         WHERE file_present = 1
           AND is_favorite = 0
           AND last_shown_at IS NOT NULL
           AND id IN (SELECT photo_id FROM photos_history)
         ORDER BY last_shown_at ASC",
    )?;
    let candidates = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                to_u64(r.get(2)?),
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut selected = Vec::new();
    for (id, rel_path, file_bytes) in candidates {
        if bytes_to_free == 0 {
            break;
        }
        if currently_displayed_ids.contains(&id) {
            continue;
        }
        selected.push((id, app_data_dir.join(rel_path)));
        bytes_to_free = bytes_to_free.saturating_sub(file_bytes);
    }
    Ok(selected)
}

/// Marks a cached photo's file as gone. `photos_history` is intentionally untouched so the id
/// is never shown again.
pub fn mark_file_evicted(conn: &Connection, photo_id: &str) -> CoreResult<()> {
    conn.execute(
        "UPDATE photos_cache SET file_present = 0 WHERE id = ?1",
        params![photo_id],
    )?;
    Ok(())
}

/// Step 3: history-preserving disk cache eviction (holds the connection for the whole call).
/// Returns the ids whose files were removed.
pub fn evict_old_cached_files_preserving_history(
    conn: &Connection,
    app_data_dir: &Path,
    incoming_bytes: u64,
    max_cache_bytes: u64,
    currently_displayed_ids: &[String],
) -> CoreResult<Vec<String>> {
    let plan = plan_cache_eviction(
        conn,
        app_data_dir,
        incoming_bytes,
        max_cache_bytes,
        currently_displayed_ids,
    )?;
    let mut evicted = Vec::with_capacity(plan.len());
    for (id, path) in plan {
        remove_cached_photo_files(&path);
        mark_file_evicted(conn, &id)?;
        evicted.push(id);
    }
    Ok(evicted)
}

/// Same as [`evict_old_cached_files_preserving_history`] but removes files OUTSIDE the SQLite
/// mutex so a slow disk never blocks other database users.
pub fn evict_old_cached_files_preserving_history_unlocked(
    db: &Arc<Mutex<Connection>>,
    app_data_dir: &Path,
    incoming_bytes: u64,
    max_cache_bytes: u64,
    currently_displayed_ids: &[String],
) -> CoreResult<Vec<String>> {
    let plan = {
        let conn = db.lock();
        plan_cache_eviction(
            &conn,
            app_data_dir,
            incoming_bytes,
            max_cache_bytes,
            currently_displayed_ids,
        )?
    };
    let mut evicted = Vec::with_capacity(plan.len());
    for (id, path) in plan {
        // The plan is a snapshot; a photo may have been favorited or put on screen since.
        // Re-check against fresh state under the lock and flip `file_present` in the same
        // critical section, so nothing can pick the file once it is condemned.
        let claimed = {
            let conn = db.lock();
            let mut displayed = list_displayed_photo_ids(&conn)?;
            displayed.extend(currently_displayed_ids.iter().cloned());
            if displayed.contains(&id) {
                false
            } else {
                conn.execute(
                    "UPDATE photos_cache SET file_present = 0
                     WHERE id = ?1 AND file_present = 1 AND is_favorite = 0",
                    params![id],
                )? > 0
            }
        };
        if claimed {
            remove_cached_photo_files(&path);
            evicted.push(id);
        }
    }
    Ok(evicted)
}

/// Every non-favorite, non-displayed file still on disk (for `photo_clear_cache`).
pub fn plan_clear_cache(
    conn: &Connection,
    app_data_dir: &Path,
    currently_displayed_ids: &[String],
) -> CoreResult<Vec<(String, PathBuf)>> {
    let mut stmt = conn.prepare(
        "SELECT id, file_path FROM photos_cache WHERE file_present = 1 AND is_favorite = 0",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|(id, _)| !currently_displayed_ids.contains(id))
        .map(|(id, rel)| (id, app_data_dir.join(rel)))
        .collect())
}

// ---------------------------------------------------------------------------------------------
// Favorites, blacklist, history, counters
// ---------------------------------------------------------------------------------------------

/// Pins / unpins a cached photo (favorites are never evicted).
pub fn set_favorite(conn: &Connection, photo_id: &str, favorite: bool) -> CoreResult<()> {
    conn.execute(
        "UPDATE photos_cache SET is_favorite = ?1 WHERE id = ?2",
        params![i32::from(favorite), photo_id],
    )?;
    Ok(())
}

/// "Hide / never show again": records the blacklist row and marks the cached file gone.
/// Returns the absolute file path the caller should delete, if the photo was cached.
pub fn blacklist_photo(
    conn: &Connection,
    app_data_dir: &Path,
    photo_id: &str,
) -> CoreResult<Option<PathBuf>> {
    let meta: Option<(String, String)> = conn
        .query_row(
            "SELECT album_slug, file_path FROM photos_cache WHERE id = ?1",
            params![photo_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (album_slug, abs_path) = match meta {
        Some((slug, rel)) => (Some(slug), Some(app_data_dir.join(rel))),
        None => (None, None),
    };
    conn.execute(
        "INSERT INTO photos_blacklist (photo_id, album_slug, hidden_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(photo_id) DO NOTHING",
        params![photo_id, album_slug, now_ms()],
    )?;
    conn.execute(
        "UPDATE photos_cache SET file_present = 0, is_favorite = 0 WHERE id = ?1",
        params![photo_id],
    )?;
    Ok(abs_path)
}

/// Monitors currently showing a photo.
pub fn monitors_showing_photo(conn: &Connection, photo_id: &str) -> CoreResult<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT monitor_id FROM monitor_assignments WHERE current_photo_id = ?1")?;
    let ids = stmt
        .query_map(params![photo_id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Photo ids currently assigned to any monitor.
pub fn list_displayed_photo_ids(conn: &Connection) -> CoreResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT current_photo_id FROM monitor_assignments WHERE current_photo_id IS NOT NULL",
    )?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// `monitor_id -> current photo` for the status panel.
pub fn current_photo_by_monitor(
    conn: &Connection,
    app_data_dir: &Path,
) -> CoreResult<HashMap<String, Option<PhotoItem>>> {
    // One LEFT JOIN instead of a lookup per monitor.
    let mut stmt = conn.prepare(
        "SELECT m.monitor_id, p.id, p.album_slug, p.album_url, p.file_path, p.width, p.height,
                p.bytes, p.featured, p.is_favorite, p.downloaded_at, p.last_shown_at
         FROM monitor_assignments m LEFT JOIN photos_cache p ON p.id = m.current_photo_id",
    )?;
    let rows = stmt.query_map([], |r| {
        let monitor_id: String = r.get(0)?;
        let item = if r.get::<_, Option<String>>(1)?.is_some() {
            Some(photo_item_from_row_at(r, 1, app_data_dir)?)
        } else {
            None
        };
        Ok((monitor_id, item))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        let (monitor_id, item) = row?;
        map.insert(monitor_id, item);
    }
    Ok(map)
}

/// Points a monitor at a photo (creates the assignment row for `photo_api` sources if missing).
pub fn assign_photo_to_monitor(
    conn: &Connection,
    monitor_id: &str,
    photo_id: &str,
) -> CoreResult<()> {
    conn.execute(
        "INSERT INTO monitor_assignments (monitor_id, source_json, current_photo_id, updated_at)
         VALUES (?1, '{\"kind\":\"photo_api\"}', ?2, ?3)
         ON CONFLICT(monitor_id) DO UPDATE SET
            current_photo_id = excluded.current_photo_id,
            updated_at = excluded.updated_at",
        params![monitor_id, photo_id, now_ms()],
    )?;
    Ok(())
}

/// Number of rows in `photos_history`.
pub fn history_count(conn: &Connection) -> CoreResult<usize> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM photos_history", [], |r| r.get(0))?;
    Ok(n.max(0) as usize)
}

/// Number of rows in `photos_blacklist`.
pub fn blacklist_count(conn: &Connection) -> CoreResult<usize> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM photos_blacklist", [], |r| r.get(0))?;
    Ok(n.max(0) as usize)
}

/// Reads one history row.
pub fn get_history_entry(
    conn: &Connection,
    photo_id: &str,
) -> CoreResult<Option<PhotoHistoryEntry>> {
    let row = conn
        .query_row(
            "SELECT photo_id, album_slug, featured, orientation, first_shown_at, last_shown_at
             FROM photos_history WHERE photo_id = ?1",
            params![photo_id],
            |r| {
                Ok(PhotoHistoryEntry {
                    photo_id: r.get(0)?,
                    album_slug: r.get(1)?,
                    featured: r.get::<_, i64>(2)? != 0,
                    orientation: r.get(3)?,
                    first_shown_at: r.get(4)?,
                    last_shown_at: r.get(5)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Lists the blacklist, newest first.
pub fn list_blacklist(conn: &Connection) -> CoreResult<Vec<BlacklistEntry>> {
    let mut stmt = conn.prepare(
        "SELECT photo_id, album_slug, hidden_at FROM photos_blacklist ORDER BY hidden_at DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(BlacklistEntry {
                photo_id: r.get(0)?,
                album_slug: r.get(1)?,
                hidden_at: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------------------------
// Saved albums
// ---------------------------------------------------------------------------------------------

/// Inserts or refreshes a saved album and (re-)enables it.
pub fn upsert_saved_album(conn: &Connection, album: &SavedAlbumUpsert) -> CoreResult<()> {
    if parse_album_slug(&album.album_slug).as_deref() != Some(album.album_slug.as_str()) {
        return Err(CoreError::InvalidInput(format!(
            "invalid album slug: {}",
            album.album_slug
        )));
    }
    conn.execute(
        "INSERT INTO saved_albums (
            album_slug, title, description, category, album_url,
            cover_thumbnail_url, cover_blurhash, image_count, is_enabled, added_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9)
        ON CONFLICT(album_slug) DO UPDATE SET
            title = excluded.title,
            cover_thumbnail_url = COALESCE(excluded.cover_thumbnail_url, saved_albums.cover_thumbnail_url),
            image_count = COALESCE(excluded.image_count, saved_albums.image_count),
            is_enabled = 1",
        params![
            album.album_slug,
            album.title,
            album.description,
            album.category,
            album.album_url,
            album.cover_thumbnail_url,
            album.cover_blurhash,
            album.image_count,
            album.added_at,
        ],
    )?;
    Ok(())
}

/// Enables / disables an album in the rotation pool.
pub fn set_album_enabled(conn: &Connection, album_slug: &str, enabled: bool) -> CoreResult<()> {
    conn.execute(
        "UPDATE saved_albums SET is_enabled = ?1 WHERE album_slug = ?2",
        params![i32::from(enabled), album_slug],
    )?;
    Ok(())
}

/// Removes a saved album (cached photos and history are kept).
pub fn remove_saved_album(conn: &Connection, album_slug: &str) -> CoreResult<()> {
    conn.execute(
        "DELETE FROM saved_albums WHERE album_slug = ?1",
        params![album_slug],
    )?;
    Ok(())
}

/// Slugs of albums included in the active multi-album rotation pool.
pub fn list_enabled_album_slugs(conn: &Connection) -> CoreResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT album_slug FROM saved_albums WHERE is_enabled = 1 ORDER BY added_at, album_slug",
    )?;
    let slugs = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(slugs)
}

/// All saved albums, oldest first.
pub fn list_saved_albums(conn: &Connection) -> CoreResult<Vec<SavedAlbumRow>> {
    let mut stmt = conn.prepare(
        "SELECT album_slug, title, description, category, album_url, cover_thumbnail_url,
                cover_blurhash, image_count, is_enabled, added_at
         FROM saved_albums ORDER BY added_at, album_slug",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SavedAlbumRow {
                album_slug: r.get(0)?,
                title: r.get(1)?,
                description: r.get(2)?,
                category: r.get(3)?,
                album_url: r.get(4)?,
                cover_thumbnail_url: r.get(5)?,
                cover_blurhash: r.get(6)?,
                image_count: r.get::<_, Option<i64>>(7)?.map(to_u32),
                is_enabled: r.get::<_, i64>(8)? != 0,
                added_at: r.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------------------------

/// Reads a JSON setting; `None` if missing or not decodable as `T`.
pub fn get_setting<T: DeserializeOwned>(conn: &Connection, key: &str) -> CoreResult<Option<T>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?;
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
}

/// Writes (upserts) a JSON setting.
pub fn set_setting<T: Serialize>(conn: &Connection, key: &str, value: &T) -> CoreResult<()> {
    let json = serde_json::to_string(value)?;
    conn.execute(
        "INSERT INTO settings (key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![key, json],
    )?;
    Ok(())
}

/// Reads the photo-related settings, applying documented defaults.
pub fn load_photo_settings(conn: &Connection) -> CoreResult<PhotoSettings> {
    let defaults = PhotoSettings::default();
    Ok(PhotoSettings {
        filter_mode: get_setting(conn, SETTING_FILTER_MODE)?.unwrap_or(defaults.filter_mode),
        featured_only: get_setting(conn, SETTING_FEATURED_ONLY)?.unwrap_or(defaults.featured_only),
        exhaustion_auto_reset: get_setting(conn, SETTING_EXHAUSTION_AUTO_RESET)?
            .unwrap_or(defaults.exhaustion_auto_reset),
        max_cache_bytes: get_setting(conn, SETTING_MAX_CACHE_BYTES)?
            .unwrap_or(defaults.max_cache_bytes),
    })
}

/// Persists the filter mode and featured-only flag.
pub fn save_photo_filter(
    conn: &Connection,
    mode: FilterMode,
    featured_only: bool,
) -> CoreResult<()> {
    set_setting(conn, SETTING_FILTER_MODE, &mode)?;
    set_setting(conn, SETTING_FEATURED_ONLY, &featured_only)
}

/// Persists the disk cache budget.
pub fn save_max_cache_bytes(conn: &Connection, max_bytes: u64) -> CoreResult<()> {
    set_setting(conn, SETTING_MAX_CACHE_BYTES, &max_bytes)
}

/// Builds the active [`PhotoFilter`] from settings + enabled albums, plus the ids on screen.
pub fn load_active_filter(
    conn: &Connection,
    required_orientation: Option<String>,
) -> CoreResult<(PhotoSettings, PhotoFilter, Vec<String>)> {
    let settings = load_photo_settings(conn)?;
    let album_slugs = if settings.filter_mode == FilterMode::Albums {
        list_enabled_album_slugs(conn)?
    } else {
        Vec::new()
    };
    let filter = PhotoFilter {
        album_slugs,
        featured_only: match settings.filter_mode {
            FilterMode::All => false,
            FilterMode::Featured => true,
            FilterMode::Albums => settings.featured_only,
        },
        required_orientation,
        auto_reset_history_on_exhaustion: settings.exhaustion_auto_reset,
    };
    let displayed = list_displayed_photo_ids(conn)?;
    Ok((settings, filter, displayed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SQLITE_SCHEMA_SQL;
    use std::sync::atomic::{AtomicU32, Ordering};

    pub(crate) struct TempDir(pub PathBuf);
    impl TempDir {
        pub fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "truongpaper-core-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(dir.join("wallpapers/photos")).unwrap();
            TempDir(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SQLITE_SCHEMA_SQL).unwrap();
        conn
    }

    fn photo(id: &str, slug: &str, w: u32, h: u32, bytes: u64, at: i64) -> NewCachedPhoto {
        NewCachedPhoto {
            id: id.into(),
            rel_path: format!("wallpapers/photos/{id}.jpg"),
            album_slug: slug.into(),
            album_url: format!("https://iantruongphotography.com/album/{slug}"),
            featured: false,
            fallback_used: false,
            width: w,
            height: h,
            bytes,
            downloaded_at: at,
        }
    }

    /// Inserts a photo row and writes a matching file on disk.
    fn add(conn: &Connection, dir: &TempDir, p: &NewCachedPhoto) {
        upsert_cached_photo(conn, p).unwrap();
        std::fs::write(dir.0.join(&p.rel_path), vec![0u8; p.bytes as usize]).unwrap();
    }

    fn filter() -> PhotoFilter {
        PhotoFilter {
            auto_reset_history_on_exhaustion: true,
            ..Default::default()
        }
    }

    #[test]
    fn pop_is_fifo_and_skips_history_blacklist_and_exclusions() {
        let conn = db();
        let dir = TempDir::new();
        for (i, id) in ["a", "b", "c", "d"].iter().enumerate() {
            add(&conn, &dir, &photo(id, "album-x", 4000, 3000, 10, i as i64));
        }
        conn.execute("INSERT INTO photos_blacklist VALUES ('a', NULL, 1)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO photos_history VALUES ('b','album-x',0,'landscape',1,1)",
            [],
        )
        .unwrap();
        assert_eq!(count_unused_in_queue(&conn, &filter()).unwrap(), 2);

        let (id, path) = pop_next_queued_photo(&conn, &dir.0, &filter())
            .unwrap()
            .unwrap();
        assert_eq!(id, "c");
        assert!(path.ends_with("wallpapers/photos/c.jpg"));

        let item = pop_next_queued_photo_with_exclusions(&conn, &dir.0, &filter(), &["c".into()])
            .unwrap()
            .unwrap();
        assert_eq!(item.id, "d");
        assert!(item.file_path.starts_with(dir.0.to_str().unwrap()));
    }

    #[test]
    fn pop_heals_rows_whose_file_vanished() {
        let conn = db();
        let dir = TempDir::new();
        add(&conn, &dir, &photo("gone", "s", 10, 10, 5, 1));
        add(&conn, &dir, &photo("here", "s", 10, 10, 5, 2));
        std::fs::remove_file(dir.0.join("wallpapers/photos/gone.jpg")).unwrap();
        let (id, _) = pop_next_queued_photo(&conn, &dir.0, &filter())
            .unwrap()
            .unwrap();
        assert_eq!(id, "here");
        assert!(
            !get_cached_photo(&conn, "gone")
                .unwrap()
                .unwrap()
                .file_present
        );
    }

    #[test]
    fn filter_by_album_orientation_and_featured_uses_bound_params() {
        let conn = db();
        let dir = TempDir::new();
        add(&conn, &dir, &photo("land", "alpha", 4000, 3000, 1, 1));
        add(&conn, &dir, &photo("port", "alpha", 3000, 4000, 1, 2));
        add(&conn, &dir, &photo("beta1", "beta", 3000, 4000, 1, 3));
        conn.execute("UPDATE photos_cache SET featured = 1 WHERE id = 'port'", [])
            .unwrap();

        let mut f = filter();
        f.album_slugs = vec!["https://iantruongphotography.com/album/alpha".into()];
        f.required_orientation = Some("portrait".into());
        assert_eq!(count_unused_in_queue(&conn, &f).unwrap(), 1);
        f.featured_only = true;
        assert_eq!(count_unused_in_queue(&conn, &f).unwrap(), 1);
        f.album_slugs = vec!["beta".into()];
        assert_eq!(count_unused_in_queue(&conn, &f).unwrap(), 0);

        // Invalid slugs match nothing instead of widening to "all albums".
        f.featured_only = false;
        f.required_orientation = None;
        f.album_slugs = vec!["x'); DROP TABLE photos_cache;--".into()];
        assert_eq!(count_unused_in_queue(&conn, &f).unwrap(), 0);
        assert_eq!(
            list_unused_queue(&conn, &dir.0, &filter()).unwrap().len(),
            3
        );
    }

    #[test]
    fn mark_shown_writes_history_and_removes_from_queue() {
        let mut conn = db();
        let dir = TempDir::new();
        add(&conn, &dir, &photo("p1", "s", 4000, 3000, 1, 1));
        mark_photo_shown(&mut conn, "p1").unwrap();
        let row = get_cached_photo(&conn, "p1").unwrap().unwrap();
        assert!(row.last_shown_at.is_some());
        assert_eq!(row.shown_count, 1);
        let first = get_history_entry(&conn, "p1").unwrap().unwrap();
        assert_eq!(first.orientation, "landscape");
        assert_eq!(count_unused_in_queue(&conn, &filter()).unwrap(), 0);
        mark_photo_shown(&mut conn, "p1").unwrap();
        assert_eq!(
            get_cached_photo(&conn, "p1").unwrap().unwrap().shown_count,
            2
        );
        assert_eq!(history_count(&conn).unwrap(), 1);
        mark_photo_shown(&mut conn, "missing").unwrap();
        assert_eq!(history_count(&conn).unwrap(), 1);
    }

    #[test]
    fn classify_distinguishes_queue_history_blacklist_orientation() {
        let mut conn = db();
        let dir = TempDir::new();
        add(&conn, &dir, &photo("queued", "s", 4000, 3000, 1, 1));
        add(&conn, &dir, &photo("seen", "s", 4000, 3000, 1, 2));
        mark_photo_shown(&mut conn, "seen").unwrap();
        conn.execute("INSERT INTO photos_blacklist VALUES ('bad', 's', 1)", [])
            .unwrap();
        // Evicted from disk but still in history.
        conn.execute(
            "INSERT INTO photos_history VALUES ('evicted','s',0,'landscape',1,1)",
            [],
        )
        .unwrap();

        let c = |id: &str, w, h, o: Option<&str>, d: &[String]| {
            classify_candidate(&conn, id, w, h, o, d).unwrap()
        };
        assert_eq!(
            c("queued", None, None, None, &[]),
            CandidateStatus::AlreadyInUnusedQueue
        );
        assert_eq!(
            c("seen", None, None, None, &[]),
            CandidateStatus::AlreadyInHistory
        );
        assert_eq!(
            c("evicted", None, None, None, &[]),
            CandidateStatus::AlreadyInHistory
        );
        assert_eq!(
            c("bad", None, None, None, &[]),
            CandidateStatus::Blacklisted
        );
        assert_eq!(
            c("new", Some(3000), Some(4000), Some("landscape"), &[]),
            CandidateStatus::OrientationMismatch
        );
        assert_eq!(
            c("new", Some(4000), Some(3000), Some("landscape"), &[]),
            CandidateStatus::UniqueAndUnseen
        );
        // Orientation is only checked when dimensions are known.
        assert_eq!(
            c("new", None, None, Some("portrait"), &[]),
            CandidateStatus::UniqueAndUnseen
        );
        // An on-screen photo counts as "already queued", even though it is in history.
        assert_eq!(
            c("seen", None, None, None, &["seen".into()]),
            CandidateStatus::AlreadyInUnusedQueue
        );
    }

    #[test]
    fn eviction_deletes_oldest_shown_files_and_preserves_history() {
        let mut conn = db();
        let dir = TempDir::new();
        for (i, id) in ["old", "mid", "fav", "shown_now", "queued"]
            .iter()
            .enumerate()
        {
            add(&conn, &dir, &photo(id, "s", 4000, 3000, 100, i as i64));
        }
        for id in ["old", "mid", "fav", "shown_now"] {
            mark_photo_shown(&mut conn, id).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        set_favorite(&conn, "fav", true).unwrap();

        // Evictable bytes = 400 (queued + old + mid + shown_now); budget 250, incoming 100 => free 250.
        let evicted = evict_old_cached_files_preserving_history(
            &conn,
            &dir.0,
            100,
            250,
            &["shown_now".to_string()],
        )
        .unwrap();
        assert_eq!(evicted, vec!["old".to_string(), "mid".to_string()]);
        for id in &evicted {
            assert!(!dir.0.join(format!("wallpapers/photos/{id}.jpg")).exists());
            assert!(!get_cached_photo(&conn, id).unwrap().unwrap().file_present);
            assert!(
                get_history_entry(&conn, id).unwrap().is_some(),
                "history kept"
            );
        }
        for id in ["fav", "shown_now", "queued"] {
            assert!(dir.0.join(format!("wallpapers/photos/{id}.jpg")).exists());
            assert!(get_cached_photo(&conn, id).unwrap().unwrap().file_present);
        }
        assert_eq!(history_count(&conn).unwrap(), 4);

        // Under budget => nothing happens.
        let none =
            evict_old_cached_files_preserving_history(&conn, &dir.0, 0, 10_000, &[]).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn unlocked_eviction_matches_locked_behavior() {
        let mut conn = db();
        let dir = TempDir::new();
        add(&conn, &dir, &photo("a", "s", 1, 1, 100, 1));
        add(&conn, &dir, &photo("b", "s", 1, 1, 100, 2));
        mark_photo_shown(&mut conn, "a").unwrap();
        let shared = Arc::new(Mutex::new(conn));
        let evicted =
            evict_old_cached_files_preserving_history_unlocked(&shared, &dir.0, 0, 100, &[])
                .unwrap();
        assert_eq!(evicted, vec!["a".to_string()]);
        assert!(!dir.0.join("wallpapers/photos/a.jpg").exists());
        assert!(dir.0.join("wallpapers/photos/b.jpg").exists());
    }

    #[test]
    fn reset_history_preserves_displayed_and_requeues_cached_files() {
        let mut conn = db();
        let dir = TempDir::new();
        for (i, id) in ["a", "b", "c"].iter().enumerate() {
            add(&conn, &dir, &photo(id, "album-x", 4000, 3000, 10, i as i64));
        }
        add(&conn, &dir, &photo("other", "album-y", 4000, 3000, 10, 9));
        for id in ["a", "b", "c", "other"] {
            mark_photo_shown(&mut conn, id).unwrap();
        }
        // "b" was evicted from disk: history is reset but it cannot be requeued.
        std::fs::remove_file(dir.0.join("wallpapers/photos/b.jpg")).unwrap();
        mark_file_evicted(&conn, "b").unwrap();

        let mut f = filter();
        f.album_slugs = vec!["album-x".into()];
        let requeued = reset_exhausted_filter_history(&mut conn, &f, &["c".to_string()]).unwrap();
        assert_eq!(
            requeued, 1,
            "only 'a' is on disk, not on screen and was shown"
        );

        assert!(get_history_entry(&conn, "a").unwrap().is_none());
        assert!(get_history_entry(&conn, "b").unwrap().is_none());
        assert!(
            get_history_entry(&conn, "c").unwrap().is_some(),
            "on-screen id kept"
        );
        assert!(
            get_history_entry(&conn, "other").unwrap().is_some(),
            "other album untouched"
        );
        assert!(
            get_cached_photo(&conn, "a")
                .unwrap()
                .unwrap()
                .last_shown_at
                .is_some(),
            "last_shown_at survives a reset so recency ordering still works"
        );
        assert!(get_cached_photo(&conn, "c")
            .unwrap()
            .unwrap()
            .last_shown_at
            .is_some());
        assert_eq!(count_unused_in_queue(&conn, &f).unwrap(), 1);
    }

    #[test]
    fn after_reset_least_recently_shown_comes_first() {
        let mut conn = db();
        let dir = TempDir::new();
        for (i, id) in ["a", "b", "c"].iter().enumerate() {
            add(&conn, &dir, &photo(id, "s", 4000, 3000, 10, i as i64));
        }
        for id in ["a", "b", "c"] {
            mark_photo_shown(&mut conn, id).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        reset_exhausted_filter_history(&mut conn, &filter(), &[]).unwrap();
        let first = pop_next_queued_photo(&conn, &dir.0, &filter())
            .unwrap()
            .unwrap();
        assert_eq!(
            first.0, "a",
            "oldest shown first, the latest ('c') comes last"
        );
        assert_eq!(count_unused_in_queue(&conn, &filter()).unwrap(), 3);
    }

    #[test]
    fn recent_exclusions_cover_window_but_never_empty_the_pool() {
        let mut conn = db();
        let dir = TempDir::new();
        for i in 0..30 {
            add(
                &conn,
                &dir,
                &photo(&format!("p{i:02}"), "s", 4000, 3000, 10, i),
            );
        }
        let ids = |conn: &Connection, pool| recent_shown_exclusions(conn, &filter(), pool).unwrap();
        assert!(ids(&conn, RotationPhotoPool::PreviouslyShown).is_empty());
        for i in 0..30 {
            mark_photo_shown(&mut conn, &format!("p{i:02}")).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let recent = ids(&conn, RotationPhotoPool::PreviouslyShown);
        assert_eq!(recent.len(), RECENT_SHOWN_WINDOW);
        assert_eq!(recent[0], "p29");
        assert!(ids(&conn, RotationPhotoPool::Unused).is_empty());

        // Pool of 2 favorites: window is 1, so one candidate always remains.
        set_favorite(&conn, "p28", true).unwrap();
        set_favorite(&conn, "p29", true).unwrap();
        assert_eq!(
            ids(&conn, RotationPhotoPool::Favorites),
            vec!["p29".to_string()]
        );
        // Pool of 1: nothing excluded.
        set_favorite(&conn, "p28", false).unwrap();
        assert!(ids(&conn, RotationPhotoPool::Favorites).is_empty());
    }

    #[test]
    fn upsert_refreshes_album_and_flag_metadata() {
        let conn = db();
        let mut p = photo("a", "old-album", 4000, 3000, 10, 1);
        upsert_cached_photo(&conn, &p).unwrap();
        p.album_slug = "new-album".into();
        p.album_url = "https://iantruongphotography.com/album/new-album".into();
        p.featured = true;
        p.fallback_used = true;
        upsert_cached_photo(&conn, &p).unwrap();
        let row = get_cached_photo(&conn, "a").unwrap().unwrap();
        assert_eq!(row.album_slug, "new-album");
        assert!(row.album_url.ends_with("new-album"));
        assert!(row.featured && row.fallback_used);
    }

    #[test]
    fn eviction_rechecks_favorite_and_displayed_state_before_each_delete() {
        let mut conn = db();
        let dir = TempDir::new();
        for (i, id) in ["a", "b", "c"].iter().enumerate() {
            add(&conn, &dir, &photo(id, "s", 1, 1, 100, i as i64));
            mark_photo_shown(&mut conn, id).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // After the plan would be computed: "a" gets favorited, "b" goes on screen.
        set_favorite(&conn, "a", true).unwrap();
        assign_photo_to_monitor(&conn, "DISPLAY-1", "b").unwrap();
        let shared = Arc::new(Mutex::new(conn));
        // The caller's displayed list is stale (empty); the fresh DB state must win.
        let evicted =
            evict_old_cached_files_preserving_history_unlocked(&shared, &dir.0, 0, 0, &[]).unwrap();
        assert_eq!(evicted, vec!["c".to_string()]);
        assert!(dir.0.join("wallpapers/photos/a.jpg").exists());
        assert!(dir.0.join("wallpapers/photos/b.jpg").exists());
        assert!(!dir.0.join("wallpapers/photos/c.jpg").exists());
    }

    #[test]
    fn blacklist_clear_cache_and_favorites() {
        let mut conn = db();
        let dir = TempDir::new();
        add(&conn, &dir, &photo("keep", "s", 1, 1, 5, 1));
        add(&conn, &dir, &photo("fav", "s", 1, 1, 5, 2));
        add(&conn, &dir, &photo("screen", "s", 1, 1, 5, 3));
        add(&conn, &dir, &photo("junk", "s", 1, 1, 5, 4));
        set_favorite(&conn, "fav", true).unwrap();
        mark_photo_shown(&mut conn, "fav").unwrap();
        let favorites = list_favorite_photos(&conn, &dir.0).unwrap();
        assert_eq!(favorites.len(), 1);
        assert_eq!(favorites[0].id, "fav");
        assert!(favorites[0].last_shown_at.is_some());

        let path = blacklist_photo(&conn, &dir.0, "junk").unwrap().unwrap();
        assert!(path.ends_with("junk.jpg"));
        assert_eq!(blacklist_count(&conn).unwrap(), 1);
        assert_eq!(
            list_blacklist(&conn).unwrap()[0].album_slug.as_deref(),
            Some("s")
        );
        assert!(
            !get_cached_photo(&conn, "junk")
                .unwrap()
                .unwrap()
                .file_present
        );

        assign_photo_to_monitor(&conn, "DISPLAY-1", "screen").unwrap();
        assert_eq!(
            monitors_showing_photo(&conn, "screen").unwrap(),
            vec!["DISPLAY-1"]
        );
        assert_eq!(list_displayed_photo_ids(&conn).unwrap(), vec!["screen"]);
        let by_mon = current_photo_by_monitor(&conn, &dir.0).unwrap();
        assert_eq!(by_mon["DISPLAY-1"].as_ref().unwrap().id, "screen");

        let plan = plan_clear_cache(&conn, &dir.0, &["screen".to_string()]).unwrap();
        let ids: Vec<_> = plan.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["keep"]);
        assert_eq!(cached_bytes_total(&conn).unwrap(), 15);
    }

    #[test]
    fn saved_albums_and_settings_roundtrip() {
        let conn = db();
        let album = SavedAlbumUpsert {
            album_slug: "cape-horn-11-7".into(),
            title: "Cape Horn 11-7".into(),
            description: Some("x".into()),
            category: Some("Hikes".into()),
            album_url: "https://iantruongphotography.com/album/cape-horn-11-7".into(),
            cover_thumbnail_url: Some("https://c/t.jpg".into()),
            cover_blurhash: None,
            image_count: Some(1),
            added_at: 5,
        };
        upsert_saved_album(&conn, &album).unwrap();
        set_album_enabled(&conn, "cape-horn-11-7", false).unwrap();
        assert!(list_enabled_album_slugs(&conn).unwrap().is_empty());
        upsert_saved_album(&conn, &album).unwrap();
        assert_eq!(
            list_enabled_album_slugs(&conn).unwrap(),
            vec!["cape-horn-11-7"]
        );
        let rows = list_saved_albums(&conn).unwrap();
        assert_eq!(rows[0].image_count, Some(1));
        assert_eq!(SavedAlbum::from(&rows[0]).title, "Cape Horn 11-7");
        let bad = SavedAlbumUpsert {
            album_slug: "Bad Slug".into(),
            ..album.clone()
        };
        assert!(matches!(
            upsert_saved_album(&conn, &bad),
            Err(CoreError::InvalidInput(_))
        ));

        assert_eq!(
            load_photo_settings(&conn).unwrap(),
            PhotoSettings::default()
        );
        save_photo_filter(&conn, FilterMode::Albums, true).unwrap();
        save_max_cache_bytes(&conn, 1234).unwrap();
        set_setting(&conn, SETTING_EXHAUSTION_AUTO_RESET, &false).unwrap();
        let s = load_photo_settings(&conn).unwrap();
        assert_eq!(s.filter_mode, FilterMode::Albums);
        assert!(s.featured_only && !s.exhaustion_auto_reset);
        assert_eq!(s.max_cache_bytes, 1234);

        let (_, f, displayed) = load_active_filter(&conn, Some("portrait".into())).unwrap();
        assert_eq!(f.album_slugs, vec!["cape-horn-11-7"]);
        assert!(f.featured_only && !f.auto_reset_history_on_exhaustion);
        assert_eq!(f.required_orientation.as_deref(), Some("portrait"));
        assert!(displayed.is_empty());

        save_photo_filter(&conn, FilterMode::All, true).unwrap();
        let (_, all, _) = load_active_filter(&conn, None).unwrap();
        assert!(!all.featured_only);
        assert!(all.album_slugs.is_empty());

        remove_saved_album(&conn, "cape-horn-11-7").unwrap();
        assert!(list_saved_albums(&conn).unwrap().is_empty());
    }
}
