//! Offline photo queue refill (docs/02 Sections 2 and 4, step 5).
//!
//! The queue keeps [`QUEUE_TARGET_UNUSED`] unused photos on disk so rotation needs zero API
//! calls. A refill is triggered when the unused count drops to [`QUEUE_REFILL_THRESHOLD`] or
//! below, makes at most [`MAX_REFILL_ATTEMPTS`] random-photo requests, and shares one
//! [`SlidingRateLimiter`] between JSON calls and image downloads.

use crate::client::{is_allowed_image_url, PhotoApiClient, PhotoTransport, RandomPhotoQuery};
use crate::decoder::inspect_display_dimensions;
use crate::error::{PhotoApiError, PhotoApiResult};
use crate::limiter::SlidingRateLimiter;
use crate::{
    ApiPhotoResponse, MAX_REFILL_ATTEMPTS, QUEUE_REFILL_THRESHOLD, QUEUE_TARGET_UNUSED,
    SMALL_ALBUM_EXIT_CONSECUTIVE_DUPS,
};
use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Arc;
use truongpaper_core::{
    classify_candidate, count_unused_in_queue, evict_old_cached_files_preserving_history_unlocked,
    now_ms, parse_album_slug, reset_exhausted_filter_history, upsert_cached_photo, CandidateStatus,
    NewCachedPhoto, PhotoFilter,
};

/// Largest image the refill loop will accept (the API serves originals around 10-30 MB).
pub const MAX_IMAGE_BYTES: usize = 128 * 1024 * 1024;
/// Give up after this many API rejections (400 / 404 / unusable metadata) in one refill.
pub const MAX_API_REJECTIONS: usize = 4;

/// Why a refill call stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefillOutcome {
    /// Unused queue holds at least [`QUEUE_TARGET_UNUSED`] photos.
    TargetReached { unused_count: usize },
    /// A small album: every photo the API can return is already queued or on screen.
    SmallPoolFullyQueued { unused_count: usize },
    /// Every eligible photo was already shown; history was reset and cached files requeued.
    FilterExhaustedAndReset { unused_count: usize },
    /// Every eligible photo was already shown and auto-reset is disabled.
    FilterExhaustedHoldingLast,
    /// Local budget spent or the server asked us to back off.
    RateLimitedOrBackedOff,
    /// A network error ended the refill; the limiter is cooling down and flagged offline.
    Offline,
    /// [`MAX_REFILL_ATTEMPTS`] (or [`MAX_API_REJECTIONS`]) reached without a clear verdict.
    RetryCapReached { unused_count: usize },
}

/// True when the unused count is low enough to start a background refill.
pub fn needs_refill(unused_count: usize) -> bool {
    unused_count <= QUEUE_REFILL_THRESHOLD
}

/// Checks API metadata against the local database BEFORE `imageUrl` is downloaded.
pub fn classify_candidate_photo(
    conn: &Connection,
    resp: &ApiPhotoResponse,
    required_orientation: Option<&str>,
) -> PhotoApiResult<CandidateStatus> {
    classify_candidate_photo_with_displayed(conn, resp, required_orientation, &[])
}

/// [`classify_candidate_photo`] that also treats photos currently on screen as queued.
pub fn classify_candidate_photo_with_displayed(
    conn: &Connection,
    resp: &ApiPhotoResponse,
    required_orientation: Option<&str>,
    currently_displayed_ids: &[String],
) -> PhotoApiResult<CandidateStatus> {
    Ok(classify_candidate(
        conn,
        &resp.id,
        resp.width,
        resp.height,
        required_orientation,
        currently_displayed_ids,
    )?)
}

/// Photo ids become file names, so only a conservative character set is accepted.
pub fn is_safe_photo_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn unused_count(db: &Arc<Mutex<Connection>>, filter: &PhotoFilter) -> PhotoApiResult<usize> {
    let conn = db.lock();
    Ok(count_unused_in_queue(&conn, filter)?)
}

fn classify(
    db: &Arc<Mutex<Connection>>,
    meta: &ApiPhotoResponse,
    filter: &PhotoFilter,
    displayed: &[String],
) -> PhotoApiResult<CandidateStatus> {
    let conn = db.lock();
    classify_candidate_photo_with_displayed(
        &conn,
        meta,
        filter.required_orientation.as_deref(),
        displayed,
    )
}

fn reset_history(
    db: &Arc<Mutex<Connection>>,
    filter: &PhotoFilter,
    displayed: &[String],
) -> PhotoApiResult<usize> {
    let mut conn = db.lock();
    reset_exhausted_filter_history(&mut conn, filter, displayed)?;
    Ok(count_unused_in_queue(&conn, filter)?)
}

fn insert_cached(db: &Arc<Mutex<Connection>>, photo: &NewCachedPhoto) -> PhotoApiResult<()> {
    let conn = db.lock();
    Ok(upsert_cached_photo(&conn, photo)?)
}

