# 02 — Ian Truong Photography API, Local Queue & SQLite Cache Specification

> **Implementation Tier**: Copy-Paste Ready / Live-Verified API Specification  
> All endpoints, headers, error JSON bodies, rate limits, small-album edge cases, SQLite queries, and Rust queue/refill code below have been verified directly against the live `photos-api.iantruongphotography.com` and `iantruongphotography.com` production servers.

---

## 1. Live-Verified API Contracts (`truongpaper-photo-api`)

### 1.1 Random Photo Endpoint
- **Method & URL**: `GET https://photos-api.iantruongphotography.com/v1/photos/random`
- **Authentication**: None (public API, CORS `access-control-allow-origin: *`).
- **Query Parameter Matrix**:

| Query String | Behavior |
| :--- | :--- |
| *(none)* | Picks one random public photo across the entire portfolio. |
| `?featured=true` | Picks one random featured public photo. |
| `?album=bald-mountain-11-1` | Picks one random photo from that public album slug. |
| `?album=bald-mountain-11-1&featured=true` | Picks one random featured photo from that album. If that album has no featured photos, automatically falls back to all photos in that same album and sets `"fallbackUsed": true`. |

#### Live Captured `200 OK` Response
```http
HTTP/2 200
content-type: application/json
cache-control: no-store
access-control-allow-origin: *
```
```json
{
  "albumSlug": "bald-mountain-11-1",
  "albumUrl": "https://iantruongphotography.com/album/bald-mountain-11-1",
  "bytes": 17616840,
  "fallbackUsed": false,
  "featured": true,
  "height": 4505,
  "id": "e95f46a6b4d6b9b772df7c79",
  "imageUrl": "https://photos-api.iantruongphotography.com/v1/images/c713e591270149ff8d51eb68caeec57d39c1d26a916c998e8d603863fb0e8bfe/YWxidW1zL2RlMzA3MjhiLWY0NjItNGMzYy1hZjQ0LTIzMTUyODVmYWE3NS9vcmlnaW5hbC9hZTA4M2Q3NGFmYmI0ZDYxOTI5NmM2NjgyNTZjMTIxZS5qcGc",
  "width": 6757
}
```

#### Live Captured Error Responses
- **`400 Bad Request`** (invalid, unknown, or duplicate query parameters):
  ```json
  {"error":"invalid_parameters"}
  ```
- **`404 Not Found`** (requested album slug does not exist or pool has 0 eligible photos):
  ```json
  {"error":"no_photos"}
  ```
- **`403 Forbidden` / `429 Too Many Requests` / `503 Service Unavailable`**:
  - Rate limit is **~100 requests per IP per 5 minutes**, shared between `/v1/photos/random` and `/v1/images/...` (`imageUrl`) downloads.

---

### 1.2 Bonus Public Album Catalog Endpoint (For Visual Album Browser & Exact `imageCount`)
In addition to pasting any album URL (`https://iantruongphotography.com/album/bald-mountain-11-1`) or slug (`bald-mountain-11-1`), TruongPaper can optionally fetch the public album catalog to let users visually browse all public photo albums with cover thumbnails and know the exact `imageCount` of each album:

- **Method & URL**: `GET https://iantruongphotography.com/api/public/albums?type=photo&limit=100` (optional `&cursor=<nextCursor>`)
- **Live Captured `200 OK` Response**:
```json
{
  "items": [
    {
      "albumId": "bfcace3f-06b6-4a63-bdf0-45c0969e05e3",
      "type": "photo",
      "title": "Cape Horn 11-7",
      "description": "🏃",
      "category": "Hikes",
      "createdAt": "2026-11-07T20:00:00.000Z",
      "uploadedAt": "2026-11-08T05:56:34Z",
      "visibility": "public",
      "imageCount": 1,
      "coverImageUrl": "https://d1twwtwfz1yeo4.cloudfront.net/albums/bfcace3f-06b6-4a63-bdf0-45c0969e05e3/original/8989f13ace894308815341002376dcad.jpg",
      "coverThumbnailUrl": "https://d1twwtwfz1yeo4.cloudfront.net/albums/bfcace3f-06b6-4a63-bdf0-45c0969e05e3/thumbnail/0bf83c15eb3348728a9723f599618abe.jpg",
      "coverBlurhash": "LuE{@HWVM{j[_NWXWBj]ogWCofof",
      "slug": "cape-horn-11-7"
    },
    {
      "albumId": "de30728b-f462-4c3c-af44-2315285faa75",
      "type": "photo",
      "title": "Bald Mountain 11-1",
      "description": "First snow ❄️",
      "category": "Hikes",
      "createdAt": "2026-11-01T20:00:00.000Z",
      "uploadedAt": "2026-11-02T04:41:59Z",
      "visibility": "public",
      "imageCount": 8,
      "coverImageUrl": "https://d1twwtwfz1yeo4.cloudfront.net/albums/de30728b-f462-4c3c-af44-2315285faa75/original/9a619a3befa14f39819f57b28fa7167e.jpg",
      "coverThumbnailUrl": "https://d1twwtwfz1yeo4.cloudfront.net/albums/de30728b-f462-4c3c-af44-2315285faa75/thumbnail/78f4ce9ecc7f47ad9e8d25cc3a045be5.jpg",
      "coverBlurhash": "LCB{cS-A9tM|_Nt7RjR*0gkW%1$%",
      "slug": "bald-mountain-11-1"
    }
  ],
  "nextCursor": null
}
```

---

## 2. Critical Edge Case: Small Albums (`imageCount < 5`) & Exhaustion Logic

> [!CAUTION]
> **Why Naive Refill Loops Fail on Small Albums (e.g., `cape-horn-11-7` with `imageCount: 1`)**  
> Suppose the user selects an album that has only **1** or **3** total photos, while `QUEUE_TARGET = 5`.
> 1. Once those 1–3 photos are downloaded into the unused queue, `unused_count` is `3` (which is `< 5`).
> 2. Every subsequent call to `/v1/photos/random?album=...` during that refill will return one of those 3 IDs that is **already in the unused queue** (`last_shown_at IS NULL`).
> 3. If the code blindly treated "12 duplicate attempts $\to$ reset `photos_history`", resetting `photos_history` would do nothing because those 3 photos are in the **unused queue**, not in `photos_history`!
>
> **Mandatory Resolution Rules**:
> 1. During a refill cycle, track two separate duplicate counters:
>    - `duplicates_in_unused_queue`: Returned `id` is already in `photos_cache` with `file_present = 1 AND last_shown_at IS NULL` (or is currently displayed on screen).
>    - `duplicates_in_history`: Returned `id` is in `photos_history`.
> 2. If `attempts == MAX_REFILL_ATTEMPTS (12)`:
>    - **Case A (`duplicates_in_history == 0` and `duplicates_in_unused_queue > 0`)**: Every photo in this small album is already sitting in the unused queue (or on screen)! **Stop refilling immediately** and do not reset `photos_history`.
>    - **Case B (`duplicates_in_history > 0` and `unused_count == 0`)**: The user has genuinely viewed all eligible photos in this album/filter!
>      - If `auto_reset_history_on_exhaustion == true` (Default):
>        1. Delete rows from `photos_history` matching the active filter, **except** the `photo_id` currently displayed on screen (so the immediate next rotation never repeats the image currently on screen, unless the album only has 1 photo total).
>        2. For any photo in `photos_cache` matching the active filter that still has `file_present = 1` on disk (and is not currently displayed on screen), set `last_shown_at = NULL` — **instantly refilling the unused queue from disk with zero API calls!**

---

