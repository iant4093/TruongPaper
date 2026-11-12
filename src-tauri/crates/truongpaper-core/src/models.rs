//! Shared photo-cache model structs (docs/02 Sections 3 and 4, IPC shapes from Section 6).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Default on-disk photo cache budget: 250 MB.
pub const DEFAULT_MAX_CACHE_BYTES: u64 = 250 * 1024 * 1024;

/// Parses either `https://iantruongphotography.com/album/bald-mountain-11-1` or
/// `bald-mountain-11-1` into a normalized (lower-case) album slug.
///
/// Returns `None` when the input is empty or contains characters other than
/// ASCII letters, digits and `-`.
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
    let slug = slug
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !slug.is_empty() && slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Some(slug)
    } else {
        None
    }
}

/// Photo orientation as stored in `photos_cache.orientation` / `photos_history.orientation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    /// `width >= height`
    Landscape,
    /// `height > width`
    Portrait,
}

impl Orientation {
    /// Classifies display dimensions (after EXIF rotation).
    pub fn from_dimensions(width: u32, height: u32) -> Self {
        if height > width {
            Orientation::Portrait
        } else {
            Orientation::Landscape
        }
    }

    /// The exact string stored in SQLite.
    pub fn as_str(self) -> &'static str {
        match self {
            Orientation::Landscape => "landscape",
            Orientation::Portrait => "portrait",
        }
    }

    /// Strict parse of `"landscape"` / `"portrait"`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "landscape" => Some(Orientation::Landscape),
            "portrait" => Some(Orientation::Portrait),
            _ => None,
        }
    }
}

/// Which pool the rotation draws from (`settings.photo_filter_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterMode {
    All,
    Featured,
    Albums,
}

/// Active selection filter for queue pops, refills and history resets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoFilter {
    /// Empty = all public photos; otherwise one or more enabled album slugs.
    pub album_slugs: Vec<String>,
    pub featured_only: bool,
    /// `Some("landscape")` or `Some("portrait")` when orientation filtering is enabled.
    pub required_orientation: Option<String>,
    pub auto_reset_history_on_exhaustion: bool,
}

impl PhotoFilter {
    /// Typed view of [`PhotoFilter::required_orientation`]. Any value other than
    /// `"portrait"` is treated as landscape (matches the reference SQL).
    pub fn orientation(&self) -> Option<Orientation> {
        self.required_orientation.as_deref().map(|o| {
            if o == "portrait" {
                Orientation::Portrait
            } else {
                Orientation::Landscape
            }
        })
    }
}

/// Settings that drive [`PhotoFilter`] construction (`settings` table keys).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoSettings {
    pub filter_mode: FilterMode,
    pub featured_only: bool,
    pub exhaustion_auto_reset: bool,
    pub max_cache_bytes: u64,
}

impl Default for PhotoSettings {
    fn default() -> Self {
        Self {
            filter_mode: FilterMode::All,
            featured_only: false,
            exhaustion_auto_reset: true,
            max_cache_bytes: DEFAULT_MAX_CACHE_BYTES,
        }
    }
}

/// Result of checking an API candidate against local state before downloading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateStatus {
    UniqueAndUnseen,
    AlreadyInUnusedQueue,
    AlreadyInHistory,
    Blacklisted,
    OrientationMismatch,
}

/// Full `photos_cache` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedPhotoRow {
    pub id: String,
    /// Relative to the app data dir: `wallpapers/photos/<id>.jpg`.
    pub file_path: String,
    pub file_present: bool,
    pub album_slug: String,
    pub album_url: String,
    pub featured: bool,
    pub fallback_used: bool,
    pub width: u32,
    pub height: u32,
    pub orientation: String,
    pub bytes: u64,
    pub is_favorite: bool,
    pub downloaded_at: i64,
    pub last_shown_at: Option<i64>,
    pub shown_count: u32,
}

/// Values needed to insert a freshly downloaded photo into the unused queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCachedPhoto {
    pub id: String,
    pub rel_path: String,
    pub album_slug: String,
    pub album_url: String,
    pub featured: bool,
    pub fallback_used: bool,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub downloaded_at: i64,
}

impl NewCachedPhoto {
    /// Orientation derived from the display dimensions.
    pub fn orientation(&self) -> Orientation {
        Orientation::from_dimensions(self.width, self.height)
    }
}

/// `photos_history` row (kept even after the file is evicted from disk).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoHistoryEntry {
    pub photo_id: String,
    pub album_slug: String,
    pub featured: bool,
    pub orientation: String,
    pub first_shown_at: i64,
    pub last_shown_at: i64,
}

/// `photos_blacklist` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlacklistEntry {
    pub photo_id: String,
    pub album_slug: Option<String>,
    pub hidden_at: i64,
}