/// Background refill with a `PhotoTransport` (production: `reqwest::Client`).
#[allow(clippy::too_many_arguments)]
pub async fn refill_queue<T: PhotoTransport>(
    transport: &T,
    limiter: &SlidingRateLimiter,
    db: &Arc<Mutex<Connection>>,
    app_data_dir: &Path,
    filter: &PhotoFilter,
    max_cache_bytes: u64,
    currently_displayed_ids: &[String],
) -> PhotoApiResult<RefillOutcome> {
    let client = PhotoApiClient::new(transport);
    refill_queue_with_client(
        &client,
        limiter,
        db,
        app_data_dir,
        filter,
        max_cache_bytes,
        currently_displayed_ids,
    )
    .await
}

/// Refill loop against an explicit [`PhotoApiClient`] (custom endpoints / tests).
#[allow(clippy::too_many_arguments)]
pub async fn refill_queue_with_client<T: PhotoTransport>(
    client: &PhotoApiClient<T>,
    limiter: &SlidingRateLimiter,
    db: &Arc<Mutex<Connection>>,
    app_data_dir: &Path,
    filter: &PhotoFilter,
    max_cache_bytes: u64,
    currently_displayed_ids: &[String],
) -> PhotoApiResult<RefillOutcome> {
    let slugs: Vec<String> = filter
        .album_slugs
        .iter()
        .filter_map(|s| parse_album_slug(s))
        .collect();
    if !filter.album_slugs.is_empty() && slugs.is_empty() {
        return Err(PhotoApiError::InvalidFilter(
            "no valid album slug in filter".into(),
        ));
    }

    let photos_dir = app_data_dir.join("wallpapers").join("photos");
    tokio::fs::create_dir_all(&photos_dir).await?;

    let mut attempts = 0_usize;
    let mut consecutive_queue_dups = 0_usize;
    let mut queue_dups_total = 0_usize;
    let mut history_dups = 0_usize;
    let mut api_rejections = 0_usize;

    loop {
        let unused = unused_count(db, filter)?;
        if unused >= QUEUE_TARGET_UNUSED {
            return Ok(RefillOutcome::TargetReached {
                unused_count: unused,
            });
        }
        if attempts >= MAX_REFILL_ATTEMPTS {
            // Case B: everything eligible was already shown.
            if history_dups > 0 && unused == 0 {
                if filter.auto_reset_history_on_exhaustion {
                    let after = reset_history(db, filter, currently_displayed_ids)?;
                    return Ok(RefillOutcome::FilterExhaustedAndReset {
                        unused_count: after,
                    });
                }
                return Ok(RefillOutcome::FilterExhaustedHoldingLast);
            }
            // Case A: only queue / on-screen duplicates; resetting history would do nothing.
            if history_dups == 0 && queue_dups_total > 0 {
                return Ok(RefillOutcome::SmallPoolFullyQueued {
                    unused_count: unused,
                });
            }
            return Ok(RefillOutcome::RetryCapReached {
                unused_count: unused,
            });
        }
        if api_rejections >= MAX_API_REJECTIONS {
            return Ok(RefillOutcome::RetryCapReached {
                unused_count: unused,
            });
        }
        if !limiter.try_acquire() {
            return Ok(RefillOutcome::RateLimitedOrBackedOff);
        }
        attempts += 1;

        let album = if slugs.is_empty() {
            None
        } else {
            Some(slugs[rand::random::<usize>() % slugs.len()].clone())
        };
        let query = RandomPhotoQuery {
            album,
            featured: filter.featured_only,
        };
        let meta = match client.fetch_random(&query).await {
            Ok(meta) => {
                limiter.note_success();
                meta
            }
            Err(e) if e.is_throttle() => {
                limiter.note_throttled_with(e.retry_after());
                return Ok(RefillOutcome::RateLimitedOrBackedOff);
            }
            Err(e) if e.is_network() => {
                tracing::warn!("photo API unreachable: {e}");
                limiter.note_network_error();
                return Ok(RefillOutcome::Offline);
            }
            Err(
                PhotoApiError::NoPhotos
                | PhotoApiError::InvalidParameters
                | PhotoApiError::UnexpectedStatus(_)
                | PhotoApiError::Json(_),
            ) => {
                api_rejections += 1;
                continue;
            }
            Err(e) => return Err(e),
        };

        if !is_safe_photo_id(&meta.id) || !is_allowed_image_url(&meta.image_url) {
            tracing::warn!(id = %meta.id, "ignoring photo with unsafe id or imageUrl outside https://*.iantruongphotography.com");
            api_rejections += 1;
            continue;
        }

        match classify(db, &meta, filter, currently_displayed_ids)? {
            CandidateStatus::AlreadyInUnusedQueue => {
                consecutive_queue_dups += 1;
                queue_dups_total += 1;
                // Small album (< 5 photos): every unseen photo is already queued on disk.
                if consecutive_queue_dups >= SMALL_ALBUM_EXIT_CONSECUTIVE_DUPS && history_dups == 0
                {
                    let unused_now = unused_count(db, filter)?;
                    return Ok(RefillOutcome::SmallPoolFullyQueued {
                        unused_count: unused_now,
                    });
                }
                continue;
            }
            CandidateStatus::AlreadyInHistory => {
                consecutive_queue_dups = 0;
                history_dups += 1;
                continue;
            }
            CandidateStatus::Blacklisted | CandidateStatus::OrientationMismatch => {
                consecutive_queue_dups = 0;
                continue;
            }
            CandidateStatus::UniqueAndUnseen => {
                consecutive_queue_dups = 0;
            }
        }

        let rel_path = format!("wallpapers/photos/{}.jpg", meta.id);
        let final_path = app_data_dir.join(&rel_path);
        let part_path = photos_dir.join(format!("{}.jpg.part", meta.id));

        if !final_path.exists() {
            // Take the token right before the request.
            if !limiter.try_acquire() {
                return Ok(RefillOutcome::RateLimitedOrBackedOff);
            }
            let downloaded = client
                .download_image_to(&meta.image_url, &part_path, MAX_IMAGE_BYTES as u64)
                .await;
            let downloaded_bytes = match downloaded {
                Ok(bytes) => {
                    limiter.note_success();
                    bytes
                }
                Err(e) if e.is_throttle() => {
                    let _ = tokio::fs::remove_file(&part_path).await;
                    limiter.note_throttled_with(e.retry_after());
                    return Ok(RefillOutcome::RateLimitedOrBackedOff);
                }
                Err(e) if e.is_network() => {
                    let _ = tokio::fs::remove_file(&part_path).await;
                    tracing::warn!("photo download failed: {e}");
                    limiter.note_network_error();
                    return Ok(RefillOutcome::Offline);
                }
                // 403 / 404 / oversized / empty: this photo is unusable, the API is fine.
                Err(
                    PhotoApiError::UnexpectedStatus(_)
                    | PhotoApiError::NoPhotos
                    | PhotoApiError::Rejected(_),
                ) => {
                    let _ = tokio::fs::remove_file(&part_path).await;
                    tracing::warn!(id = %meta.id, "rejecting image download");
                    api_rejections += 1;
                    continue;
                }
                Err(e) => {
                    let _ = tokio::fs::remove_file(&part_path).await;
                    return Err(e);
                }
            };

            // Evict only now that the download succeeded, so a failed download never costs
            // the user cached photos.
            if let Err(e) = evict_old_cached_files_preserving_history_unlocked(
                db,
                app_data_dir,
                downloaded_bytes,
                max_cache_bytes,
                currently_displayed_ids,
            ) {
                let _ = tokio::fs::remove_file(&part_path).await;
                return Err(e.into());
            }
            if let Err(e) = tokio::fs::rename(&part_path, &final_path).await {
                let _ = tokio::fs::remove_file(&part_path).await;
                return Err(e.into());
            }
        }

        // EXIF-aware display size (axes swap for orientations 5-8), then API metadata, then
        // raw header size, then a 4K default.
        let inspect_path = final_path.clone();
        let inspected =
            tokio::task::spawn_blocking(move || inspect_display_dimensions(&inspect_path))
                .await
                .ok()
                .flatten();
        let (width, height) = inspected
            .or(match (meta.width, meta.height) {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                _ => None,
            })
            .or_else(|| {
                imagesize::size(&final_path)
                    .ok()
                    .map(|d| (d.width as u32, d.height as u32))
            })
            .unwrap_or((3840, 2160));

        let on_disk_bytes = tokio::fs::metadata(&final_path)
            .await
            .map(|m| m.len())
            .unwrap_or(meta.bytes);

        insert_cached(
            db,
            &NewCachedPhoto {
                id: meta.id.clone(),
                rel_path,
                album_slug: meta.album_slug.clone(),
                album_url: meta.album_url.clone(),
                featured: meta.featured,
                fallback_used: meta.fallback_used,
                width,
                height,
                bytes: on_disk_bytes,
                downloaded_at: now_ms(),
            },
        )?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{FnTransport, HttpResponse};
    use crate::decoder::test_util::{make_jpeg, with_exif_orientation};
    use crate::{API_RANDOM_URL, RATE_LIMIT_WINDOW};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use truongpaper_core::{
        get_cached_photo, get_history_entry, history_count, mark_file_evicted, mark_photo_shown,
        SQLITE_SCHEMA_SQL,
    };

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "truongpaper-photoapi-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn new_db() -> Arc<Mutex<Connection>> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SQLITE_SCHEMA_SQL).unwrap();
        Arc::new(Mutex::new(conn))
    }

    fn filter(albums: &[&str]) -> PhotoFilter {
        PhotoFilter {
            album_slugs: albums.iter().map(|s| s.to_string()).collect(),
            featured_only: false,
            required_orientation: None,
            auto_reset_history_on_exhaustion: true,
        }
    }

    fn meta_json(id: &str, slug: &str, w: u32, h: u32, bytes: u64) -> String {
        format!(
            r#"{{"albumSlug":"{slug}","albumUrl":"https://iantruongphotography.com/album/{slug}",
            "bytes":{bytes},"fallbackUsed":false,"featured":false,"height":{h},"id":"{id}",
            "imageUrl":"https://photos-api.iantruongphotography.com/v1/images/{id}","width":{w}}}"#
        )
    }

    #[derive(Default)]
    struct Counters {
        json: AtomicUsize,
        images: AtomicUsize,
        queries: Mutex<Vec<Vec<(String, String)>>>,
    }

    /// Fake API: `script(n)` answers the n-th `/v1/photos/random` call; every image download
    /// returns `image`.
    fn fake<S>(script: S, image: Vec<u8>) -> (impl PhotoTransport, Arc<Counters>)
    where
        S: Fn(usize) -> HttpResponse + Send + Sync + 'static,
    {
        let counters = Arc::new(Counters::default());
        let c = counters.clone();
        let t = FnTransport(move |url: &str, q: &[(&str, &str)]| {
            if url == API_RANDOM_URL {
                c.queries.lock().push(
                    q.iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                );
                let n = c.json.fetch_add(1, Ordering::SeqCst);
                Ok(script(n))
            } else {
                c.images.fetch_add(1, Ordering::SeqCst);
                Ok(HttpResponse {
                    status: 200,
                    body: image.clone(),
                    retry_after: None,
                })
            }
        });
        (t, counters)
    }

    fn seed_shown(db: &Arc<Mutex<Connection>>, dir: &Path, ids: &[&str], slug: &str, bytes: u64) {
        std::fs::create_dir_all(dir.join("wallpapers/photos")).unwrap();
        let mut conn = db.lock();
        for (i, id) in ids.iter().enumerate() {
            upsert_cached_photo(
                &conn,
                &NewCachedPhoto {
                    id: id.to_string(),
                    rel_path: format!("wallpapers/photos/{id}.jpg"),
                    album_slug: slug.into(),
                    album_url: format!("https://iantruongphotography.com/album/{slug}"),
                    featured: false,
                    fallback_used: false,
                    width: 100,
                    height: 50,
                    bytes,
                    downloaded_at: i as i64,
                },
            )
            .unwrap();
            std::fs::write(
                dir.join(format!("wallpapers/photos/{id}.jpg")),
                vec![0u8; bytes as usize],
            )
            .unwrap();
            mark_photo_shown(&mut conn, id).unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn assert_send<T: Send>(_: &T) {}

    #[tokio::test]
    async fn fills_queue_to_target_with_exif_aware_orientation() {
        let dir = TempDir::new();
        let db = new_db();
        let landscape = make_jpeg(64, 48);
        let rotated = with_exif_orientation(&landscape, 6);
        let (t, counters) = fake(
            |n| HttpResponse::json(200, &meta_json(&format!("p{n}"), "alpha", 64, 48, 1000)),
            rotated,
        );
        let limiter = SlidingRateLimiter::new();

        let all = filter(&[]);
        let fut = refill_queue(&t, &limiter, &db, &dir.0, &all, 1 << 30, &[]);
        assert_send(&fut);
        let outcome = fut.await.unwrap();

        assert_eq!(outcome, RefillOutcome::TargetReached { unused_count: 5 });
        assert_eq!(counters.json.load(Ordering::SeqCst), 5);
        assert_eq!(counters.images.load(Ordering::SeqCst), 5);
        assert_eq!(
            limiter.remaining(),
            90 - 10,
            "JSON calls and downloads share the budget"
        );
        assert!(
            counters.queries.lock().iter().all(|q| q.is_empty()),
            "no-params request"
        );

        let conn = db.lock();
        let row = get_cached_photo(&conn, "p0").unwrap().unwrap();
        // API metadata said 64x48 but EXIF rotates the sensor buffer: stored as portrait.
        assert_eq!((row.width, row.height), (48, 64));
        assert_eq!(row.orientation, "portrait");
        assert!(row.file_present && row.last_shown_at.is_none());
        assert!(dir.0.join("wallpapers/photos/p0.jpg").exists());
        assert!(!dir.0.join("wallpapers/photos/p0.jpg.part").exists());
    }

    #[tokio::test]
    async fn sends_album_and_featured_parameters() {
        let dir = TempDir::new();
        let db = new_db();
        let (t, counters) = fake(
            |n| HttpResponse::json(200, &meta_json(&format!("q{n}"), "alpha-1", 64, 48, 10)),
            make_jpeg(8, 8),
        );
        let mut f = filter(&["https://iantruongphotography.com/album/Alpha-1"]);
        f.featured_only = true;
        let limiter = SlidingRateLimiter::new();
        refill_queue(&t, &limiter, &db, &dir.0, &f, 1 << 30, &[])
            .await
            .unwrap();
        let q = counters.queries.lock()[0].clone();
        assert_eq!(
            q,
            vec![
                ("album".into(), "alpha-1".into()),
                ("featured".into(), "true".into())
            ]
        );
    }

    #[tokio::test]
    async fn no_requests_when_queue_already_full() {
        let dir = TempDir::new();
        let db = new_db();
        {
            let conn = db.lock();
            std::fs::create_dir_all(dir.0.join("wallpapers/photos")).unwrap();
            for i in 0..5 {
                let id = format!("x{i}");
                std::fs::write(dir.0.join(format!("wallpapers/photos/{id}.jpg")), b"x").unwrap();
                upsert_cached_photo(
                    &conn,
                    &NewCachedPhoto {
                        id: id.clone(),
                        rel_path: format!("wallpapers/photos/{id}.jpg"),
                        album_slug: "a".into(),
                        album_url: "u".into(),
                        featured: false,
                        fallback_used: false,
                        width: 2,
                        height: 1,
                        bytes: 1,
                        downloaded_at: i,
                    },
                )
                .unwrap();
            }
        }
        let (t, counters) = fake(|_| HttpResponse::json(500, ""), vec![]);
        let out = refill_queue(
            &t,
            &SlidingRateLimiter::new(),
            &db,
            &dir.0,
            &filter(&[]),
            1 << 30,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(out, RefillOutcome::TargetReached { unused_count: 5 });
        assert_eq!(counters.json.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn small_album_exits_after_four_consecutive_queue_duplicates() {
        let dir = TempDir::new();
        let db = new_db();
        // Album with exactly two photos: a, b, then a/b forever.
        let (t, counters) = fake(
            |n| {
                let id = if n % 2 == 0 { "a" } else { "b" };
                HttpResponse::json(200, &meta_json(id, "tiny-1", 64, 48, 100))
            },
            make_jpeg(64, 48),
        );
        let limiter = SlidingRateLimiter::new();
        let out = refill_queue(
            &t,
            &limiter,
            &db,
            &dir.0,
            &filter(&["tiny-1"]),
            1 << 30,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(out, RefillOutcome::SmallPoolFullyQueued { unused_count: 2 });
        // 2 unique downloads + 4 consecutive duplicates; far fewer than 12 attempts.
        assert_eq!(counters.json.load(Ordering::SeqCst), 6);
        assert_eq!(counters.images.load(Ordering::SeqCst), 2);
        assert_eq!(
            history_count(&db.lock()).unwrap(),
            0,
            "history must not be reset"
        );
    }

    #[tokio::test]
    async fn single_photo_album_on_screen_is_not_downloaded_or_reset() {
        let dir = TempDir::new();
        let db = new_db();
        seed_shown(&db, &dir.0, &["only"], "solo-1", 10);
        let (t, counters) = fake(
            |_| HttpResponse::json(200, &meta_json("only", "solo-1", 100, 50, 10)),
            make_jpeg(8, 8),
        );
        let displayed = vec!["only".to_string()];
        let out = refill_queue(
            &t,
            &SlidingRateLimiter::new(),
            &db,
            &dir.0,
            &filter(&["solo-1"]),
            1 << 30,
            &displayed,
        )
        .await
        .unwrap();
        // The on-screen photo counts as queued: 4 duplicates, no history reset, no download.
        assert_eq!(out, RefillOutcome::SmallPoolFullyQueued { unused_count: 0 });
        assert_eq!(counters.images.load(Ordering::SeqCst), 0);
        assert_eq!(history_count(&db.lock()).unwrap(), 1);
    }

    #[tokio::test]
    async fn exhausted_filter_resets_history_except_on_screen_photo() {
        let dir = TempDir::new();
        let db = new_db();
        seed_shown(&db, &dir.0, &["a", "b", "c"], "trio-1", 10);
        let (t, counters) = fake(
            |n| {
                let id = ["a", "b", "c"][n % 3];
                HttpResponse::json(200, &meta_json(id, "trio-1", 100, 50, 10))
            },
            make_jpeg(8, 8),
        );
        let displayed = vec!["c".to_string()];
        let out = refill_queue(
            &t,
            &SlidingRateLimiter::new(),
            &db,
            &dir.0,
            &filter(&["trio-1"]),
            1 << 30,
            &displayed,
        )
        .await
        .unwrap();
        assert_eq!(
            out,
            RefillOutcome::FilterExhaustedAndReset { unused_count: 2 }
        );
        assert_eq!(
            counters.json.load(Ordering::SeqCst),
            12,
            "uses the full 12 attempts"
        );
        assert_eq!(
            counters.images.load(Ordering::SeqCst),
            0,
            "zero downloads: cached files requeued"
        );
        let conn = db.lock();
        assert!(get_history_entry(&conn, "a").unwrap().is_none());
        assert!(get_history_entry(&conn, "c").unwrap().is_some());
    }

    #[tokio::test]
    async fn exhausted_filter_holds_when_auto_reset_disabled() {
        let dir = TempDir::new();
        let db = new_db();
        seed_shown(&db, &dir.0, &["a", "b"], "duo-1", 10);
        let (t, _) = fake(
            |n| HttpResponse::json(200, &meta_json(["a", "b"][n % 2], "duo-1", 100, 50, 10)),
            make_jpeg(8, 8),
        );
        let mut f = filter(&["duo-1"]);
        f.auto_reset_history_on_exhaustion = false;
        let out = refill_queue(
            &t,
            &SlidingRateLimiter::new(),
            &db,
            &dir.0,
            &f,
            1 << 30,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(out, RefillOutcome::FilterExhaustedHoldingLast);
        assert_eq!(history_count(&db.lock()).unwrap(), 2);
    }

    #[tokio::test]
    async fn evicted_history_photo_is_not_redownloaded() {
        let dir = TempDir::new();
        let db = new_db();
        seed_shown(&db, &dir.0, &["gone"], "x-1", 10);
        std::fs::remove_file(dir.0.join("wallpapers/photos/gone.jpg")).unwrap();
        mark_file_evicted(&db.lock(), "gone").unwrap();
        let (t, counters) = fake(
            |n| {
                let id = if n < 3 {
                    "gone".to_string()
                } else {
                    format!("fresh{n}")
                };
                HttpResponse::json(200, &meta_json(&id, "x-1", 64, 48, 10))
            },
            make_jpeg(64, 48),
        );
        let out = refill_queue(
            &t,
            &SlidingRateLimiter::new(),
            &db,
            &dir.0,
            &filter(&[]),
            1 << 30,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(out, RefillOutcome::TargetReached { unused_count: 5 });
        assert_eq!(counters.images.load(Ordering::SeqCst), 5);
        assert!(
            !get_cached_photo(&db.lock(), "gone")
                .unwrap()
                .unwrap()
                .file_present
        );
    }

    #[tokio::test]
    async fn throttle_response_starts_backoff_and_blocks_next_refill() {
        let dir = TempDir::new();
        let db = new_db();
        let (t, counters) = fake(|_| HttpResponse::json(503, ""), vec![]);
        let limiter = SlidingRateLimiter::new();
        let out = refill_queue(&t, &limiter, &db, &dir.0, &filter(&[]), 1 << 30, &[])
            .await
            .unwrap();
        assert_eq!(out, RefillOutcome::RateLimitedOrBackedOff);
        assert_eq!(limiter.consecutive_failures(), 1);
        assert!(limiter.cooldown_remaining().is_some());
        // Second call is refused locally without touching the network.
        let out = refill_queue(&t, &limiter, &db, &dir.0, &filter(&[]), 1 << 30, &[])
            .await
            .unwrap();
        assert_eq!(out, RefillOutcome::RateLimitedOrBackedOff);
        assert_eq!(counters.json.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn image_download_403_rejects_that_photo_without_throttling() {
        let dir = TempDir::new();
        let db = new_db();
        let (t, counters) = {
            let c = Arc::new(Counters::default());
            let c2 = c.clone();
            (
                FnTransport(move |url: &str, _: &[(&str, &str)]| {
                    Ok(if url == API_RANDOM_URL {
                        let n = c2.json.fetch_add(1, Ordering::SeqCst);
                        HttpResponse::json(200, &meta_json(&format!("p{n}"), "a-1", 64, 48, 10))
                    } else {
                        c2.images.fetch_add(1, Ordering::SeqCst);
                        HttpResponse::json(403, "")
                    })
                }),
                c,
            )
        };
        let limiter = SlidingRateLimiter::new();
        let out = refill_queue(&t, &limiter, &db, &dir.0, &filter(&[]), 1 << 30, &[])
            .await
            .unwrap();
        assert_eq!(out, RefillOutcome::RetryCapReached { unused_count: 0 });
        assert!(limiter.cooldown_remaining().is_none(), "no global throttle");
        assert_eq!(limiter.consecutive_failures(), 0);
        assert_eq!(counters.images.load(Ordering::SeqCst), MAX_API_REJECTIONS);
        assert!(get_cached_photo(&db.lock(), "p0").unwrap().is_none());
        assert!(!dir.0.join("wallpapers/photos/p0.jpg.part").exists());
    }

    #[tokio::test]
    async fn image_download_429_honours_retry_after() {
        let dir = TempDir::new();
        let db = new_db();
        let t = FnTransport(|url: &str, _: &[(&str, &str)]| {
            Ok(if url == API_RANDOM_URL {
                HttpResponse::json(200, &meta_json("p", "a-1", 64, 48, 10))
            } else {
                HttpResponse {
                    status: 429,
                    body: vec![],
                    retry_after: Some(Duration::from_secs(90)),
                }
            })
        });
        let limiter = SlidingRateLimiter::new();
        let out = refill_queue(&t, &limiter, &db, &dir.0, &filter(&[]), 1 << 30, &[])
            .await
            .unwrap();
        assert_eq!(out, RefillOutcome::RateLimitedOrBackedOff);
        assert!(limiter.cooldown_remaining().unwrap() > Duration::from_secs(80));
    }

    #[tokio::test]
    async fn network_error_goes_offline_with_cooldown_and_no_hot_loop() {
        let dir = TempDir::new();
        let db = new_db();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let t = FnTransport(move |_: &str, _: &[(&str, &str)]| {
            c.fetch_add(1, Ordering::SeqCst);
            Err(PhotoApiError::Transport("dns".into()))
        });
        let limiter = SlidingRateLimiter::new();
        let out = refill_queue(&t, &limiter, &db, &dir.0, &filter(&[]), 1 << 30, &[])
            .await
            .unwrap();
        assert_eq!(out, RefillOutcome::Offline);
        assert!(limiter.is_offline());
        assert!(limiter.cooldown_remaining().is_some());
        let out = refill_queue(&t, &limiter, &db, &dir.0, &filter(&[]), 1 << 30, &[])
            .await
            .unwrap();
        assert_eq!(out, RefillOutcome::RateLimitedOrBackedOff);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "second call stays local");
    }

    #[tokio::test]
    async fn failed_download_does_not_evict_cached_photos() {
        let dir = TempDir::new();
        let db = new_db();
        seed_shown(&db, &dir.0, &["old1", "old2"], "x-1", 1000);
        let t = FnTransport(|url: &str, _: &[(&str, &str)]| {
            Ok(if url == API_RANDOM_URL {
                HttpResponse::json(200, &meta_json("big", "x-1", 64, 48, 1000))
            } else {
                HttpResponse::json(500, "")
            })
        });
        refill_queue(
            &t,
            &SlidingRateLimiter::new(),
            &db,
            &dir.0,
            &filter(&[]),
            2000,
            &[],
        )
        .await
        .unwrap();
        let conn = db.lock();
        assert!(
            get_cached_photo(&conn, "old1")
                .unwrap()
                .unwrap()
                .file_present
        );
        assert!(dir.0.join("wallpapers/photos/old1.jpg").exists());
    }

    #[tokio::test]
    async fn oversized_download_is_rejected_and_leaves_no_part_file() {
        let dir = TempDir::new();
        let db = new_db();
        let (t, _) = fake(
            |n| HttpResponse::json(200, &meta_json(&format!("p{n}"), "a-1", 64, 48, 10)),
            vec![1; 64],
        );
        let client = PhotoApiClient::new(&t);
        let photos = dir.0.join("wallpapers/photos");
        std::fs::create_dir_all(&photos).unwrap();
        let err = client
            .download_image_to(
                "https://photos-api.iantruongphotography.com/v1/images/x",
                &photos.join("x.jpg.part"),
                32,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, PhotoApiError::Rejected(_)));
        assert!(!photos.join("x.jpg.part").exists());
        drop(db);
    }

    #[tokio::test]
    async fn exhausted_local_budget_makes_no_requests() {
        let dir = TempDir::new();
        let db = new_db();
        let (t, counters) = fake(|_| HttpResponse::json(500, ""), vec![]);
        let limiter = SlidingRateLimiter::with_limits(0, RATE_LIMIT_WINDOW);
        let out = refill_queue(&t, &limiter, &db, &dir.0, &filter(&[]), 1 << 30, &[])
            .await
            .unwrap();
        assert_eq!(out, RefillOutcome::RateLimitedOrBackedOff);
        assert_eq!(counters.json.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn budget_running_out_mid_refill_stops_before_download() {
        let dir = TempDir::new();
        let db = new_db();
        let (t, counters) = fake(
            |n| HttpResponse::json(200, &meta_json(&format!("p{n}"), "a-1", 64, 48, 10)),
            make_jpeg(64, 48),
        );
        // 3 slots: json+image (p0), json+image (p1) would need 4.
        let limiter = SlidingRateLimiter::with_limits(3, RATE_LIMIT_WINDOW);
        let out = refill_queue(&t, &limiter, &db, &dir.0, &filter(&[]), 1 << 30, &[])
            .await
            .unwrap();
        assert_eq!(out, RefillOutcome::RateLimitedOrBackedOff);
        assert_eq!(counters.json.load(Ordering::SeqCst), 2);
        assert_eq!(counters.images.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn repeated_404_gives_up_without_burning_all_attempts() {
        let dir = TempDir::new();
        let db = new_db();
        let (t, counters) = fake(
            |_| HttpResponse::json(404, r#"{"error":"no_photos"}"#),
            vec![],
        );
        let out = refill_queue(
            &t,
            &SlidingRateLimiter::new(),
            &db,
            &dir.0,
            &filter(&["nope-1"]),
            1 << 30,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(out, RefillOutcome::RetryCapReached { unused_count: 0 });
        assert_eq!(counters.json.load(Ordering::SeqCst), MAX_API_REJECTIONS);
    }

    #[tokio::test]
    async fn unsafe_ids_and_insecure_urls_are_rejected() {
        let dir = TempDir::new();
        let db = new_db();
        let bad_id = r#"{"albumSlug":"a","albumUrl":"u","bytes":1,"fallbackUsed":false,"featured":false,
            "id":"../../evil","imageUrl":"https://img.test/x","width":1,"height":1}"#;
        let http_url = r#"{"albumSlug":"a","albumUrl":"u","bytes":1,"fallbackUsed":false,"featured":false,
            "id":"fine","imageUrl":"http://img.test/x","width":1,"height":1}"#;
        let (t, counters) = fake(
            move |n| HttpResponse::json(200, if n % 2 == 0 { bad_id } else { http_url }),
            make_jpeg(8, 8),
        );
        let out = refill_queue(
            &t,
            &SlidingRateLimiter::new(),
            &db,
            &dir.0,
            &filter(&[]),
            1 << 30,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(out, RefillOutcome::RetryCapReached { unused_count: 0 });
        assert_eq!(counters.images.load(Ordering::SeqCst), 0);
        assert!(is_safe_photo_id("e95f46a6b4d6b9b772df7c79"));
        assert!(!is_safe_photo_id("a/b") && !is_safe_photo_id("") && !is_safe_photo_id("a.b"));
    }

    #[tokio::test]
    async fn orientation_mismatches_are_skipped_before_download() {
        let dir = TempDir::new();
        let db = new_db();
        let (t, counters) = fake(
            |n| HttpResponse::json(200, &meta_json(&format!("l{n}"), "a-1", 4000, 3000, 10)),
            make_jpeg(8, 8),
        );
        let mut f = filter(&[]);
        f.required_orientation = Some("portrait".into());
        let out = refill_queue(
            &t,
            &SlidingRateLimiter::new(),
            &db,
            &dir.0,
            &f,
            1 << 30,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(out, RefillOutcome::RetryCapReached { unused_count: 0 });
        assert_eq!(counters.json.load(Ordering::SeqCst), MAX_REFILL_ATTEMPTS);
        assert_eq!(counters.images.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn undecodable_image_falls_back_to_api_metadata_dimensions() {
        let dir = TempDir::new();
        let db = new_db();
        let (t, _) = fake(
            |n| HttpResponse::json(200, &meta_json(&format!("z{n}"), "a-1", 3000, 4000, 10)),
            b"not a real jpeg".to_vec(),
        );
        refill_queue(
            &t,
            &SlidingRateLimiter::new(),
            &db,
            &dir.0,
            &filter(&[]),
            1 << 30,
            &[],
        )
        .await
        .unwrap();
        let row = get_cached_photo(&db.lock(), "z0").unwrap().unwrap();
        assert_eq!(
            (row.width, row.height, row.orientation.as_str()),
            (3000, 4000, "portrait")
        );
    }

    #[tokio::test]
    async fn refill_evicts_shown_files_but_keeps_their_history() {
        let dir = TempDir::new();
        let db = new_db();
        seed_shown(&db, &dir.0, &["old1", "old2"], "x-1", 1000);
        let (t, _) = fake(
            |n| HttpResponse::json(200, &meta_json(&format!("n{n}"), "x-1", 64, 48, 1000)),
            make_jpeg(64, 48),
        );
        // Budget 2000 bytes: with 2000 cached, the first downloaded photo frees the oldest.
        refill_queue(
            &t,
            &SlidingRateLimiter::new(),
            &db,
            &dir.0,
            &filter(&[]),
            2000,
            &[],
        )
        .await
        .unwrap();
        let conn = db.lock();
        assert!(
            !get_cached_photo(&conn, "old1")
                .unwrap()
                .unwrap()
                .file_present
        );
        assert!(!dir.0.join("wallpapers/photos/old1.jpg").exists());
        assert!(get_history_entry(&conn, "old1").unwrap().is_some());
        assert!(get_history_entry(&conn, "old2").unwrap().is_some());
    }

    #[test]
    fn refill_threshold_is_two() {
        assert!(needs_refill(0) && needs_refill(2));
        assert!(!needs_refill(3));
    }

    #[tokio::test]
    async fn invalid_filter_slugs_are_an_error() {
        let dir = TempDir::new();
        let db = new_db();
        let (t, counters) = fake(|_| HttpResponse::json(500, ""), vec![]);
        let err = refill_queue(
            &t,
            &SlidingRateLimiter::new(),
            &db,
            &dir.0,
            &filter(&["bad slug!"]),
            1,
            &[],
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PhotoApiError::InvalidFilter(_)));
        assert_eq!(counters.json.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn classify_wrapper_maps_queue_vs_history() {
        let db = new_db();
        let dir = TempDir::new();
        seed_shown(&db, &dir.0, &["seen"], "s-1", 5);
        {
            let conn = db.lock();
            upsert_cached_photo(
                &conn,
                &NewCachedPhoto {
                    id: "queued".into(),
                    rel_path: "wallpapers/photos/queued.jpg".into(),
                    album_slug: "s-1".into(),
                    album_url: "u".into(),
                    featured: false,
                    fallback_used: false,
                    width: 2,
                    height: 1,
                    bytes: 1,
                    downloaded_at: 9,
                },
            )
            .unwrap();
        }
        let resp = |id: &str| -> ApiPhotoResponse {
            serde_json::from_str(&meta_json(id, "s-1", 4000, 3000, 1)).unwrap()
        };
        let conn = db.lock();
        assert_eq!(
            classify_candidate_photo(&conn, &resp("queued"), None).unwrap(),
            CandidateStatus::AlreadyInUnusedQueue
        );
        assert_eq!(
            classify_candidate_photo(&conn, &resp("seen"), None).unwrap(),
            CandidateStatus::AlreadyInHistory
        );
        assert_eq!(
            classify_candidate_photo(&conn, &resp("fresh"), Some("portrait")).unwrap(),
            CandidateStatus::OrientationMismatch
        );
        assert_eq!(
            classify_candidate_photo(&conn, &resp("fresh"), Some("landscape")).unwrap(),
            CandidateStatus::UniqueAndUnseen
        );
    }
}