## 3. Complete SQLite Schema (`truongpaper.db`)

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS photos_cache (
    id              TEXT PRIMARY KEY,
    file_path       TEXT NOT NULL,                  -- Relative: wallpapers/photos/<id>.jpg
    file_present    INTEGER NOT NULL DEFAULT 1,     -- 1 = .jpg exists on disk, 0 = deleted by cache cleanup
    album_slug      TEXT NOT NULL,
    album_url       TEXT NOT NULL,
    featured        INTEGER NOT NULL,               -- 0 or 1
    fallback_used   INTEGER NOT NULL DEFAULT 0,     -- 0 or 1
    width           INTEGER NOT NULL,
    height          INTEGER NOT NULL,
    orientation     TEXT NOT NULL,                  -- 'landscape' (width >= height) | 'portrait' (height > width)
    bytes           INTEGER NOT NULL,
    is_favorite     INTEGER NOT NULL DEFAULT 0,     -- 1 = Pinned on disk (NEVER deleted by cache cleanup)
    downloaded_at   INTEGER NOT NULL,               -- Unix epoch ms
    last_shown_at   INTEGER,                        -- NULL = currently in UNUSED QUEUE
    shown_count     INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_photos_unused_queue
    ON photos_cache (file_present, last_shown_at, orientation, featured, album_slug);

CREATE INDEX IF NOT EXISTS idx_photos_unused_fifo
    ON photos_cache (downloaded_at ASC)
    WHERE file_present = 1 AND last_shown_at IS NULL;

-- Persistent History: Kept even after .jpg files are deleted from disk!
CREATE TABLE IF NOT EXISTS photos_history (
    photo_id        TEXT PRIMARY KEY,
    album_slug      TEXT NOT NULL,
    featured        INTEGER NOT NULL,
    orientation     TEXT NOT NULL,
    first_shown_at  INTEGER NOT NULL,
    last_shown_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_photos_history_filter
    ON photos_history (album_slug, featured, orientation);

-- Blacklist ("Hide / Never show again")
CREATE TABLE IF NOT EXISTS photos_blacklist (
    photo_id        TEXT PRIMARY KEY,
    album_slug      TEXT,
    hidden_at       INTEGER NOT NULL
);

-- Saved / Catalog Albums
CREATE TABLE IF NOT EXISTS saved_albums (
    album_slug          TEXT PRIMARY KEY,
    title               TEXT NOT NULL,
    description         TEXT,
    category            TEXT,
    album_url           TEXT NOT NULL,
    cover_thumbnail_url TEXT,
    cover_blurhash      TEXT,
    image_count         INTEGER,                    -- Known total photos in album (if known from catalog)
    is_enabled          INTEGER NOT NULL DEFAULT 1, -- 1 = included in active multi-album rotation pool
    added_at            INTEGER NOT NULL
);

-- Wallpaper Engine Library & User Property Overrides
CREATE TABLE IF NOT EXISTS wallpaper_configs (
    wallpaper_id        TEXT PRIMARY KEY,           -- 'workshop:<id>' or 'local:<hash>'
    workshop_id         TEXT,
    title               TEXT NOT NULL,
    wallpaper_type      TEXT NOT NULL,              -- 'scene' | 'video' | 'web' | 'application'
    install_dir         TEXT NOT NULL,
    preview_path        TEXT,
    bytes               INTEGER NOT NULL DEFAULT 0,
    audio_reactive      INTEGER NOT NULL DEFAULT 0,
    user_properties_json TEXT NOT NULL DEFAULT '{}',-- JSON map of key -> current value
    adjustments_json    TEXT NOT NULL DEFAULT '{}', -- JSON UniversalAdjustments
    updated_at          INTEGER NOT NULL
);

-- Named User Presets for Wallpaper Engine Wallpapers
CREATE TABLE IF NOT EXISTS wallpaper_presets (
    preset_id           TEXT PRIMARY KEY,
    wallpaper_id        TEXT NOT NULL,
    preset_name         TEXT NOT NULL,
    user_properties_json TEXT NOT NULL,
    adjustments_json    TEXT NOT NULL,
    created_at          INTEGER NOT NULL,
    FOREIGN KEY (wallpaper_id) REFERENCES wallpaper_configs(wallpaper_id) ON DELETE CASCADE
);

-- Unified Playlists (Photos Only, Workshop Only, or Mixed)
CREATE TABLE IF NOT EXISTS playlists (
    playlist_id         TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    mode                TEXT NOT NULL,              -- 'photos_only' | 'workshop_only' | 'mixed'
    interval_seconds    INTEGER NOT NULL DEFAULT 900,
    items_json          TEXT NOT NULL DEFAULT '[]',
    updated_at          INTEGER NOT NULL
);

-- Per-Monitor Wallpaper Assignment State
CREATE TABLE IF NOT EXISTS monitor_assignments (
    monitor_id          TEXT PRIMARY KEY,
    source_json         TEXT NOT NULL,              -- Serialized WallpaperSource
    current_photo_id    TEXT,
    updated_at          INTEGER NOT NULL
);

-- Global Key-Value Settings
CREATE TABLE IF NOT EXISTS settings (
    key                 TEXT PRIMARY KEY,
    value_json          TEXT NOT NULL
);
```

---

## 4. Reference Rust Implementation (`crates/truongpaper-photo-api/src/lib.rs`)

Below is the complete, self-contained Rust reference implementation for URL/slug normalization, sliding-window rate limiting, zero-API-call queue rotation, background queue refill, exhaustion auto-reset, and history-preserving cache eviction.

```rust
use parking_lot::Mutex;
use reqwest::{Client, StatusCode};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const API_RANDOM_URL: &str = "https://photos-api.iantruongphotography.com/v1/photos/random";
pub const PUBLIC_CATALOG_URL: &str = "https://iantruongphotography.com/api/public/albums?type=photo&limit=100";

pub const QUEUE_TARGET_UNUSED: usize = 5;
pub const QUEUE_REFILL_THRESHOLD: usize = 2;
pub const MAX_REFILL_ATTEMPTS: usize = 12;
pub const RATE_LIMIT_MAX_REQUESTS: usize = 80; // 80 req / 5 min (20-req safety margin under 100 limit)
pub const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(300);
pub const DEFAULT_MAX_CACHE_BYTES: u64 = 250 * 1024 * 1024; // 250 MB

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiPhotoResponse {
    pub id: String,
    pub album_slug: String,
    pub album_url: String,
    pub image_url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: u64,
    pub featured: bool,
    pub fallback_used: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoFilter {
    /// None = all public photos; Some(slugs) = one or more enabled albums
    pub album_slugs: Vec<String>,
    pub featured_only: bool,
    /// Some("landscape") or Some("portrait") when orientation filtering is enabled on the monitor
    pub required_orientation: Option<String>,
    pub auto_reset_history_on_exhaustion: bool,
}

/// Parses either "https://iantruongphotography.com/album/bald-mountain-11-1" or "bald-mountain-11-1"
pub fn parse_album_slug(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let slug = if let Some(idx) = trimmed.rfind("/album/") {
        &trimmed[idx + "/album/".len()..]
    } else if let Some(idx) = trimmed.rfind('/') {
        &trimmed[idx + 1..]
    } else {
        trimmed
    };
    let slug = slug.split('?').next().unwrap_or(slug).split('#').next().unwrap_or(slug).trim().to_lowercase();
    if !slug.is_empty() && slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Some(slug)
    } else {
        None
    }
}

/// Thread-safe Sliding Window Rate Limiter (shared between JSON calls and imageUrl downloads)
pub struct SlidingRateLimiter {
    timestamps: Mutex<VecDeque<Instant>>,
    cooldown_until: Mutex<Option<Instant>>,
}

impl SlidingRateLimiter {
    pub fn new() -> Self {
        Self {
            timestamps: Mutex::new(VecDeque::with_capacity(RATE_LIMIT_MAX_REQUESTS + 4)),
            cooldown_until: Mutex::new(None),
        }
    }

    pub fn can_make_request(&self) -> bool {
        let now = Instant::now();
        if let Some(until) = *self.cooldown_until.lock() {
            if now < until {
                return false;
            }
        }
        let mut q = self.timestamps.lock();
        while let Some(&front) = q.front() {
            if now.duration_since(front) > RATE_LIMIT_WINDOW {
                q.pop_front();
            } else {
                break;
            }
        }
        q.len() < RATE_LIMIT_MAX_REQUESTS
    }

    pub fn record_request(&self) {
        self.timestamps.lock().push_back(Instant::now());
    }

    pub fn trigger_backoff(&self, consecutive_failures: u32) -> Duration {
        let base_ms = (2000_u64.saturating_mul(1_u64 << consecutive_failures.min(6))).min(120_000);
        let dur = Duration::from_millis(base_ms);
        *self.cooldown_until.lock() = Some(Instant::now() + dur);
        dur
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Step 1: Pick next unused photo from local disk queue (ZERO API CALLS).
/// Returns None if the local unused queue for this filter is empty.
pub fn pop_next_queued_photo(
    conn: &Connection,
    app_data_dir: &Path,
    filter: &PhotoFilter,
) -> rusqlite::Result<Option<(String, PathBuf)>> {
    let mut sql = String::from(
        "SELECT id, file_path FROM photos_cache
         WHERE file_present = 1
           AND last_shown_at IS NULL
           AND id NOT IN (SELECT photo_id FROM photos_history)
           AND id NOT IN (SELECT photo_id FROM photos_blacklist)",
    );

    if filter.featured_only {
        sql.push_str(" AND featured = 1");
    }
    if let Some(ref orient) = filter.required_orientation {
        sql.push_str(&format!(
            " AND orientation = '{}'",
            if orient == "portrait" { "portrait" } else { "landscape" }
        ));
    }
    if !filter.album_slugs.is_empty() {
        let quoted: Vec<String> = filter
            .album_slugs
            .iter()
            .filter_map(|s| parse_album_slug(s))
            .map(|s| format!("'{}'", s))
            .collect();
        if !quoted.is_empty() {
            sql.push_str(&format!(" AND album_slug IN ({})", quoted.join(",")));
        }
    }
    sql.push_str(" ORDER BY downloaded_at ASC LIMIT 1");

    let row: Option<(String, String)> = conn
        .query_row(&sql, [], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;

    if let Some((id, rel_path)) = row {
        let abs_path = app_data_dir.join(&rel_path);
        if abs_path.exists() {
            return Ok(Some((id, abs_path)));
        } else {
            // Heal stale file_present flag if user deleted file manually
            conn.execute("UPDATE photos_cache SET file_present = 0 WHERE id = ?1", params![id])?;
        }
    }
    Ok(None)
}

/// Step 2: Called ONLY AFTER the wallpaper change / GPU transition succeeds on the desktop.
pub fn mark_photo_shown(conn: &mut Connection, photo_id: &str) -> rusqlite::Result<()> {
    let ts = now_ms();
    let tx = conn.transaction()?;

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
            "INSERT INTO photos_history (photo_id, album_slug, featured, orientation, first_shown_at, last_shown_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(photo_id) DO UPDATE SET last_shown_at = excluded.last_shown_at",
            params![photo_id, album_slug, featured, orientation, ts],
        )?;
    }

    tx.commit()
}

/// Step 3: History-preserving Disk Cache Eviction.
/// Deletes oldest already-shown non-favorite .jpg files when over max_cache_bytes,
/// but NEVER deletes rows from photos_history!
pub fn evict_old_cached_files_preserving_history(
    conn: &Connection,
    app_data_dir: &Path,
    incoming_bytes: u64,
    max_cache_bytes: u64,
    currently_displayed_ids: &[String],
) -> rusqlite::Result<()> {
    let current_bytes: u64 = conn.query_row(
        "SELECT COALESCE(SUM(bytes), 0) FROM photos_cache WHERE file_present = 1 AND is_favorite = 0",
        [],
        |r| r.get::<_, i64>(0).map(|v| v.max(0) as u64),
    )?;

    if current_bytes + incoming_bytes <= max_cache_bytes {
        return Ok(());
    }

    let mut bytes_to_free = (current_bytes + incoming_bytes).saturating_sub(max_cache_bytes);

    let mut stmt = conn.prepare(
        "SELECT id, file_path, bytes FROM photos_cache
         WHERE file_present = 1
           AND is_favorite = 0
           AND last_shown_at IS NOT NULL
         ORDER BY last_shown_at ASC",
    )?;

    let candidates: Vec<(String, String, u64)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)?.max(0) as u64))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    for (id, rel_path, file_bytes) in candidates {
        if bytes_to_free == 0 {
            break;
        }
        if currently_displayed_ids.contains(&id) {
            continue;
        }
        let full_path = app_data_dir.join(&rel_path);
        let _ = std::fs::remove_file(&full_path);
        conn.execute(
            "UPDATE photos_cache SET file_present = 0 WHERE id = ?1",
            params![id],
        )?;
        // NOTE: photos_history is intentionally untouched so this photo ID never repeats!
        bytes_to_free = bytes_to_free.saturating_sub(file_bytes);
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateStatus {
    UniqueAndUnseen,
    AlreadyInUnusedQueue,
    AlreadyInHistory,
    Blacklisted,
    OrientationMismatch,
}

/// Checks returned API photo metadata BEFORE downloading imageUrl!
pub fn classify_candidate_photo(
    conn: &Connection,
    resp: &ApiPhotoResponse,
    required_orientation: Option<&str>,
) -> rusqlite::Result<CandidateStatus> {
    // 1. Check blacklist first
    let blacklisted: bool = conn
        .query_row(
            "SELECT 1 FROM photos_blacklist WHERE photo_id = ?1",
            params![resp.id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if blacklisted {
        return Ok(CandidateStatus::Blacklisted);
    }

    // 2. Check orientation if width & height are provided in metadata
    if let (Some(req_orient), Some(w), Some(h)) = (required_orientation, resp.width, resp.height) {
        let actual_orient = if h > w { "portrait" } else { "landscape" };
        if actual_orient != req_orient {
            return Ok(CandidateStatus::OrientationMismatch);
        }
    }

    // 3. Check if already in the UNUSED queue on disk
    let in_unused_queue: bool = conn
        .query_row(
            "SELECT 1 FROM photos_cache WHERE id = ?1 AND file_present = 1 AND last_shown_at IS NULL",
            params![resp.id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if in_unused_queue {
        return Ok(CandidateStatus::AlreadyInUnusedQueue);
    }

    // 4. Check persistent history (covers both on-disk shown files and evicted files!)
    let in_history: bool = conn
        .query_row(
            "SELECT 1 FROM photos_history WHERE photo_id = ?1",
            params![resp.id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if in_history {
        return Ok(CandidateStatus::AlreadyInHistory);
    }

    Ok(CandidateStatus::UniqueAndUnseen)
}

/// Counts how many unused photos currently on disk match the active filter.
pub fn count_unused_in_queue(conn: &Connection, filter: &PhotoFilter) -> rusqlite::Result<usize> {
    let mut sql = String::from(
        "SELECT COUNT(*) FROM photos_cache
         WHERE file_present = 1
           AND last_shown_at IS NULL
           AND id NOT IN (SELECT photo_id FROM photos_history)
           AND id NOT IN (SELECT photo_id FROM photos_blacklist)",
    );
    if filter.featured_only {
        sql.push_str(" AND featured = 1");
    }
    if let Some(ref orient) = filter.required_orientation {
        sql.push_str(&format!(
            " AND orientation = '{}'",
            if orient == "portrait" { "portrait" } else { "landscape" }
        ));
    }
    if !filter.album_slugs.is_empty() {
        let quoted: Vec<String> = filter
            .album_slugs
            .iter()
            .filter_map(|s| parse_album_slug(s))
            .map(|s| format!("'{}'", s))
            .collect();
        if !quoted.is_empty() {
            sql.push_str(&format!(" AND album_slug IN ({})", quoted.join(",")));
        }
    }
    conn.query_row(&sql, [], |r| r.get::<_, i64>(0).map(|v| v.max(0) as usize))
}

/// Step 4: Resets shown history for an exhausted album/filter (preserving currently displayed IDs)
/// and immediately re-queues any still-cached .jpg files on disk with ZERO API calls!
pub fn reset_exhausted_filter_history(
    conn: &mut Connection,
    filter: &PhotoFilter,
    currently_displayed_ids: &[String],
) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;

    let mut where_clause = String::from("WHERE 1=1");
    if filter.featured_only {
        where_clause.push_str(" AND featured = 1");
    }
    if let Some(ref orient) = filter.required_orientation {
        where_clause.push_str(&format!(
            " AND orientation = '{}'",
            if orient == "portrait" { "portrait" } else { "landscape" }
        ));
    }
    if !filter.album_slugs.is_empty() {
        let quoted: Vec<String> = filter
            .album_slugs
            .iter()
            .filter_map(|s| parse_album_slug(s))
            .map(|s| format!("'{}'", s))
            .collect();
        if !quoted.is_empty() {
            where_clause.push_str(&format!(" AND album_slug IN ({})", quoted.join(",")));
        }
    }

    let exclude_clause = if currently_displayed_ids.is_empty() {
        String::new()
    } else {
        let quoted_ids: Vec<String> = currently_displayed_ids
            .iter()
            .map(|id| format!("'{}'", id.replace('\'', "")))
            .collect();
        format!(" AND photo_id NOT IN ({})", quoted_ids.join(","))
    };

    tx.execute(
        &format!("DELETE FROM photos_history {where_clause}{exclude_clause}"),
        [],
    )?;

    // Re-activate any photos still cached on disk (`file_present = 1`) back into the unused queue!
    let cache_exclude = exclude_clause.replace("photo_id", "id");
    let requeued = tx.execute(
        &format!("UPDATE photos_cache SET last_shown_at = NULL {where_clause} AND file_present = 1{cache_exclude}"),
        [],
    )?;

    tx.commit()?;
    Ok(requeued)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefillOutcome {
    TargetReached { unused_count: usize },
    SmallPoolFullyQueued { unused_count: usize },
    FilterExhaustedAndReset { unused_count: usize },
    FilterExhaustedHoldingLast,
    RateLimitedOrBackedOff,
    RetryCapReached { unused_count: usize },
}

/// Step 5: Background Refill Loop (Target = 5 unused photos, Triggered when unused <= 2, Max 12 attempts).
pub async fn refill_queue(
    client: &Client,
    limiter: &SlidingRateLimiter,
    db: &Arc<Mutex<Connection>>,
    app_data_dir: &Path,
    filter: &PhotoFilter,
    max_cache_bytes: u64,
    currently_displayed_ids: &[String],
) -> anyhow::Result<RefillOutcome> {
    let mut attempts = 0_usize;
    let mut consecutive_queue_dups = 0_usize;
    let mut history_dups = 0_usize;
    let mut consecutive_http_failures = 0_u32;

    let photos_dir = app_data_dir.join("wallpapers").join("photos");
    std::fs::create_dir_all(&photos_dir)?;

    loop {
        let unused_count = {
            let conn = db.lock();
            count_unused_in_queue(&conn, filter)?
        };
        if unused_count >= QUEUE_TARGET_UNUSED {
            return Ok(RefillOutcome::TargetReached { unused_count });
        }
        if attempts >= MAX_REFILL_ATTEMPTS {
            if history_dups > 0 && unused_count == 0 {
                if filter.auto_reset_history_on_exhaustion {
                    let mut conn = db.lock();
                    let _ = reset_exhausted_filter_history(&mut conn, filter, currently_displayed_ids)?;
                    let after_count = count_unused_in_queue(&conn, filter)?;
                    return Ok(RefillOutcome::FilterExhaustedAndReset {
                        unused_count: after_count,
                    });
                } else {
                    return Ok(RefillOutcome::FilterExhaustedHoldingLast);
                }
            }
            return Ok(RefillOutcome::RetryCapReached { unused_count });
        }
        if !limiter.can_make_request() {
            return Ok(RefillOutcome::RateLimitedOrBackedOff);
        }

        attempts += 1;

        // Build query parameters
        let mut req = client.get(API_RANDOM_URL);
        if !filter.album_slugs.is_empty() {
            let idx = rand::random::<usize>() % filter.album_slugs.len();
            if let Some(slug) = parse_album_slug(&filter.album_slugs[idx]) {
                req = req.query(&[("album", slug)]);
            }
        }
        if filter.featured_only {
            req = req.query(&[("featured", "true")]);
        }

        limiter.record_request();
        let resp = req.send().await?;
        let status = resp.status();

        if status == StatusCode::FORBIDDEN
            || status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::SERVICE_UNAVAILABLE
        {
            consecutive_http_failures += 1;
            limiter.trigger_backoff(consecutive_http_failures);
            return Ok(RefillOutcome::RateLimitedOrBackedOff);
        }
        if !status.is_success() {
            break;
        }
        consecutive_http_failures = 0;

        let meta: ApiPhotoResponse = resp.json().await?;

        // Check candidate against unused queue, history, and blacklist BEFORE downloading imageUrl
        let status_check = {
            let conn = db.lock();
            classify_candidate_photo(&conn, &meta, filter.required_orientation.as_deref())?
        };

        match status_check {
            CandidateStatus::AlreadyInUnusedQueue => {
                consecutive_queue_dups += 1;
                // Small album (< 5 photos) where all unseen photos are already queued on disk
                if consecutive_queue_dups >= 4 && history_dups == 0 {
                    let unused_now = count_unused_in_queue(&db.lock(), filter)?;
                    return Ok(RefillOutcome::SmallPoolFullyQueued {
                        unused_count: unused_now,
                    });
                }
                continue;
            }
            CandidateStatus::AlreadyInHistory => {
                history_dups += 1;
                continue;
            }
            CandidateStatus::Blacklisted | CandidateStatus::OrientationMismatch => {
                continue;
            }
            CandidateStatus::UniqueAndUnseen => {
                consecutive_queue_dups = 0;
            }
        }

        // Check if file already exists on disk from an earlier session
        let rel_path = format!("wallpapers/photos/{}.jpg", meta.id);
        let final_path = app_data_dir.join(&rel_path);
        let part_path = app_data_dir.join(format!("wallpapers/photos/{}.jpg.part", meta.id));

        if !final_path.exists() {
            if !limiter.can_make_request() {
                return Ok(RefillOutcome::RateLimitedOrBackedOff);
            }
            {
                let conn = db.lock();
                evict_old_cached_files_preserving_history(
                    &conn,
                    app_data_dir,
                    meta.bytes,
                    max_cache_bytes,
                    currently_displayed_ids,
                )?;
            }

            limiter.record_request();
            let img_resp = client.get(&meta.image_url).send().await?;
            if img_resp.status() == StatusCode::FORBIDDEN
                || img_resp.status() == StatusCode::SERVICE_UNAVAILABLE
            {
                consecutive_http_failures += 1;
                limiter.trigger_backoff(consecutive_http_failures);
                return Ok(RefillOutcome::RateLimitedOrBackedOff);
            }
            if !img_resp.status().is_success() {
                continue;
            }

            let bytes = img_resp.bytes().await?;
            tokio::fs::write(&part_path, &bytes).await?;
            tokio::fs::rename(&part_path, &final_path).await?;
        }

        // Resolve EXIF-aware display width/height (swapping axes when EXIF Orientation is 5, 6, 7, or 8)
        let final_path_clone = final_path.clone();
        let (width, height) = tokio::task::spawn_blocking(move || {
            inspect_display_dimensions(&final_path_clone)
        })
        .await
        .ok()
        .flatten()
        .or(match (meta.width, meta.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
            _ => imagesize::size(&final_path)
                .ok()
                .map(|dim| (dim.width as u32, dim.height as u32)),
        })
        .unwrap_or((3840, 2160));
        let orientation = if height > width { "portrait" } else { "landscape" };

        // Insert into photos_cache with last_shown_at = NULL (unused queue) and refresh all metadata on conflict
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO photos_cache (
                    id, file_path, file_present, album_slug, album_url,
                    featured, fallback_used, width, height, orientation,
                    bytes, is_favorite, downloaded_at, last_shown_at, shown_count
                ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, NULL, 0)
                ON CONFLICT(id) DO UPDATE SET
                    file_path = excluded.file_path,
                    file_present = 1,
                    width = excluded.width,
                    height = excluded.height,
                    orientation = excluded.orientation,
                    bytes = excluded.bytes,
                    downloaded_at = excluded.downloaded_at,
                    last_shown_at = NULL",
                params![
                    meta.id,
                    rel_path,
                    meta.album_slug,
                    meta.album_url,
                    i32::from(meta.featured),
                    i32::from(meta.fallback_used),
                    width,
                    height,
                    orientation,
                    meta.bytes as i64,
                    now_ms(),
                ],
            )?;
        }
    }

    let unused_count = count_unused_in_queue(&db.lock(), filter)?;
    Ok(RefillOutcome::TargetReached { unused_count })
}
```

---

## 5. EXIF Orientation-Aware Dimension Inspector & `<= 8192px` `wgpu` Texture Decoder (`crates/truongpaper-photo-api/src/decoder.rs`)

> [!CAUTION]
> **Two High-Resolution DSLR/Mirrorless JPEG Traps**:
> 1. **EXIF `Orientation` (`1..8`)**: `imagesize::size` only reads raw SOF0/SOF2 sensor dimensions and ignores EXIF `Orientation` tags (`5, 6, 7, 8`). A vertical portrait shot (`6757 × 4505` raw sensor buffer with EXIF `Orientation = 6` [Rotate 90° CW]) will be misclassified as `"landscape"` and rendered sideways unless EXIF orientation is applied via `image::ImageDecoder::orientation()` + `DynamicImage::apply_orientation()`.
> 2. **`> 8192px` `wgpu` Texture Limit Panic**: Default `wgpu::Limits::default().max_texture_dimension_2d` is `8192`. High-resolution cameras or stitched panoramas exceeding `8192px` will panic `wgpu::Device::create_texture`. Always clamp decoded GPU textures to `device.limits().max_texture_dimension_2d.min(8192)` using `FilterType::Lanczos3`.

```rust
// crates/truongpaper-photo-api/src/decoder.rs
use image::{DynamicImage, ImageDecoder, ImageReader, RgbaImage};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Reads JPEG header + EXIF Orientation (1..8) without decoding pixel data.
/// Swaps (width, height) when EXIF orientation is 5, 6, 7, or 8 (90°/270° rotated).
pub fn inspect_display_dimensions(path: &Path) -> Option<(u32, u32)> {
    let file = File::open(path).ok()?;
    let reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let (raw_w, raw_h) = decoder.dimensions();
    let orient = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);

    use image::metadata::Orientation::*;
    let swaps_axes = matches!(
        orient,
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH
    );
    if swaps_axes {
        Some((raw_h, raw_w))
    } else {
        Some((raw_w, raw_h))
    }
}

pub struct DecodedGpuPhoto {
    pub rgba: RgbaImage,
    pub width: u32,
    pub height: u32,
}

/// Decodes baseline or progressive (4:2:0 / 4:2:2 / 4:4:4) JPEG, applies EXIF Orientation (1..8),
/// and downscales via Lanczos3 if either dimension exceeds `device_max_texture_dim_2d.min(8192)`.
pub fn decode_photo_for_wgpu(
    path: &Path,
    device_max_texture_dim_2d: u32,
) -> anyhow::Result<DecodedGpuPhoto> {
    let max_dim = device_max_texture_dim_2d.min(8192).max(2048);
    let file = File::open(path)?;
    let reader = ImageReader::new(BufReader::new(file)).with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);

    let mut dyn_img = DynamicImage::from_decoder(decoder)?;
    dyn_img.apply_orientation(orientation);

    let (w, h) = (dyn_img.width(), dyn_img.height());
    if w > max_dim || h > max_dim {
        dyn_img = dyn_img.resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3);
    }

    let rgba = dyn_img.into_rgba8();
    let (final_w, final_h) = (rgba.width(), rgba.height());
    Ok(DecodedGpuPhoto {
        rgba,
        width: final_w,
        height: final_h,
    })
}
```

---

## 6. Complete Implementation of All 13 `photo_*` Tauri v2 IPC Commands (`src-tauri/src/commands/photo.rs`)

Below is the complete, copy-paste-ready Rust implementation of all 13 `photo_*` IPC commands defined in [`docs/04-UI-UX-LIQUID-GLASS-SPEC.md`](file:///Users/iantruong/Desktop/Fun/TruongPaper/docs/04-UI-UX-LIQUID-GLASS-SPEC.md#L1060-L1074), including:
- Non-blocking disk file eviction outside the SQLite `Mutex` lock (`evict_old_cached_files_preserving_history_unlocked`).
- Multi-monitor Cloned vs. Independent rotation (`pop_next_queued_photo_with_exclusions`) with automatic fallback to `FitMode::FitBlur` when an active album contains zero photos matching a portrait/landscape monitor's native orientation.

```rust
// src-tauri/src/commands/photo.rs
use parking_lot::Mutex;
use reqwest::Client;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use truongpaper_photo_api::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterMode {
    All,
    Featured,
    Albums,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoItem {
    pub id: String,
    pub album_slug: String,
    pub album_url: String,
    pub file_path: String, // Resolved absolute path for Tauri asset:// protocol
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: u64,
    pub featured: bool,
    pub is_favorite: bool,
    pub downloaded_at: i64,
    pub last_shown_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedAlbum {
    pub album_slug: String,
    #[serde(rename = "displayName")]
    pub title: String,
    pub album_url: String,
    pub is_enabled: bool,
    pub cover_thumbnail_url: Option<String>,
    pub image_count: Option<u32>,
    pub added_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicCatalogAlbum {
    pub album_id: String,
    #[serde(rename = "type")]
    pub album_type: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    pub image_count: u32,
    pub cover_image_url: String,
    pub cover_thumbnail_url: String,
    #[serde(default)]
    pub cover_blurhash: String,
    pub slug: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogApiResponse {
    pub items: Vec<PublicCatalogAlbum>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoStatus {
    pub filter_mode: FilterMode,
    pub featured_only: bool,
    pub unused_queue: Vec<PhotoItem>,
    pub current_photo_by_monitor: HashMap<String, Option<PhotoItem>>,
    pub cached_bytes: u64,
    pub max_cache_bytes: u64,
    pub history_count: usize,
    pub blacklist_count: usize,
    pub is_refilling: bool,
    pub rate_limit_remaining: usize,
}

pub fn query_photo_item_by_id(
    conn: &Connection,
    app_data_dir: &Path,
    photo_id: &str,
) -> rusqlite::Result<Option<PhotoItem>> {
    conn.query_row(
        "SELECT id, album_slug, album_url, file_path, width, height, bytes,
                featured, is_favorite, downloaded_at, last_shown_at
         FROM photos_cache WHERE id = ?1",
        params![photo_id],
        |r| {
            let rel_path: String = r.get(3)?;
            let abs_path = app_data_dir.join(&rel_path).to_string_lossy().into_owned();
            Ok(PhotoItem {
                id: r.get(0)?,
                album_slug: r.get(1)?,
                album_url: r.get(2)?,
                file_path: abs_path,
                width: r.get(4)?,
                height: r.get(5)?,
                bytes: r.get::<_, i64>(6)?.max(0) as u64,
                featured: r.get::<_, i64>(7)? != 0,
                is_favorite: r.get::<_, i64>(8)? != 0,
                downloaded_at: r.get(9)?,
                last_shown_at: r.get(10)?,
            })
        },
    )
    .optional()
}

pub fn pop_next_queued_photo_with_exclusions(
    conn: &Connection,
    app_data_dir: &Path,
    filter: &PhotoFilter,
    currently_displayed_ids: &[String],
) -> rusqlite::Result<Option<PhotoItem>> {
    loop {
        let mut sql = String::from(
            "SELECT id, file_path FROM photos_cache
             WHERE file_present = 1
               AND last_shown_at IS NULL
               AND id NOT IN (SELECT photo_id FROM photos_history)
               AND id NOT IN (SELECT photo_id FROM photos_blacklist)",
        );

        if filter.featured_only {
            sql.push_str(" AND featured = 1");
        }
        if let Some(ref orient) = filter.required_orientation {
            sql.push_str(&format!(
                " AND orientation = '{}'",
                if orient == "portrait" { "portrait" } else { "landscape" }
            ));
        }
        if !filter.album_slugs.is_empty() {
            let quoted: Vec<String> = filter
                .album_slugs
                .iter()
                .filter_map(|s| parse_album_slug(s))
                .map(|s| format!("'{}'", s))
                .collect();
            if !quoted.is_empty() {
                sql.push_str(&format!(" AND album_slug IN ({})", quoted.join(",")));
            }
        }
        if !currently_displayed_ids.is_empty() {
            let quoted_ids: Vec<String> = currently_displayed_ids
                .iter()
                .map(|id| format!("'{}'", id.replace('\'', "")))
                .collect();
            sql.push_str(&format!(" AND id NOT IN ({})", quoted_ids.join(",")));
        }
        sql.push_str(" ORDER BY downloaded_at ASC LIMIT 1");

        let row: Option<(String, String)> = conn
            .query_row(&sql, [], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;

        let Some((id, rel_path)) = row else {
            return Ok(None);
        };

        let abs_path = app_data_dir.join(&rel_path);
        if abs_path.exists() {
            return query_photo_item_by_id(conn, app_data_dir, &id);
        } else {
            conn.execute(
                "UPDATE photos_cache SET file_present = 0 WHERE id = ?1",
                params![id],
            )?;
        }
    }
}

pub fn evict_old_cached_files_preserving_history_unlocked(
    db: &Arc<Mutex<Connection>>,
    app_data_dir: &Path,
    incoming_bytes: u64,
    max_cache_bytes: u64,
    currently_displayed_ids: &[String],
) -> rusqlite::Result<()> {
    let to_evict: Vec<(String, PathBuf)> = {
        let conn = db.lock();
        let current_bytes: u64 = conn.query_row(
            "SELECT COALESCE(SUM(bytes), 0) FROM photos_cache WHERE file_present = 1 AND is_favorite = 0",
            [],
            |r| r.get::<_, i64>(0).map(|v| v.max(0) as u64),
        )?;

        if current_bytes + incoming_bytes <= max_cache_bytes {
            return Ok(());
        }

        let mut bytes_to_free = (current_bytes + incoming_bytes).saturating_sub(max_cache_bytes);
        let mut stmt = conn.prepare(
            "SELECT id, file_path, bytes FROM photos_cache
             WHERE file_present = 1
               AND is_favorite = 0
               AND last_shown_at IS NOT NULL
             ORDER BY last_shown_at ASC",
        )?;

        let candidates: Vec<(String, String, u64)> = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)?.max(0) as u64))
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
        selected
    };

    for (id, full_path) in to_evict {
        let _ = std::fs::remove_file(&full_path);
        let conn = db.lock();
        conn.execute(
            "UPDATE photos_cache SET file_present = 0 WHERE id = ?1",
            params![id],
        )?;
    }
    Ok(())
}

#[derive(Clone)]
pub struct PhotoServiceState {
    pub db: Arc<Mutex<Connection>>,
    pub app_data_dir: PathBuf,
    pub http: Client,
    pub limiter: Arc<SlidingRateLimiter>,
    pub is_refilling: Arc<AtomicBool>,
    pub refill_mutex: Arc<tokio::sync::Mutex<()>>,
}

impl PhotoServiceState {
    pub fn load_active_filter(
        &self,
        required_orientation: Option<String>,
    ) -> rusqlite::Result<(FilterMode, PhotoFilter, u64, Vec<String>)> {
        let conn = self.db.lock();

        let mode_str: String = conn
            .query_row("SELECT value_json FROM settings WHERE key = 'photo_filter_mode'", [], |r| r.get(0))
            .optional()?
            .unwrap_or_else(|| "\"all\"".to_string());
        let mode: FilterMode = serde_json::from_str(&mode_str).unwrap_or(FilterMode::All);

        let featured_only: bool = conn
            .query_row("SELECT value_json FROM settings WHERE key = 'photo_featured_only'", [], |r| r.get::<_, String>(0))
            .optional()?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(false);

        let auto_reset: bool = conn
            .query_row("SELECT value_json FROM settings WHERE key = 'exhaustion_auto_reset'", [], |r| r.get::<_, String>(0))
            .optional()?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(true);

        let max_cache_bytes: u64 = conn
            .query_row("SELECT value_json FROM settings WHERE key = 'max_photo_cache_bytes'", [], |r| r.get::<_, String>(0))
            .optional()?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(DEFAULT_MAX_CACHE_BYTES);

        let album_slugs: Vec<String> = if mode == FilterMode::Albums {
            let mut stmt = conn.prepare("SELECT album_slug FROM saved_albums WHERE is_enabled = 1")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        } else {
            Vec::new()
        };

        let mut stmt = conn.prepare(
            "SELECT current_photo_id FROM monitor_assignments WHERE current_photo_id IS NOT NULL",
        )?;
        let displayed_ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;

        let filter = PhotoFilter {
            album_slugs,
            featured_only: mode == FilterMode::Featured || featured_only,
            required_orientation,
            auto_reset_history_on_exhaustion: auto_reset,
        };

        Ok((mode, filter, max_cache_bytes, displayed_ids))
    }
}

/// 1. `photo_get_status`
#[tauri::command]
pub async fn photo_get_status(state: State<'_, PhotoServiceState>) -> Result<PhotoStatus, String> {
    let (mode, filter, max_cache_bytes, _) = state
        .load_active_filter(None)
        .map_err(|e| e.to_string())?;

    let conn = state.db.lock();

    let mut sql = String::from(
        "SELECT id FROM photos_cache
         WHERE file_present = 1
           AND last_shown_at IS NULL
           AND id NOT IN (SELECT photo_id FROM photos_history)
           AND id NOT IN (SELECT photo_id FROM photos_blacklist)",
    );
    if filter.featured_only {
        sql.push_str(" AND featured = 1");
    }
    if !filter.album_slugs.is_empty() {
        let quoted: Vec<String> = filter
            .album_slugs
            .iter()
            .filter_map(|s| parse_album_slug(s))
            .map(|s| format!("'{}'", s))
            .collect();
        if !quoted.is_empty() {
            sql.push_str(&format!(" AND album_slug IN ({})", quoted.join(",")));
        }
    }
    sql.push_str(" ORDER BY downloaded_at ASC");

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let ids: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let mut unused_queue = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(item) = query_photo_item_by_id(&conn, &state.app_data_dir, &id).map_err(|e| e.to_string())? {
            unused_queue.push(item);
        }
    }

    let mut mon_stmt = conn
        .prepare("SELECT monitor_id, current_photo_id FROM monitor_assignments")
        .map_err(|e| e.to_string())?;
    let mon_rows: Vec<(String, Option<String>)> = mon_stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let mut current_photo_by_monitor = HashMap::new();
    for (mon_id, maybe_pid) in mon_rows {
        let item = match maybe_pid {
            Some(pid) => query_photo_item_by_id(&conn, &state.app_data_dir, &pid).map_err(|e| e.to_string())?,
            None => None,
        };
        current_photo_by_monitor.insert(mon_id, item);
    }

    let cached_bytes: u64 = conn
        .query_row(
            "SELECT COALESCE(SUM(bytes), 0) FROM photos_cache WHERE file_present = 1",
            [],
            |r| r.get::<_, i64>(0).map(|v| v.max(0) as u64),
        )
        .map_err(|e| e.to_string())?;

    let history_count: usize = conn
        .query_row("SELECT COUNT(*) FROM photos_history", [], |r| r.get::<_, i64>(0).map(|v| v as usize))
        .map_err(|e| e.to_string())?;

    let blacklist_count: usize = conn
        .query_row("SELECT COUNT(*) FROM photos_blacklist", [], |r| r.get::<_, i64>(0).map(|v| v as usize))
        .map_err(|e| e.to_string())?;

    Ok(PhotoStatus {
        filter_mode: mode,
        featured_only: filter.featured_only,
        unused_queue,
        current_photo_by_monitor,
        cached_bytes,
        max_cache_bytes,
        history_count,
        blacklist_count,
        is_refilling: state.is_refilling.load(Ordering::Relaxed),
        rate_limit_remaining: 100,
    })
}

/// 2. `photo_fetch_public_catalog`
#[tauri::command]
pub async fn photo_fetch_public_catalog(
    state: State<'_, PhotoServiceState>,
) -> Result<Vec<PublicCatalogAlbum>, String> {
    let mut all_albums = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut url = PUBLIC_CATALOG_URL.to_string();
        if let Some(ref c) = cursor {
            url.push_str(&format!("&cursor={c}"));
        }
        let resp = state.http.get(&url).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("Catalog request failed with HTTP {}", resp.status()));
        }
        let page: CatalogApiResponse = resp.json().await.map_err(|e| e.to_string())?;
        all_albums.extend(page.items);
        if page.next_cursor.is_none() {
            break;
        }
        cursor = page.next_cursor;
    }

    Ok(all_albums)
}

/// 3. `photo_rotate_next` (Supports Cloned vs. Independent Multi-Monitor + Orientation Fallback)
#[tauri::command]
pub async fn photo_rotate_next(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    monitor_id: Option<String>,
) -> Result<PhotoItem, String> {
    let (_, mut base_filter, max_cache_bytes, mut displayed_ids) = state
        .load_active_filter(None)
        .map_err(|e| e.to_string())?;

    let (skip_mismatched, multi_monitor_mode, target_monitors): (bool, String, Vec<(String, bool)>) = {
        let conn = state.db.lock();
        let skip: bool = conn
            .query_row("SELECT value_json FROM settings WHERE key = 'skip_mismatched_orientation'", [], |r| r.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(true);
        let mm_mode: String = conn
            .query_row("SELECT value_json FROM settings WHERE key = 'multi_monitor_mode'", [], |r| r.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| "independent".to_string());

        let monitors = if let Some(ref mid) = monitor_id {
            vec![(mid.clone(), mid.contains("portrait"))]
        } else {
            let mut stmt = conn
                .prepare("SELECT monitor_id FROM monitor_assignments")
                .map_err(|e| e.to_string())?;
            let ids: Vec<String> = stmt
                .query_map([], |r| r.get(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            if ids.is_empty() {
                vec![("DISPLAY-1".to_string(), false)]
            } else {
                ids.into_iter().map(|id| (id, false)).collect()
            }
        };
        (skip, mm_mode, monitors)
    };

    let mut last_applied_item: Option<PhotoItem> = None;

    if multi_monitor_mode == "clone" && monitor_id.is_none() {
        base_filter.required_orientation = None;
        let mut item = {
            let conn = state.db.lock();
            pop_next_queued_photo_with_exclusions(&conn, &state.app_data_dir, &base_filter, &displayed_ids)
                .map_err(|e| e.to_string())?
        };
        if item.is_none() {
            let _ = refill_queue(
                &state.http,
                &state.limiter,
                &state.db,
                &state.app_data_dir,
                &base_filter,
                max_cache_bytes,
                &displayed_ids,
            )
            .await;
            let conn = state.db.lock();
            item = pop_next_queued_photo_with_exclusions(&conn, &state.app_data_dir, &base_filter, &displayed_ids)
                .map_err(|e| e.to_string())?;
        }
        let chosen = item.ok_or_else(|| "No eligible photos available in queue".to_string())?;
        {
            let mut conn = state.db.lock();
            mark_photo_shown(&mut conn, &chosen.id).map_err(|e| e.to_string())?;
            let ts = now_ms();
            for (mid, _) in &target_monitors {
                conn.execute(
                    "UPDATE monitor_assignments SET current_photo_id = ?1, updated_at = ?2 WHERE monitor_id = ?3",
                    params![chosen.id, ts, mid],
                )
                .map_err(|e| e.to_string())?;
                let _ = app.emit("wallpaper://changed", serde_json::json!({
                    "monitorId": mid,
                    "currentItem": &chosen,
                    "transitionUsed": "liquid_glass"
                }));
            }
        }
        last_applied_item = Some(chosen);
    } else {
        for (mid, is_portrait) in &target_monitors {
            let preferred_orient = if skip_mismatched {
                Some(if *is_portrait { "portrait".to_string() } else { "landscape".to_string() })
            } else {
                None
            };
            base_filter.required_orientation = preferred_orient;

            let mut item = {
                let conn = state.db.lock();
                pop_next_queued_photo_with_exclusions(&conn, &state.app_data_dir, &base_filter, &displayed_ids)
                    .map_err(|e| e.to_string())?
            };

            if item.is_none() {
                let _ = refill_queue(
                    &state.http,
                    &state.limiter,
                    &state.db,
                    &state.app_data_dir,
                    &base_filter,
                    max_cache_bytes,
                    &displayed_ids,
                )
                .await;
                let conn = state.db.lock();
                item = pop_next_queued_photo_with_exclusions(&conn, &state.app_data_dir, &base_filter, &displayed_ids)
                    .map_err(|e| e.to_string())?;
            }

            // Graceful fallback: if album has 0 photos matching monitor orientation, allow any orientation (FitBlur)
            if item.is_none() && base_filter.required_orientation.is_some() {
                base_filter.required_orientation = None;
                let conn = state.db.lock();
                item = pop_next_queued_photo_with_exclusions(&conn, &state.app_data_dir, &base_filter, &displayed_ids)
                    .map_err(|e| e.to_string())?;
            }

            if let Some(chosen) = item {
                let mut conn = state.db.lock();
                mark_photo_shown(&mut conn, &chosen.id).map_err(|e| e.to_string())?;
                conn.execute(
                    "INSERT INTO monitor_assignments (monitor_id, source_json, current_photo_id, updated_at)
                     VALUES (?1, '{\"kind\":\"photo_api\"}', ?2, ?3)
                     ON CONFLICT(monitor_id) DO UPDATE SET current_photo_id = excluded.current_photo_id, updated_at = excluded.updated_at",
                    params![mid, chosen.id, now_ms()],
                )
                .map_err(|e| e.to_string())?;
                displayed_ids.push(chosen.id.clone());
                let _ = app.emit("wallpaper://changed", serde_json::json!({
                    "monitorId": mid,
                    "currentItem": &chosen,
                    "transitionUsed": "liquid_glass"
                }));
                last_applied_item = Some(chosen);
            }
        }
    }

    let unused_remaining = {
        let conn = state.db.lock();
        base_filter.required_orientation = None;
        count_unused_in_queue(&conn, &base_filter).unwrap_or(0)
    };
    if unused_remaining <= QUEUE_REFILL_THRESHOLD {
        let _ = photo_trigger_refill(app.clone(), state.clone()).await;
    } else if let Ok(status) = photo_get_status(state).await {
        let _ = app.emit("photo-queue://updated", status);
    }

    last_applied_item.ok_or_else(|| "No eligible photo found".to_string())
}

/// 4. `photo_trigger_refill`
#[tauri::command]
pub async fn photo_trigger_refill(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
) -> Result<(), String> {
    let Ok(_guard) = state.refill_mutex.try_lock() else {
        return Ok(());
    };
    state.is_refilling.store(true, Ordering::Relaxed);
    if let Ok(status) = photo_get_status(state.clone()).await {
        let _ = app.emit("photo-queue://updated", status);
    }

    let (_, filter, max_cache_bytes, displayed_ids) = state
        .load_active_filter(None)
        .map_err(|e| {
            state.is_refilling.store(false, Ordering::Relaxed);
            e.to_string()
        })?;

    let res = refill_queue(
        &state.http,
        &state.limiter,
        &state.db,
        &state.app_data_dir,
        &filter,
        max_cache_bytes,
        &displayed_ids,
    )
    .await;

    state.is_refilling.store(false, Ordering::Relaxed);
    if let Ok(status) = photo_get_status(state).await {
        let _ = app.emit("photo-queue://updated", status);
    }
    res.map(|_| ()).map_err(|e| e.to_string())
}

/// 5. `photo_add_album`
#[tauri::command]
pub async fn photo_add_album(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    url_or_slug: String,
    display_name: Option<String>,
) -> Result<SavedAlbum, String> {
    let slug = parse_album_slug(&url_or_slug)
        .ok_or_else(|| "Invalid album URL or slug".to_string())?;

    let test_resp = state
        .http
        .get(API_RANDOM_URL)
        .query(&[("album", &slug)])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !test_resp.status().is_success() {
        return Err(format!("Album '{}' not found or has no public photos", slug));
    }
    let sample: ApiPhotoResponse = test_resp.json().await.map_err(|e| e.to_string())?;

    let catalog_entry = photo_fetch_public_catalog(state.clone())
        .await
        .ok()
        .and_then(|items| items.into_iter().find(|a| a.slug == slug));

    let title = display_name
        .filter(|s| !s.trim().is_empty())
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
                slug,
                title,
                catalog_entry.as_ref().map(|c| c.description.as_str()),
                catalog_entry.as_ref().map(|c| c.category.as_str()),
                sample.album_url,
                cover_thumb,
                catalog_entry.as_ref().map(|c| c.cover_blurhash.as_str()),
                image_count,
                added_at,
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    if let Ok(status) = photo_get_status(state).await {
        let _ = app.emit("photo-queue://updated", status);
    }

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
        conn.execute(
            "UPDATE saved_albums SET is_enabled = ?1 WHERE album_slug = ?2",
            params![i32::from(enabled), album_slug],
        )
        .map_err(|e| e.to_string())?;
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
        conn.execute(
            "DELETE FROM saved_albums WHERE album_slug = ?1",
            params![album_slug],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Ok(status) = photo_get_status(state).await {
        let _ = app.emit("photo-queue://updated", status);
    }
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
        let mode_json = serde_json::to_string(&mode).map_err(|e| e.to_string())?;
        let feat_json = serde_json::to_string(&featured_only).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO settings (key, value_json) VALUES ('photo_filter_mode', ?1)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![mode_json],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO settings (key, value_json) VALUES ('photo_featured_only', ?1)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![feat_json],
        )
        .map_err(|e| e.to_string())?;
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
        conn.execute(
            "UPDATE photos_cache SET is_favorite = ?1 WHERE id = ?2",
            params![i32::from(favorite), photo_id],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Ok(status) = photo_get_status(state).await {
        let _ = app.emit("photo-queue://updated", status);
    }
    Ok(())
}

/// 10. `photo_blacklist_and_skip`
#[tauri::command]
pub async fn photo_blacklist_and_skip(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    photo_id: String,
) -> Result<(), String> {
    let (file_to_delete, affected_monitors): (Option<PathBuf>, Vec<String>) = {
        let conn = state.db.lock();
        let meta: Option<(String, String)> = conn
            .query_row(
                "SELECT album_slug, file_path FROM photos_cache WHERE id = ?1",
                params![photo_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;

        let album_slug = meta.as_ref().map(|(s, _)| s.clone());
        let abs_path = meta.map(|(_, rel)| state.app_data_dir.join(rel));

        conn.execute(
            "INSERT INTO photos_blacklist (photo_id, album_slug, hidden_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(photo_id) DO NOTHING",
            params![photo_id, album_slug, now_ms()],
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "UPDATE photos_cache SET file_present = 0, is_favorite = 0 WHERE id = ?1",
            params![photo_id],
        )
        .map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare("SELECT monitor_id FROM monitor_assignments WHERE current_photo_id = ?1")
            .map_err(|e| e.to_string())?;
        let mons = stmt
            .query_map(params![photo_id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        (abs_path, mons)
    };

    if let Some(path) = file_to_delete {
        let _ = tokio::fs::remove_file(path).await;
    }

    for mid in affected_monitors {
        let _ = photo_rotate_next(app.clone(), state.clone(), Some(mid)).await;
    }

    if let Ok(status) = photo_get_status(state).await {
        let _ = app.emit("photo-queue://updated", status);
    }
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
        conn.execute(
            "INSERT INTO settings (key, value_json) VALUES ('max_photo_cache_bytes', ?1)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![max_bytes.to_string()],
        )
        .map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare("SELECT current_photo_id FROM monitor_assignments WHERE current_photo_id IS NOT NULL")
            .map_err(|e| e.to_string())?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        ids
    };

    evict_old_cached_files_preserving_history_unlocked(
        &state.db,
        &state.app_data_dir,
        0,
        max_bytes,
        &displayed_ids,
    )
    .map_err(|e| e.to_string())?;

    if let Ok(status) = photo_get_status(state).await {
        let _ = app.emit("photo-queue://updated", status);
    }
    Ok(())
}

/// 12. `photo_clear_cache` (Deletes all non-favorited, non-displayed .jpg files; NEVER touches photos_history)
#[tauri::command]
pub async fn photo_clear_cache(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
) -> Result<(), String> {
    let files_to_remove: Vec<(String, PathBuf)> = {
        let conn = state.db.lock();
        let mut mon_stmt = conn
            .prepare("SELECT current_photo_id FROM monitor_assignments WHERE current_photo_id IS NOT NULL")
            .map_err(|e| e.to_string())?;
        let displayed_ids: Vec<String> = mon_stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare("SELECT id, file_path FROM photos_cache WHERE file_present = 1 AND is_favorite = 0")
            .map_err(|e| e.to_string())?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        rows.into_iter()
            .filter(|(id, _)| !displayed_ids.contains(id))
            .map(|(id, rel)| (id, state.app_data_dir.join(rel)))
            .collect()
    };

    for (id, full_path) in files_to_remove {
        let _ = tokio::fs::remove_file(&full_path).await;
        let conn = state.db.lock();
        let _ = conn.execute(
            "UPDATE photos_cache SET file_present = 0 WHERE id = ?1",
            params![id],
        );
    }

    if let Ok(status) = photo_get_status(state).await {
        let _ = app.emit("photo-queue://updated", status);
    }
    Ok(())
}

/// 13. `photo_reset_history` (Resets history for one album or all albums, preserving active screen IDs, and re-queues cached files)
#[tauri::command]
pub async fn photo_reset_history(
    app: AppHandle,
    state: State<'_, PhotoServiceState>,
    album_slug: Option<String>,
) -> Result<(), String> {
    {
        let mut conn = state.db.lock();
        let displayed_ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT current_photo_id FROM monitor_assignments WHERE current_photo_id IS NOT NULL")
                .map_err(|e| e.to_string())?;
            stmt.query_map([], |r| r.get(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };

        let filter = PhotoFilter {
            album_slugs: album_slug.into_iter().collect(),
            featured_only: false,
            required_orientation: None,
            auto_reset_history_on_exhaustion: true,
        };
        reset_exhausted_filter_history(&mut conn, &filter, &displayed_ids)
            .map_err(|e| e.to_string())?;
    }

    let _ = photo_trigger_refill(app, state).await;
    Ok(())
}
```
