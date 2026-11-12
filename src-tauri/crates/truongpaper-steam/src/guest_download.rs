//! GGNetwork's public Workshop delivery flow. The provider receives the public
//! item URL, and its download page selects a GGNetwork CDN using an opaque key.
use crate::WALLPAPER_ENGINE_APP_ID;
use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Read},
    path::{Component, Path, PathBuf},
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;
use url::Url;
use zip::ZipArchive;

const LOOKUP: &str = "https://api.ggntw.com/steam.request";
const CDN: &str = "https://hk-cdn.ggntw.com/";
const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 12_000;
const MAX_PROJECT_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize)]
struct ProviderReply {
    result: u32,
    id: Option<String>,
    game: Option<String>,
    status: Option<String>,
    url: Option<String>,
}

fn cdn_url(reply: ProviderReply, id: &str) -> Result<Url> {
    if reply.result == 10 {
        bail!("GGNetwork requires its own account for this wallpaper. Try Steam for this item.");
    }
    ensure!(
        reply.result == 1 && reply.status.as_deref() == Some("1"),
        "GGNetwork does not have a guest download ready for this wallpaper."
    );
    ensure!(
        reply.id.as_deref() == Some(id)
            && reply.game.as_deref() == Some(WALLPAPER_ENGINE_APP_ID.to_string().as_str()),
        "GGNetwork returned a different Workshop item."
    );
    let page = reply
        .url
        .and_then(|value| Url::parse(&value).ok())
        .context("GGNetwork did not provide a valid download link.")?;
    ensure!(
        page.scheme() == "https"
            && page.host_str() == Some("ggntw.com")
            && page.port().is_none()
            && page.username().is_empty()
            && page.password().is_none()
            && page.query().is_none()
            && page.fragment().is_none(),
        "GGNetwork returned an unexpected download address."
    );
    let key = page
        .path()
        .strip_prefix("/download/")
        .context("GGNetwork returned an unexpected download page.")?;
    ensure!(
        (16..=512).contains(&key.len())
            && key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
        "GGNetwork returned an invalid download key."
    );
    Ok(Url::parse(CDN)?.join(key)?)
}

fn guest_client(timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("TruongPaper/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .timeout(timeout)
        .build()?)
}

/// Whether GGNetwork can serve an item to guests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GuestAvailability {
    Available,
    Unavailable,
    Unknown,
}

/// Most ids accepted by one availability check.
pub const MAX_AVAILABILITY_IDS: usize = 30;
const AVAILABILITY_TTL: Duration = Duration::from_secs(10 * 60);
const UNKNOWN_TTL: Duration = Duration::from_secs(60);
const PROBE_CONCURRENCY: usize = 4;

type AvailabilityCache = HashMap<String, (Instant, GuestAvailability)>;

fn availability_cache() -> &'static parking_lot::Mutex<AvailabilityCache> {
    static CACHE: OnceLock<parking_lot::Mutex<AvailabilityCache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn classify(reply: ProviderReply, id: &str) -> GuestAvailability {
    if reply.result == 10 {
        return GuestAvailability::Unavailable;
    }
    if cdn_url(reply, id).is_ok() {
        GuestAvailability::Available
    } else {
        GuestAvailability::Unavailable
    }
}

/// One cheap lookup request (no download), 8 s timeout, 64 KiB body cap.
async fn probe(client: &reqwest::Client, id: &str) -> GuestAvailability {
    let workshop_url = format!("https://steamcommunity.com/sharedfiles/filedetails/?id={id}");
    let Ok(mut resp) = client
        .post(LOOKUP)
        .header(reqwest::header::ORIGIN, "https://ggntw.com")
        .json(&serde_json::json!({ "url": workshop_url }))
        .send()
        .await
    else {
        return GuestAvailability::Unknown;
    };
    if !resp.status().is_success() {
        return GuestAvailability::Unknown;
    }
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = resp.chunk().await {
        if body.len() + chunk.len() > 64 * 1024 {
            return GuestAvailability::Unknown;
        }
        body.extend_from_slice(&chunk);
    }
    match serde_json::from_slice::<ProviderReply>(&body) {
        Ok(reply) => classify(reply, id),
        Err(_) => GuestAvailability::Unknown,
    }
}

