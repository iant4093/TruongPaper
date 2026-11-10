//! Public Steam Workshop metadata client (`ISteamRemoteStorage/GetPublishedFileDetails/v1`).
//!
//! The HTTP call is isolated in [`fetch_workshop_preview`]; response interpretation lives in
//! [`parse_published_file_details`] and is unit-tested with inline JSON.

use crate::workshop_id::parse_workshop_url_or_id;
use crate::WALLPAPER_ENGINE_APP_ID;
use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};

pub const PUBLISHED_FILE_DETAILS_URL: &str =
    "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

fn u64_field(details: &serde_json::Value, key: &str) -> Option<u64> {
    details.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

fn str_field(details: &serde_json::Value, key: &str) -> Option<String> {
    details.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// Interprets a full `GetPublishedFileDetails` response body for `workshop_id` and verifies the
/// item belongs to Wallpaper Engine (`consumer_app_id == 431960`).
pub fn parse_published_file_details(
    workshop_id: &str,
    resp: &serde_json::Value,
) -> anyhow::Result<WorkshopPreview> {
    let details = resp
        .pointer("/response/publishedfiledetails/0")
        .ok_or_else(|| anyhow!("Missing publishedfiledetails in Steam response"))?;

    // Steam reports `result: 1` for success and e.g. `9` (file not found) otherwise.
    if let Some(result) = u64_field(details, "result") {
        if result != 1 {
            bail!("Workshop item #{workshop_id} was not found (Steam result code {result})");
        }
    }

    let consumer_app_id = u64_field(details, "consumer_app_id").unwrap_or(0) as u32;
    if consumer_app_id != WALLPAPER_ENGINE_APP_ID {
        bail!(
            "Workshop item #{workshop_id} belongs to AppID {consumer_app_id}, not Wallpaper Engine ({WALLPAPER_ENGINE_APP_ID})"
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
        workshop_id: workshop_id.to_string(),
        title: str_field(details, "title")
            .unwrap_or_else(|| "Untitled Workshop Wallpaper".to_string()),
        description: str_field(details, "description").unwrap_or_default(),
        preview_url: str_field(details, "preview_url").unwrap_or_default(),
        file_size_bytes: u64_field(details, "file_size").unwrap_or(0),
        tags,
        time_updated: u64_field(details, "time_updated").unwrap_or(0),
    })
}

/// Queries the unauthenticated Steam Web API and returns a verified Wallpaper Engine preview.
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
        .post(PUBLISHED_FILE_DETAILS_URL)
        .form(&form)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    parse_published_file_details(&workshop_id, &resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample(consumer: u64) -> serde_json::Value {
        json!({"response": {"result": 1, "resultcount": 1, "publishedfiledetails": [{
            "publishedfileid": "2841928371",
            "result": 1,
            "consumer_app_id": consumer,
            "title": "Neon City",
            "description": "A neon scene",
            "preview_url": "https://images.steamusercontent.com/x/",
            "file_size": "1048576",
            "time_updated": 1700000000u64,
            "tags": [{"tag": "Scene"}, {"tag": "Anime"}]
        }]}})
    }

    #[test]
    fn parses_wallpaper_engine_item() {
        let p = parse_published_file_details("2841928371", &sample(431960)).unwrap();
        assert_eq!(p.workshop_id, "2841928371");
        assert_eq!(p.title, "Neon City");
        assert_eq!(p.file_size_bytes, 1_048_576);
        assert_eq!(p.time_updated, 1_700_000_000);
        assert_eq!(p.tags, vec!["Scene", "Anime"]);
    }

    #[test]
    fn rejects_other_apps() {
        let err = parse_published_file_details("2841928371", &sample(730))
            .unwrap_err()
            .to_string();
        assert!(err.contains("AppID 730"), "{err}");
    }

    #[test]
    fn rejects_missing_item_and_bad_body() {
        let not_found = json!({"response": {"publishedfiledetails": [{"result": 9}]}});
        assert!(parse_published_file_details("1234567", &not_found).is_err());
        assert!(parse_published_file_details("1234567", &json!({})).is_err());
    }

    #[test]
    fn numeric_file_size_and_defaults() {
        let body = json!({"response": {"publishedfiledetails": [
            {"consumer_app_id": 431960, "file_size": 42}
        ]}});
        let p = parse_published_file_details("1234567", &body).unwrap();
        assert_eq!(p.file_size_bytes, 42);
        assert_eq!(p.title, "Untitled Workshop Wallpaper");
        assert!(p.tags.is_empty());
    }
}
