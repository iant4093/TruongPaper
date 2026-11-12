//! `truongpaper-photo-api`: Ian Truong Photography API v1 client, EXIF-aware decoder,
//! 5-photo offline queue, persistent shown history, and cache eviction manager.
//! Complete specification: `docs/02-PHOTO-API-AND-CACHE-SPEC.md`.
//!
//! * [`client`]: `GET /v1/photos/random` and `GET /api/public/albums` behind an injectable
//!   [`PhotoTransport`].
//! * [`limiter`]: 90-per-5-minutes sliding window with exponential backoff and jitter.
//! * [`queue`]: offline queue refill, small-album exit and exhaustion auto-reset.
//! * [`decoder`]: EXIF Orientation aware dimensions and `<= 8192px` texture decode.
//!
//! SQLite data access (queue pop, mark shown, history-preserving eviction, history reset,
//! saved albums, settings) lives in `truongpaper-core` and is re-exported here, so
//! `use truongpaper_photo_api::*;` exposes the same names as the spec's reference code.

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub mod client;
pub mod decoder;
pub mod error;
pub mod limiter;
pub mod queue;

pub use client::{
    check_status, is_allowed_image_url, parse_retry_after, DownloadOutcome, FnTransport,
    HttpResponse, PhotoApiClient, PhotoTransport, RandomPhotoQuery,
};
pub use decoder::{
    decode_photo_bytes_for_wgpu, decode_photo_for_wgpu, effective_max_dimension,
    inspect_display_dimensions, inspect_display_dimensions_from_bytes, DecodedGpuPhoto,
};
pub use error::{PhotoApiError, PhotoApiResult};
pub use limiter::SlidingRateLimiter;
pub use queue::{
    classify_candidate_photo, classify_candidate_photo_with_displayed, needs_refill, refill_queue,
    refill_queue_with_client, RefillOutcome,
};

pub use truongpaper_core::error::{CoreError, CoreResult};
pub use truongpaper_core::models::*;
pub use truongpaper_core::repo::*;

pub const API_RANDOM_URL: &str = "https://photos-api.iantruongphotography.com/v1/photos/random";
pub const PUBLIC_CATALOG_URL: &str =
    "https://iantruongphotography.com/api/public/albums?type=photo&limit=100";

/// Unused photos kept on disk per active filter.
pub const QUEUE_TARGET_UNUSED: usize = 5;
/// A background refill starts when the unused count is at or below this.
pub const QUEUE_REFILL_THRESHOLD: usize = 2;
/// Maximum random-photo requests per refill.
pub const MAX_REFILL_ATTEMPTS: usize = 12;
/// Small-album early exit: consecutive `AlreadyInUnusedQueue` answers before refill stops.
pub const SMALL_ALBUM_EXIT_CONSECUTIVE_DUPS: usize = 4;
/// Server-side budget: ~100 requests per IP per 5 minutes.
pub const RATE_LIMIT_HARD_CAP: usize = 100;
/// Safe ceiling used by [`SlidingRateLimiter`] (10-request margin under the hard cap).
pub const RATE_LIMIT_MAX_REQUESTS: usize = 90;
pub const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(300);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_album_urls_and_slugs() {
        assert_eq!(
            parse_album_slug("https://iantruongphotography.com/album/bald-mountain-11-1"),
            Some("bald-mountain-11-1".to_string())
        );
        assert_eq!(
            parse_album_slug("cape-horn-11-7"),
            Some("cape-horn-11-7".to_string())
        );
    }

    #[test]
    fn constants_match_spec() {
        assert_eq!(QUEUE_TARGET_UNUSED, 5);
        assert_eq!(QUEUE_REFILL_THRESHOLD, 2);
        assert_eq!(MAX_REFILL_ATTEMPTS, 12);
        assert_eq!(RATE_LIMIT_HARD_CAP, 100);
        const { assert!(RATE_LIMIT_MAX_REQUESTS < RATE_LIMIT_HARD_CAP) };
        assert_eq!(DEFAULT_MAX_CACHE_BYTES, 250 * 1024 * 1024);
    }

    #[test]
    fn api_photo_response_parses_live_sample() {
        let json = r#"{"albumSlug":"bald-mountain-11-1",
            "albumUrl":"https://iantruongphotography.com/album/bald-mountain-11-1",
            "bytes":17616840,"fallbackUsed":false,"featured":true,"height":4505,
            "id":"e95f46a6b4d6b9b772df7c79","imageUrl":"https://photos-api.iantruongphotography.com/v1/images/x/y",
            "width":6757}"#;
        let p: ApiPhotoResponse = serde_json::from_str(json).unwrap();
        assert_eq!((p.width, p.height), (Some(6757), Some(4505)));
        assert_eq!(p.bytes, 17_616_840);
    }
}