/// Checks up to [`MAX_AVAILABILITY_IDS`] distinct Workshop ids. Results are cached for ten
/// minutes (one minute for `Unknown`) and at most four lookups run at once. Invalid ids are
/// reported as `Unknown`.
pub async fn guest_availability(ids: &[String]) -> Result<HashMap<String, GuestAvailability>> {
    ensure!(
        ids.len() <= MAX_AVAILABILITY_IDS,
        "Check at most {MAX_AVAILABILITY_IDS} wallpapers at a time."
    );
    let mut out = HashMap::new();
    let mut todo = Vec::new();
    {
        let cache = availability_cache().lock();
        for id in ids {
            if out.contains_key(id) || todo.contains(id) {
                continue;
            }
            if id.is_empty() || id.len() > 20 || !id.bytes().all(|b| b.is_ascii_digit()) {
                out.insert(id.clone(), GuestAvailability::Unknown);
            } else if let Some((at, value)) = cache.get(id) {
                let ttl = if *value == GuestAvailability::Unknown {
                    UNKNOWN_TTL
                } else {
                    AVAILABILITY_TTL
                };
                if at.elapsed() < ttl {
                    out.insert(id.clone(), *value);
                    continue;
                }
                todo.push(id.clone());
            } else {
                todo.push(id.clone());
            }
        }
    }
    if todo.is_empty() {
        return Ok(out);
    }
    let client = guest_client(Duration::from_secs(8))?;
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(PROBE_CONCURRENCY));
    let mut set = tokio::task::JoinSet::new();
    for id in todo {
        let (client, gate) = (client.clone(), gate.clone());
        set.spawn(async move {
            let _permit = gate.acquire_owned().await;
            let value = probe(&client, &id).await;
            (id, value)
        });
    }
    while let Some(Ok((id, value))) = set.join_next().await {
        availability_cache()
            .lock()
            .insert(id.clone(), (Instant::now(), value));
        out.insert(id, value);
    }
    Ok(out)
}

/// Downloads into private staging and installs only a complete wallpaper project.
pub async fn download_guest_workshop(
    id: &str,
    destination: &Path,
    on_progress: impl Fn(f32),
) -> Result<()> {
    ensure!(
        !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()),
        "Invalid Workshop ID"
    );
    let client = guest_client(Duration::from_secs(30))?;
    let workshop_url = format!("https://steamcommunity.com/sharedfiles/filedetails/?id={id}");
    let mut lookup = client
        .post(LOOKUP)
        .header(reqwest::header::ORIGIN, "https://ggntw.com")
        .json(&serde_json::json!({ "url": workshop_url }))
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Could not reach GGNetwork."))?;
    ensure!(
        lookup.status().is_success(),
        "GGNetwork could not check this wallpaper."
    );
    let mut json = Vec::new();
    while let Some(chunk) = lookup
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("GGNetwork's response was interrupted."))?
    {
        ensure!(
            json.len() + chunk.len() <= 64 * 1024,
            "GGNetwork returned an oversized response."
        );
        json.extend_from_slice(&chunk);
    }
    let reply =
        serde_json::from_slice(&json).context("GGNetwork returned an unreadable response.")?;
    let url = cdn_url(reply, id)?;

    let root = destination
        .parent()
        .context("Missing Workshop library folder")?;
    tokio::fs::create_dir_all(root).await?;
    let staging = tempfile::Builder::new()
        .prefix(".guest-")
        .tempdir_in(root)?;
    let archive_path = staging.path().join("wallpaper.zip");
    let mut response = client
        .get(url)
        .timeout(Duration::from_secs(15 * 60))
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Could not reach GGNetwork's file server."))?;
    ensure!(
        response.status().is_success(),
        "GGNetwork's file server could not deliver this wallpaper."
    );
    let expected = response.content_length();
    ensure!(
        expected.is_none_or(|size| size <= MAX_ARCHIVE_BYTES),
        "This wallpaper exceeds the 2 GiB guest download limit."
    );
    let mut file = tokio::fs::File::create(&archive_path).await?;
    let mut received = 0u64;
    let mut last_report = 0.0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("The GGNetwork download was interrupted."))?
    {
        received += chunk.len() as u64;
        ensure!(
            received <= MAX_ARCHIVE_BYTES,
            "This wallpaper exceeds the 2 GiB guest download limit."
        );
        file.write_all(&chunk).await?;
        if let Some(total) = expected.filter(|size| *size > 0) {
            let progress = 95.0 * (received as f64 / total as f64) as f32;
            if progress >= last_report + 1.0 {
                on_progress(progress.min(95.0));
                last_report = progress;
            }
        }
    }
    file.flush().await?;
    drop(file);
    ensure!(
        expected.is_none_or(|total| total == received),
        "The GGNetwork download was incomplete."
    );
    on_progress(95.0);

    let destination = destination.to_owned();
    tokio::task::spawn_blocking(move || {
        let extracted = staging.path().join("unpacked");
        unpack_archive(&archive_path, &extracted)?;
        let project = find_project(&extracted)?;
        install_project(staging, &project, &destination)
    })
    .await?
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|component| {
            let Component::Normal(name) = component else {
                return false;
            };
            let Some(name) = name.to_str() else {
                return false;
            };
            if name.ends_with(['.', ' '])
                || name.chars().any(|c| {
                    c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '<' | '>' | '|' | '"')
                })
            {
                return false;
            }
            let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
            !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !(stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.as_bytes()[3].is_ascii_digit())
        })
}