/// IPC photo item with an absolute, asset-protocol-ready `file_path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoItem {
    pub id: String,
    pub album_slug: String,
    pub album_url: String,
    pub file_path: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: u64,
    pub featured: bool,
    pub is_favorite: bool,
    pub downloaded_at: i64,
    pub last_shown_at: Option<i64>,
}

/// Full `saved_albums` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedAlbumRow {
    pub album_slug: String,
    pub title: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub album_url: String,
    pub cover_thumbnail_url: Option<String>,
    pub cover_blurhash: Option<String>,
    pub image_count: Option<u32>,
    pub is_enabled: bool,
    pub added_at: i64,
}

/// Values for inserting / refreshing a `saved_albums` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedAlbumUpsert {
    pub album_slug: String,
    pub title: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub album_url: String,
    pub cover_thumbnail_url: Option<String>,
    pub cover_blurhash: Option<String>,
    pub image_count: Option<u32>,
    pub added_at: i64,
}

/// IPC view of a saved album (docs/02 Section 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

impl From<&SavedAlbumRow> for SavedAlbum {
    fn from(row: &SavedAlbumRow) -> Self {
        Self {
            album_slug: row.album_slug.clone(),
            title: row.title.clone(),
            album_url: row.album_url.clone(),
            is_enabled: row.is_enabled,
            cover_thumbnail_url: row.cover_thumbnail_url.clone(),
            image_count: row.image_count,
            added_at: row.added_at,
        }
    }
}

/// One album in the public catalog (`GET /api/public/albums`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// One page of the public catalog.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPage {
    pub items: Vec<PublicCatalogAlbum>,
    pub next_cursor: Option<String>,
}

/// Snapshot returned to the UI (`photo_get_status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoStatus {
    pub filter_mode: FilterMode,
    pub featured_only: bool,
    pub unused_queue: Vec<PhotoItem>,
    pub favorite_photos: Vec<PhotoItem>,
    pub current_photo_by_monitor: HashMap<String, Option<PhotoItem>>,
    pub cached_bytes: u64,
    pub max_cache_bytes: u64,
    pub history_count: usize,
    pub blacklist_count: usize,
    pub is_refilling: bool,
    pub rate_limit_remaining: usize,
    /// The last API attempt failed with a network error and nothing has succeeded since.
    pub offline: bool,
    /// Offline with no unused photo left: rotation is serving already-shown cached photos.
    pub showing_cached_only: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_parsing_handles_urls_slugs_and_garbage() {
        assert_eq!(
            parse_album_slug("https://iantruongphotography.com/album/bald-mountain-11-1/"),
            Some("bald-mountain-11-1".into())
        );
        assert_eq!(
            parse_album_slug("https://iantruongphotography.com/album/Cape-Horn-11-7?x=1#y"),
            Some("cape-horn-11-7".into())
        );
        assert_eq!(
            parse_album_slug("  cape-horn-11-7 "),
            Some("cape-horn-11-7".into())
        );
        assert_eq!(parse_album_slug(""), None);
        assert_eq!(parse_album_slug("bad slug!"), None);
        assert_eq!(parse_album_slug("x'; DROP TABLE photos_cache;--"), None);
    }

    #[test]
    fn orientation_from_dimensions_and_strings() {
        assert_eq!(
            Orientation::from_dimensions(4000, 4000),
            Orientation::Landscape
        );
        assert_eq!(
            Orientation::from_dimensions(4505, 6757),
            Orientation::Portrait
        );
        assert_eq!(Orientation::Portrait.as_str(), "portrait");
        assert_eq!(
            Orientation::parse("landscape"),
            Some(Orientation::Landscape)
        );
        assert_eq!(Orientation::parse("square"), None);
    }

    #[test]
    fn filter_mode_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&FilterMode::Albums).unwrap(),
            "\"albums\""
        );
        let m: FilterMode = serde_json::from_str("\"featured\"").unwrap();
        assert_eq!(m, FilterMode::Featured);
    }

    #[test]
    fn catalog_page_deserializes_live_sample() {
        let json = r#"{"items":[{"albumId":"a","type":"photo","title":"Cape Horn 11-7",
            "description":"x","category":"Hikes","imageCount":1,
            "coverImageUrl":"https://c/o.jpg","coverThumbnailUrl":"https://c/t.jpg",
            "coverBlurhash":"LuE","slug":"cape-horn-11-7","visibility":"public"}],
            "nextCursor":null}"#;
        let page: CatalogPage = serde_json::from_str(json).unwrap();
        assert_eq!(page.items[0].slug, "cape-horn-11-7");
        assert_eq!(page.items[0].image_count, 1);
        assert!(page.next_cursor.is_none());
    }
}
