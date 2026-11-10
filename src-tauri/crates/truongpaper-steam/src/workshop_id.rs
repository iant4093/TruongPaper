//! Steam Workshop URL / `steam://` URI / raw numeric ID parser.

use anyhow::bail;

/// Hosts accepted for `http(s)` Workshop links.
const STEAM_COMMUNITY_HOSTS: [&str; 2] = ["steamcommunity.com", "www.steamcommunity.com"];

fn is_workshop_id(s: &str) -> bool {
    (6..=12).contains(&s.len()) && s.chars().all(|c| c.is_ascii_digit())
}

/// Extracts a numeric Steam Workshop `publishedfileid` from:
/// - `https://steamcommunity.com/sharedfiles/filedetails/?id=<ID>` (also `/workshop/filedetails/`)
/// - `steam://url/CommunityFilePage/<ID>`
/// - `steam://openurl/<one of the above>`
/// - a raw numeric ID (6 to 12 digits)
pub fn parse_workshop_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if is_workshop_id(trimmed) {
        return Some(trimmed.to_string());
    }

    // Tolerate a pasted link without a scheme.
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else if trimmed.starts_with("steamcommunity.com/")
        || trimmed.starts_with("www.steamcommunity.com/")
    {
        format!("https://{trimmed}")
    } else {
        return None;
    };

    let parsed = url::Url::parse(&candidate).ok()?;
    match parsed.scheme() {
        "http" | "https" => {
            let host = parsed.host_str()?.to_ascii_lowercase();
            if !STEAM_COMMUNITY_HOSTS.contains(&host.as_str()) {
                return None;
            }
            let path = parsed.path().to_ascii_lowercase();
            if !(path.starts_with("/sharedfiles/filedetails")
                || path.starts_with("/workshop/filedetails"))
            {
                return None;
            }
            parsed
                .query_pairs()
                .find(|(k, v)| k == "id" && is_workshop_id(v))
                .map(|(_, v)| v.into_owned())
        }
        "steam" => {
            let host = parsed.host_str()?.to_ascii_lowercase();
            match host.as_str() {
                "url" => {
                    let mut segs = parsed.path_segments()?;
                    if segs.next()? != "CommunityFilePage" {
                        return None;
                    }
                    let id = segs.next()?;
                    is_workshop_id(id).then(|| id.to_string())
                }
                "openurl" => {
                    // steam://openurl/https://steamcommunity.com/...
                    let rest = candidate.strip_prefix("steam://openurl/")?;
                    if rest.starts_with("steam://") {
                        return None;
                    }
                    parse_workshop_id(rest)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Like [`parse_workshop_id`] but returns a descriptive error.
pub fn parse_workshop_url_or_id(input: &str) -> anyhow::Result<String> {
    match parse_workshop_id(input) {
        Some(id) => Ok(id),
        None => bail!("Invalid Steam Workshop URL or ID: {}", input.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steam_workshop_urls_and_ids() {
        assert_eq!(
            parse_workshop_id("https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371"),
            Some("2841928371".to_string())
        );
        assert_eq!(
            parse_workshop_id("steam://url/CommunityFilePage/2841928371"),
            Some("2841928371".to_string())
        );
        assert_eq!(
            parse_workshop_id("2841928371"),
            Some("2841928371".to_string())
        );
    }

    #[test]
    fn parses_url_variants() {
        let id = Some("2841928371".to_string());
        assert_eq!(
            parse_workshop_id(
                "  https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371&searchtext=neon  "
            ),
            id
        );
        assert_eq!(
            parse_workshop_id("https://steamcommunity.com/workshop/filedetails/?id=2841928371"),
            id
        );
        assert_eq!(
            parse_workshop_id("steamcommunity.com/sharedfiles/filedetails/?id=2841928371"),
            id
        );
        assert_eq!(
            parse_workshop_id(
                "steam://openurl/https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371"
            ),
            id
        );
        assert_eq!(
            parse_workshop_id("steam://url/CommunityFilePage/2841928371/"),
            id
        );
    }

    #[test]
    fn rejects_invalid_input() {
        for bad in [
            "",
            "   ",
            "abc",
            "12345",
            "1234567890123",
            "28419x8371",
            "https://steamcommunity.com/sharedfiles/filedetails/?id=abc",
            "https://steamcommunity.com/sharedfiles/filedetails/",
            "https://steamcommunity.com/profiles/76561198000000000",
            "https://example.com/sharedfiles/filedetails/?id=2841928371",
            "https://steamcommunity.com.evil.com/sharedfiles/filedetails/?id=2841928371",
            "steam://url/StoreAppPage/431960",
            "steam://run/431960",
            "ftp://steamcommunity.com/sharedfiles/filedetails/?id=2841928371",
        ] {
            assert_eq!(parse_workshop_id(bad), None, "should reject {bad:?}");
            assert!(parse_workshop_url_or_id(bad).is_err());
        }
    }

    #[test]
    fn url_or_id_returns_error_message() {
        let err = parse_workshop_url_or_id("nope").unwrap_err().to_string();
        assert!(err.contains("Invalid Steam Workshop URL or ID"));
    }
}