fn unpack_archive(archive_path: &Path, output: &Path) -> Result<()> {
    let mut archive = ZipArchive::new(File::open(archive_path)?)
        .context("GGNetwork did not return a readable ZIP wallpaper.")?;
    ensure!(
        archive.len() <= MAX_ENTRIES,
        "The wallpaper archive contains too many files."
    );
    fs::create_dir(output)?;
    let mut remaining = MAX_EXTRACTED_BYTES;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let relative = entry
            .enclosed_name()
            .context("The wallpaper archive contains an unsafe path.")?;
        ensure!(
            safe_relative(&relative) && !entry.is_symlink(),
            "The wallpaper archive contains an unsafe path."
        );
        let path = output.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&path)?;
            continue;
        }
        let size = entry.size();
        ensure!(
            size <= remaining,
            "The wallpaper archive exceeds the 4 GiB extraction limit."
        );
        fs::create_dir_all(path.parent().context("Missing archive parent")?)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        let count = io::copy(&mut entry.take(remaining + 1), &mut file)?;
        ensure!(
            count == size && count <= remaining,
            "The wallpaper archive contains an invalid file size."
        );
        remaining -= count;
    }
    Ok(())
}

fn find_project(unpacked: &Path) -> Result<PathBuf> {
    let mut projects = Vec::new();
    if unpacked.join("project.json").is_file() {
        projects.push(unpacked.to_owned());
    } else {
        for entry in fs::read_dir(unpacked)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() && entry.path().join("project.json").is_file() {
                projects.push(entry.path());
            }
        }
    }
    ensure!(
        projects.len() == 1,
        "The archive must contain one Wallpaper Engine project."
    );
    let project = projects.remove(0);
    let file = File::open(project.join("project.json"))?;
    ensure!(
        file.metadata()?.len() <= MAX_PROJECT_BYTES,
        "The wallpaper project file is too large."
    );
    let data: serde_json::Value =
        serde_json::from_reader(file).context("The wallpaper project file is unreadable.")?;
    let kind = data["type"].as_str().unwrap_or("").to_ascii_lowercase();
    ensure!(
        matches!(kind.as_str(), "scene" | "video" | "web"),
        "Guest downloads support Scene, Video, and Web wallpapers."
    );
    for key in ["file", "preview"] {
        if let Some(relative) = data[key].as_str().filter(|value| !value.is_empty()) {
            ensure!(
                safe_relative(Path::new(relative)),
                "The wallpaper project contains an unsafe file path."
            );
        }
    }
    if kind == "video" || kind == "web" {
        let file = data["file"]
            .as_str()
            .context("The wallpaper project does not specify its main file.")?;
        ensure!(
            project.join(file).is_file(),
            "The archive is missing the wallpaper's main file."
        );
    }
    Ok(project)
}

