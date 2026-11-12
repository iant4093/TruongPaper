//! Guest Workshop discovery from Steam Community's public server-rendered JSON.
//! No Steam cookies, Web API keys, or remote JavaScript enter the Studio webview.
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, time::Duration};

use crate::{WorkshopPreview, WALLPAPER_ENGINE_APP_ID};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopSort {
    #[default]
    Trending,
    MostSubscribed,
    Newest,
    TopRated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkshopBrowseQuery {
    pub search: String,
    pub sort: WorkshopSort,
    pub page: u32,
    pub days: i32,
    pub tags: Vec<String>,
}

impl Default for WorkshopBrowseQuery {
    fn default() -> Self {
        Self {
            search: String::new(),
            sort: WorkshopSort::Trending,
            page: 1,
            days: 7,
            tags: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopBrowseItem {
    #[serde(flatten)]
    pub preview: WorkshopPreview,
    pub author: String,
    pub subscribers: u64,
    pub rating: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkshopTag {
    pub tag: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkshopTagGroup {
    pub name: String,
    pub tags: Vec<WorkshopTag>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopBrowsePage {
    pub items: Vec<WorkshopBrowseItem>,
    pub page: u32,
    pub total_pages: u32,
    pub total_items: u64,
    pub tag_groups: Vec<WorkshopTagGroup>,
}

fn browse_url(query: &WorkshopBrowseQuery) -> Result<url::Url> {
    ensure!(
        (1..=1000).contains(&query.page),
        "Workshop page must be between 1 and 1000"
    );
    ensure!(
        [7, 30, 90, 365, -1].contains(&query.days),
        "Invalid trending period"
    );
    ensure!(
        query.search.chars().count() <= 200 && !query.search.chars().any(char::is_control),
        "Search is too long or contains control characters"
    );
    ensure!(
        query.tags.len() <= 16
            && query.tags.iter().all(|tag| !tag.is_empty()
                && tag.chars().count() <= 80
                && !tag.chars().any(char::is_control)),
        "Invalid Workshop filters"
    );
    let mut url = url::Url::parse("https://steamcommunity.com/workshop/browse/")?;
    let sort = match query.sort {
        WorkshopSort::Trending => "trend",
        WorkshopSort::MostSubscribed => "totaluniquesubscribers",
        WorkshopSort::Newest => "mostrecent",
        WorkshopSort::TopRated => "toprated",
    };
    {
        let mut params = url.query_pairs_mut();
        params
            .append_pair("appid", &WALLPAPER_ENGINE_APP_ID.to_string())
            .append_pair("section", "readytouseitems")
            .append_pair("l", "english")
            .append_pair("browsesort", sort)
            .append_pair("p", &query.page.to_string())
            .append_pair("days", &query.days.to_string())
            .append_pair("searchtext", query.search.trim())
            .append_pair("requiredtags[]", "Wallpaper");
        for tag in &query.tags {
            if tag != "Wallpaper" {
                params.append_pair("requiredtags[]", tag);
            }
        }
    }
    Ok(url)
}

/// A bounded, timed public request. Steam may alter this undocumented page format;
/// fail visibly if that happens rather than returning invented or partial results.
pub async fn browse_workshop(
    client: &reqwest::Client,
    query: &WorkshopBrowseQuery,
) -> Result<WorkshopBrowsePage> {
    let url = browse_url(query)?;
    let mut response = client
        .get(url)
        .timeout(Duration::from_secs(25))
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .send()
        .await
        .context("Cannot reach Steam Workshop")?
        .error_for_status()?;
    const MAX_BYTES: usize = 4 * 1024 * 1024;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= MAX_BYTES,
            "Steam Workshop response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    parse_browse_page(std::str::from_utf8(&bytes).context("Invalid Steam page encoding")?)
}

fn number(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .unwrap_or(0)
}

fn string(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

fn decoded(value: &Value) -> Result<Value> {
    match value.as_str() {
        Some(raw) => Ok(serde_json::from_str(raw)?),
        None => Ok(value.clone()),
    }
}

fn thumbnail(value: &Value) -> String {
    trusted_image_url(value.as_str().unwrap_or_default())
}

/// Returns `raw` when it is an https Steam CDN image URL without credentials, else `""`.
pub(crate) fn trusted_image_url(raw: &str) -> String {
    let Ok(url) = url::Url::parse(raw) else {
        return String::new();
    };
    let host = url.host_str().unwrap_or_default();
    let trusted = ["steamusercontent.com", "steamstatic.com", "akamaihd.net"]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
    if url.scheme() == "https" && trusted && url.username().is_empty() && url.password().is_none() {
        raw.to_owned()
    } else {
        String::new()
    }
}

fn parse_browse_page(html: &str) -> Result<WorkshopBrowsePage> {
    let marker = html.find("id=\"valve-ssr-data\"").context(
        "Steam changed its Workshop page format. Try again later or import a direct link.",
    )?;
    let start = html[marker..]
        .find('>')
        .map(|n| marker + n + 1)
        .context("Missing Workshop data")?;
    let end = html[start..]
        .find("</script>")
        .map(|n| start + n)
        .context("Incomplete Workshop data")?;
    let outer: Value = serde_json::from_str(&html[start..end]).context("Invalid Workshop data")?;
    let query_data = decoded(&outer["renderContext"]["queryData"])?;
    let query = query_data["queries"]
        .as_array()
        .and_then(|queries| {
            queries.iter().find(|q| {
                q["queryKey"][0] == "workshop_browse"
                    && number(&q["queryKey"][1]["appid"]) == u64::from(WALLPAPER_ENGINE_APP_ID)
            })
        })
        .context(
            "Steam did not return Workshop results. Try again later or import a direct link.",
        )?;
    let data = &query["state"]["data"];
    ensure!(
        number(&data["eresult"]) == 1,
        "Steam could not load Workshop results"
    );
    let results = data["results"]
        .as_array()
        .context("Missing Workshop results")?;
    let mut authors = HashMap::new();
    if let Some(details) = data["creator_player_link_details"].as_array() {
        for detail in details {
            authors.insert(
                string(&detail["public_data"]["steamid"]),
                string(&detail["public_data"]["persona_name"]),
            );
        }
    }
    let mut items = Vec::new();
    for result in results {
        if number(&result["consumer_appid"]) != u64::from(WALLPAPER_ENGINE_APP_ID) {
            continue;
        }
        let workshop_id = string(&result["publishedfileid"]);
        if workshop_id.is_empty() || !workshop_id.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let tags = result["tags"]
            .as_array()
            .map(|tags| {
                tags.iter()
                    .filter_map(|tag| tag["tag"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        items.push(WorkshopBrowseItem {
            preview: WorkshopPreview {
                workshop_id,
                title: string(&result["title"]),
                description: string(&result["short_description"]),
                preview_url: thumbnail(&result["preview_url"]),
                file_size_bytes: number(&result["file_size"]),
                tags,
                time_updated: number(&result["time_updated"]),
            },
            author: authors
                .get(&string(&result["creator"]))
                .cloned()
                .unwrap_or_default(),
            subscribers: number(&result["subscriptions"]),
            rating: number(&result["star_rating"]).min(5) as u8,
        });
    }
    let mut tag_groups = Vec::new();
    if let Some(loaders) = outer["loaderData"].as_array() {
        for loader in loaders {
            let Ok(loader) = decoded(loader) else {
                continue;
            };
            if let Some(groups) = loader["declaredTags"]["readytouse_tags"].as_array() {
                for group in groups {
                    let name = string(&group["name"]);
                    if !["Type", "Genre", "Resolution", "Miscellaneous", "Age Rating"]
                        .contains(&name.as_str())
                    {
                        continue;
                    }
                    let tags = group["tags"]
                        .as_array()
                        .map(|tags| {
                            tags.iter()
                                .filter(|tag| tag["admin_only"] != true)
                                .filter_map(|tag| {
                                    let name = tag["name"].as_str()?;
                                    Some(WorkshopTag {
                                        tag: name.to_owned(),
                                        label: tag["display_name"]
                                            .as_str()
                                            .unwrap_or(name)
                                            .to_owned(),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    tag_groups.push(WorkshopTagGroup { name, tags });
                }
            }
        }
    }
    let page = number(&data["current_page"]) as u32;
    let total_pages = number(&data["total_pages"]).min(1000) as u32;
    if !(1..=1000).contains(&page) {
        bail!("Steam returned an invalid page");
    }
    Ok(WorkshopBrowsePage {
        items,
        page,
        total_pages: total_pages.max(1),
        total_items: number(&data["total_count"]),
        tag_groups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture(results: Value) -> String {
        let outer = json!({"renderContext":{"queryData":json!({"queries":[{"queryKey":["workshop_browse",{"appid":431960}],"state":{"data":{"eresult":1,"current_page":2,"total_pages":2000,"total_count":50000,"results":results,"creator_player_link_details":[{"public_data":{"steamid":"123","persona_name":"Artist"}}]}}}]}).to_string()},"loaderData":[json!({"declaredTags":{"readytouse_tags":[{"name":"Type","tags":[{"name":"Scene","display_name":"Scene"},{"name":"Hidden","admin_only":true}]}]}}).to_string()]});
        format!("<script id=\"valve-ssr-data\" type=\"application/json\">{outer}</script>")
    }
    #[test]
    fn parses_public_data_and_filters_untrusted_items() {
        let html = fixture(json!([
            {"publishedfileid":"1234567890","consumer_appid":431960,"creator":"123","title":"Ocean","file_size":"184467","preview_url":"https://images.steamusercontent.com/ugc/abc/","tags":[{"tag":"Scene"}],"star_rating":4,"subscriptions":42},
            {"publishedfileid":"2222222222","consumer_appid":123},
            {"publishedfileid":"3333333333","consumer_appid":431960,"preview_url":"https://steamusercontent.com.evil.test/image"}
        ]));
        let page = parse_browse_page(&html).unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.page, 2);
        assert_eq!(page.total_pages, 1000);
        assert_eq!(page.items[0].author, "Artist");
        assert_eq!(page.items[0].preview.file_size_bytes, 184467);
        assert_eq!(page.items[0].preview.tags, vec!["Scene"]);
        assert!(page.items[1].preview.preview_url.is_empty());
        assert_eq!(page.tag_groups[0].tags.len(), 1);
    }
    #[test]
    fn encodes_search_and_restricts_pages_and_filters() {
        let query = WorkshopBrowseQuery {
            search: "a&appid=123".into(),
            page: 2,
            tags: vec!["1920 x 1080".into()],
            ..Default::default()
        };
        let url = browse_url(&query).unwrap();
        let pairs: Vec<_> = url.query_pairs().collect();
        assert_eq!(pairs.iter().filter(|(k, _)| k == "appid").count(), 1);
        assert!(pairs
            .iter()
            .any(|(k, v)| k == "searchtext" && v == "a&appid=123"));
        assert!(pairs
            .iter()
            .any(|(k, v)| k == "requiredtags[]" && v == "Wallpaper"));
        assert!(browse_url(&WorkshopBrowseQuery {
            page: 1001,
            ..query.clone()
        })
        .is_err());
        assert!(browse_url(&WorkshopBrowseQuery {
            search: "bad\nquery".into(),
            ..query
        })
        .is_err());
    }
    #[test]
    fn empty_catalog_is_valid_but_changed_page_is_an_error() {
        assert!(parse_browse_page(&fixture(json!([])))
            .unwrap()
            .items
            .is_empty());
        assert!(parse_browse_page("<html>Service unavailable</html>").is_err());
        assert!(parse_browse_page("<script id=\"valve-ssr-data\">{}</script>").is_err());
    }
}