fn install_project(staging: tempfile::TempDir, project: &Path, destination: &Path) -> Result<()> {
    let previous = staging.path().join("previous");
    let has_previous = match fs::symlink_metadata(destination) {
        Ok(meta) => {
            ensure!(
                meta.is_dir() && !meta.is_symlink(),
                "The wallpaper destination is not a library folder."
            );
            fs::rename(destination, &previous)?;
            true
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
        Err(err) => return Err(err.into()),
    };
    if let Err(err) = fs::rename(project, destination) {
        if has_previous && fs::rename(&previous, destination).is_err() {
            let backup = staging.keep().join("previous");
            bail!(
                "Could not install the wallpaper. Its previous files are in {}.",
                backup.display()
            );
        }
        return Err(err).context("Could not install the wallpaper.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{write::SimpleFileOptions, ZipWriter};

    #[test]
    fn classifies_provider_replies() {
        let reply = |result: u32, status: &str| ProviderReply {
            result,
            id: Some("123456789".into()),
            game: Some("431960".into()),
            status: Some(status.into()),
            url: Some("https://ggntw.com/download/exampleKey1234567".into()),
        };
        assert_eq!(
            classify(reply(1, "1"), "123456789"),
            GuestAvailability::Available
        );
        assert_eq!(
            classify(reply(1, "0"), "123456789"),
            GuestAvailability::Unavailable
        );
        assert_eq!(
            classify(reply(10, "1"), "123456789"),
            GuestAvailability::Unavailable
        );
        assert_eq!(
            classify(reply(1, "1"), "999"),
            GuestAvailability::Unavailable
        );
    }

    #[tokio::test]
    async fn availability_rejects_too_many_ids_and_serves_cache() {
        let many: Vec<String> = (0..31).map(|n| n.to_string()).collect();
        assert!(guest_availability(&many).await.is_err());
        availability_cache().lock().insert(
            "424242".into(),
            (Instant::now(), GuestAvailability::Available),
        );
        let got = guest_availability(&["424242".into(), "bad".into()])
            .await
            .unwrap();
        assert_eq!(got["424242"], GuestAvailability::Available);
        assert_eq!(got["bad"], GuestAvailability::Unknown);
    }

    #[test]
    fn accepts_only_the_requested_wallpaper_and_a_ggnetwork_key() {
        let reply = |id: &str, url: &str| ProviderReply {
            result: 1,
            id: Some(id.to_owned()),
            game: Some("431960".to_owned()),
            status: Some("1".to_owned()),
            url: Some(url.to_owned()),
        };
        let good = "https://ggntw.com/download/exampleKey1234567";
        assert_eq!(
            cdn_url(reply("123456789", good), "123456789")
                .unwrap()
                .host_str(),
            Some("hk-cdn.ggntw.com")
        );
        assert!(cdn_url(reply("123456780", good), "123456789").is_err());
        assert!(cdn_url(
            reply(
                "123456789",
                "https://ggntw.com.example.com/download/exampleKey1234567"
            ),
            "123456789"
        )
        .is_err());
    }

    #[test]
    fn installs_a_wrapped_project_and_rejects_a_zip_escape() {
        let stage = tempfile::tempdir().unwrap();
        let archive = stage.path().join("test.zip");
        let mut writer = ZipWriter::new(File::create(&archive).unwrap());
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("wrapper/project.json", options).unwrap();
        writer
            .write_all(br#"{"type":"video","file":"wallpaper.mp4"}"#)
            .unwrap();
        writer.start_file("wrapper/wallpaper.mp4", options).unwrap();
        writer.write_all(b"test video bytes").unwrap();
        writer.finish().unwrap();
        let output = stage.path().join("unpacked");
        unpack_archive(&archive, &output).unwrap();
        let project = find_project(&output).unwrap();
        let destination = tempfile::tempdir().unwrap();
        let installed = destination.path().join("123456789");
        install_project(stage, &project, &installed).unwrap();
        assert_eq!(
            fs::read(installed.join("wallpaper.mp4")).unwrap(),
            b"test video bytes"
        );

        let stage = tempfile::tempdir().unwrap();
        let archive = stage.path().join("unsafe.zip");
        let mut writer = ZipWriter::new(File::create(&archive).unwrap());
        writer
            .start_file("../outside", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"unexpected").unwrap();
        writer.finish().unwrap();
        assert!(unpack_archive(&archive, &stage.path().join("unpacked")).is_err());
        assert!(!stage.path().join("outside").exists());
    }
}
