//! What the photo caption says: album names and camera settings (EXIF) for Photos API
//! wallpapers, and the clock text. The desktop renderer draws it (`truongpaper_renderer::
//! photo_overlay`).

use chrono::{DateTime, Local};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::Path;
use truongpaper_core::{get_setting, set_setting, AppSettings, PhotoCaptionParts};
use truongpaper_renderer::OverlayText;

/// Album titles from the public catalog, `slug -> title` (settings key).
pub const SETTING_ALBUM_TITLES: &str = "album_titles";

/// Album name and camera settings of one photo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhotoCaption {
    pub album_slug: String,
    pub album: Option<String>,
    pub exif: Option<String>,
}

/// Which parts of the caption one display shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayPrefs {
    pub time: bool,
    pub album: bool,
    pub exif: bool,
}

impl OverlayPrefs {
    pub fn any(self) -> bool {
        self.time || self.album || self.exif
    }
}

impl From<PhotoCaptionParts> for OverlayPrefs {
    fn from(p: PhotoCaptionParts) -> Self {
        Self {
            time: p.time,
            album: p.album,
            exif: p.exif,
        }
    }
}

/// The caption settings of every display: a default plus displays with their own choice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptionSettings {
    pub default: OverlayPrefs,
    pub by_monitor: BTreeMap<String, OverlayPrefs>,
}

impl CaptionSettings {
    pub fn from_settings(s: &AppSettings) -> Self {
        Self {
            default: OverlayPrefs {
                time: s.photo_show_time,
                album: s.photo_show_album,
                exif: s.photo_show_exif,
            },
            by_monitor: s
                .photo_caption_by_monitor
                .iter()
                .map(|(id, parts)| (id.clone(), OverlayPrefs::from(*parts)))
                .collect(),
        }
    }

    /// What `monitor_id` shows: its own choice, else the default.
    pub fn for_monitor(&self, monitor_id: &str) -> OverlayPrefs {
        self.by_monitor
            .get(monitor_id)
            .copied()
            .unwrap_or(self.default)
    }

    /// Whether any display may show some caption (photo files are only read then).
    pub fn any(&self) -> bool {
        self.default.any() || self.by_monitor.values().any(|p| p.any())
    }

    /// Whether any display may show an album name.
    pub fn album_anywhere(&self) -> bool {
        self.default.album || self.by_monitor.values().any(|p| p.album)
    }

    /// Whether any display may show the clock.
    pub fn time_anywhere(&self) -> bool {
        self.default.time || self.by_monitor.values().any(|p| p.time)
    }
}

/// The caption text for one frame.
pub fn overlay_text(
    prefs: OverlayPrefs,
    caption: Option<&PhotoCaption>,
    now: DateTime<Local>,
    clock_24h: bool,
) -> OverlayText {
    let time_format = if clock_24h { "%-H:%M" } else { "%-I:%M %p" };
    OverlayText {
        time: prefs.time.then(|| now.format(time_format).to_string()),
        date: prefs.time.then(|| now.format("%A, %B %-d").to_string()),
        album: caption
            .filter(|_| prefs.album)
            .and_then(|c| c.album.clone()),
        exif: caption.filter(|_| prefs.exif).and_then(|c| c.exif.clone()),
    }
}

/// `day-6-dresden` -> `Day 6 Dresden` (used until the catalog title is known).
pub fn title_from_slug(slug: &str) -> String {
    slug.split('-')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Album title for a slug: a saved album, then the cached catalog, then `None`.
pub fn known_album_title(conn: &Connection, slug: &str) -> Option<String> {
    let saved: Option<String> = conn
        .query_row(
            "SELECT title FROM saved_albums WHERE album_slug = ?1 AND title <> ''",
            params![slug],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    saved.or_else(|| {
        get_setting::<HashMap<String, String>>(conn, SETTING_ALBUM_TITLES)
            .ok()
            .flatten()
            .and_then(|titles| titles.get(slug).cloned())
    })
}

/// Stores catalog titles for later captions.
pub fn store_album_titles<'a>(
    conn: &Connection,
    albums: impl IntoIterator<Item = (&'a str, &'a str)>,
) {
    let mut titles: HashMap<String, String> = get_setting(conn, SETTING_ALBUM_TITLES)
        .ok()
        .flatten()
        .unwrap_or_default();
    for (slug, title) in albums {
        if !slug.is_empty() && !title.trim().is_empty() {
            titles.insert(slug.to_string(), title.trim().to_string());
        }
    }
    if let Err(err) = set_setting(conn, SETTING_ALBUM_TITLES, &titles) {
        tracing::warn!("could not store album titles: {err}");
    }
}

/// Caption for a cached photo. `album` falls back to a title made from the slug.
/// Caption for a cached photo. `known_title` is [`known_album_title`] (looked up under the
/// database lock); the file is read here, outside it. Without a known title the album name is
/// made from the slug.
pub fn caption_for(album_slug: &str, known_title: Option<String>, file: &Path) -> PhotoCaption {
    let album = (!album_slug.is_empty())
        .then(|| known_title.unwrap_or_else(|| title_from_slug(album_slug)));
    PhotoCaption {
        album_slug: album_slug.to_string(),
        album,
        exif: read_exif_line(file),
    }
}

/// Every known album title (saved albums win over catalog titles), `slug -> title`.
pub fn all_album_titles(conn: &Connection) -> HashMap<String, String> {
    let mut titles: HashMap<String, String> = get_setting(conn, SETTING_ALBUM_TITLES)
        .ok()
        .flatten()
        .unwrap_or_default();
    if let Ok(mut stmt) =
        conn.prepare("SELECT album_slug, title FROM saved_albums WHERE title <> ''")
    {
        if let Ok(rows) =
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        {
            titles.extend(rows.flatten());
        }
    }
    titles
}

// ---------------------------------------------------------------------------------------------
// EXIF
// ---------------------------------------------------------------------------------------------

/// Camera settings read from a JPEG's EXIF block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CameraSettings {
    pub make: Option<String>,
    pub model: Option<String>,
    pub focal_length_mm: Option<f64>,
    pub f_number: Option<f64>,
    pub exposure_s: Option<f64>,
    pub iso: Option<u32>,
}

impl CameraSettings {
    /// `Canon EOS R7 · 12mm · f/6.3 · 1/125s · ISO 100`; `None` when nothing is known.
    pub fn line(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(camera) = self.camera() {
            parts.push(camera);
        }
        if let Some(f) = self.focal_length_mm.filter(|f| *f > 0.0) {
            parts.push(format!("{}mm", f.round() as i64));
        }
        if let Some(n) = self.f_number.filter(|n| *n > 0.0) {
            parts.push(format!("f/{}", trim_decimal(n)));
        }
        if let Some(t) = self.exposure_s.filter(|t| *t > 0.0) {
            parts.push(if t < 1.0 {
                format!("1/{}s", (1.0 / t).round() as i64)
            } else {
                format!("{}s", trim_decimal(t))
            });
        }
        if let Some(iso) = self.iso.filter(|i| *i > 0) {
            parts.push(format!("ISO {iso}"));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// The model, prefixed with the brand when the model does not already name it.
    fn camera(&self) -> Option<String> {
        let model = self
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty());
        let make = self
            .make
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty());
        match (make, model) {
            (Some(make), Some(model)) => {
                let brand = make.split_whitespace().next().unwrap_or(make);
                if model.to_lowercase().starts_with(&brand.to_lowercase()) {
                    Some(model.to_string())
                } else {
                    Some(format!("{} {model}", title_case_brand(brand)))
                }
            }
            (None, Some(model)) => Some(model.to_string()),
            (Some(make), None) => Some(make.to_string()),
            (None, None) => None,
        }
    }
}

/// `NIKON` -> `Nikon`, `FUJIFILM` -> `Fujifilm`; mixed-case brands stay as they are.
fn title_case_brand(brand: &str) -> String {
    if brand.chars().any(|c| c.is_lowercase()) || brand.len() <= 3 {
        return brand.to_string();
    }
    let lower = brand.to_lowercase();
    let mut chars = lower.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn trim_decimal(v: f64) -> String {
    let s = format!("{:.1}", v);
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

/// The caption line for a photo file, or `None` without EXIF.
pub fn read_exif_line(path: &Path) -> Option<String> {
    let mut head = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(256 * 1024)
        .read_to_end(&mut head)
        .ok()?;
    parse_jpeg_exif(&head).and_then(|s| s.line())
}

/// Finds the EXIF APP1 segment of a JPEG and reads the camera settings.
pub fn parse_jpeg_exif(jpeg: &[u8]) -> Option<CameraSettings> {
    if jpeg.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut pos = 2;
    while pos + 4 <= jpeg.len() {
        if jpeg[pos] != 0xFF {
            return None;
        }
        let marker = jpeg[pos + 1];
        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            pos += 2;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            return None; // Image data starts: no EXIF before it.
        }
        let len = u16::from_be_bytes([jpeg[pos + 2], jpeg[pos + 3]]) as usize;
        let body = jpeg.get(pos + 4..pos + 2 + len)?;
        if marker == 0xE1 && body.starts_with(b"Exif\0\0") {
            return parse_tiff(&body[6..]);
        }
        pos += 2 + len;
    }
    None
}

struct Tiff<'a> {
    data: &'a [u8],
    le: bool,
}

impl Tiff<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let b = self.data.get(at..at + 2)?;
        Some(if self.le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let b = self.data.get(at..at + 4)?;
        let b = [b[0], b[1], b[2], b[3]];
        Some(if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    /// `(tag, type, count, entry offset)` of every entry in the IFD at `ifd`.
    fn entries(&self, ifd: usize) -> Vec<(u16, u16, u32, usize)> {
        let Some(n) = self.u16(ifd) else {
            return Vec::new();
        };
        (0..n as usize)
            .filter_map(|i| {
                let e = ifd + 2 + i * 12;
                Some((self.u16(e)?, self.u16(e + 2)?, self.u32(e + 4)?, e))
            })
            .collect()
    }

    /// Where an entry's value lives (inline when it fits in 4 bytes).
    fn value_at(&self, typ: u16, count: u32, entry: usize) -> Option<usize> {
        let size = match typ {
            1 | 2 | 7 => 1,
            3 => 2,
            4 | 9 => 4,
            5 | 10 => 8,
            _ => return None,
        } * count as usize;
        if size <= 4 {
            Some(entry + 8)
        } else {
            self.u32(entry + 8).map(|o| o as usize)
        }
    }

    fn ascii(&self, typ: u16, count: u32, entry: usize) -> Option<String> {
        if typ != 2 {
            return None;
        }
        let at = self.value_at(typ, count, entry)?;
        let bytes = self.data.get(at..at + count as usize)?;
        let s: String = bytes
            .iter()
            .take_while(|b| **b != 0)
            .map(|b| *b as char)
            .collect();
        let s = s.trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    fn rational(&self, typ: u16, count: u32, entry: usize) -> Option<f64> {
        if typ != 5 && typ != 10 {
            return None;
        }
        let at = self.value_at(typ, count, entry)?;
        let (n, d) = (self.u32(at)? as f64, self.u32(at + 4)? as f64);
        (d != 0.0).then(|| n / d)
    }

    fn integer(&self, typ: u16, count: u32, entry: usize) -> Option<u32> {
        let at = self.value_at(typ, count, entry)?;
        match typ {
            3 => self.u16(at).map(u32::from),
            4 => self.u32(at),
            _ => None,
        }
    }
}

fn parse_tiff(data: &[u8]) -> Option<CameraSettings> {
    let le = match data.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let tiff = Tiff { data, le };
    let ifd0 = tiff.u32(4)? as usize;
    let mut out = CameraSettings::default();
    let mut exif_ifd = None;
    for (tag, typ, count, entry) in tiff.entries(ifd0) {
        match tag {
            0x010F => out.make = tiff.ascii(typ, count, entry),
            0x0110 => out.model = tiff.ascii(typ, count, entry),
            0x8769 => exif_ifd = tiff.integer(typ, count, entry),
            _ => {}
        }
    }
    if let Some(ifd) = exif_ifd {
        for (tag, typ, count, entry) in tiff.entries(ifd as usize) {
            match tag {
                0x829A => out.exposure_s = tiff.rational(typ, count, entry),
                0x829D => out.f_number = tiff.rational(typ, count, entry),
                0x8827 => out.iso = tiff.integer(typ, count, entry),
                0x920A => out.focal_length_mm = tiff.rational(typ, count, entry),
                _ => {}
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Little-endian TIFF with Make, Model and an EXIF IFD.
    fn sample_exif() -> Vec<u8> {
        let mut t: Vec<u8> = b"II*\0".to_vec();
        t.extend_from_slice(&8u32.to_le_bytes());
        // IFD0 at 8: 3 entries.
        let ifd0_len = 2 + 3 * 12 + 4;
        let strings_at = 8 + ifd0_len;
        let make = b"Canon\0";
        let model = b"Canon EOS R7\0";
        let exif_at = strings_at + make.len() + model.len();
        t.extend_from_slice(&3u16.to_le_bytes());
        let entry = |t: &mut Vec<u8>, tag: u16, typ: u16, count: u32, value: u32| {
            t.extend_from_slice(&tag.to_le_bytes());
            t.extend_from_slice(&typ.to_le_bytes());
            t.extend_from_slice(&count.to_le_bytes());
            t.extend_from_slice(&value.to_le_bytes());
        };
        entry(&mut t, 0x010F, 2, make.len() as u32, strings_at as u32);
        entry(
            &mut t,
            0x0110,
            2,
            model.len() as u32,
            (strings_at + make.len()) as u32,
        );
        entry(&mut t, 0x8769, 4, 1, exif_at as u32);
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(make);
        t.extend_from_slice(model);
        // EXIF IFD: 4 entries, rationals after it.
        let rationals_at = exif_at + 2 + 4 * 12 + 4;
        t.extend_from_slice(&4u16.to_le_bytes());
        entry(&mut t, 0x829A, 5, 1, rationals_at as u32);
        entry(&mut t, 0x829D, 5, 1, (rationals_at + 8) as u32);
        let mut iso = Vec::new();
        iso.extend_from_slice(&0x8827u16.to_le_bytes());
        iso.extend_from_slice(&3u16.to_le_bytes());
        iso.extend_from_slice(&1u32.to_le_bytes());
        iso.extend_from_slice(&100u16.to_le_bytes());
        iso.extend_from_slice(&[0, 0]);
        t.extend_from_slice(&iso);
        entry(&mut t, 0x920A, 5, 1, (rationals_at + 16) as u32);
        t.extend_from_slice(&0u32.to_le_bytes());
        for (n, d) in [(1u32, 125u32), (63, 10), (12, 1)] {
            t.extend_from_slice(&n.to_le_bytes());
            t.extend_from_slice(&d.to_le_bytes());
        }
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        let len = (2 + 6 + t.len()) as u16;
        jpeg.extend_from_slice(&len.to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&t);
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
        jpeg
    }

    #[test]
    fn reads_camera_settings_from_exif() {
        let settings = parse_jpeg_exif(&sample_exif()).unwrap();
        assert_eq!(
            settings.line().as_deref(),
            Some("Canon EOS R7 · 12mm · f/6.3 · 1/125s · ISO 100")
        );
    }

    #[test]
    fn camera_name_avoids_repeating_the_brand() {
        let s = |make: &str, model: &str| CameraSettings {
            make: Some(make.into()),
            model: Some(model.into()),
            ..Default::default()
        };
        assert_eq!(
            s("NIKON CORPORATION", "NIKON Z 6").line().as_deref(),
            Some("NIKON Z 6")
        );
        assert_eq!(
            s("FUJIFILM", "X-T5").line().as_deref(),
            Some("Fujifilm X-T5")
        );
        assert_eq!(
            s("SONY", "ILCE-7M4").line().as_deref(),
            Some("Sony ILCE-7M4")
        );
    }

    #[test]
    fn long_exposures_read_in_seconds() {
        let s = CameraSettings {
            exposure_s: Some(2.5),
            ..Default::default()
        };
        assert_eq!(s.line().as_deref(), Some("2.5s"));
    }

    #[test]
    fn files_without_exif_have_no_line() {
        assert!(parse_jpeg_exif(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2]).is_none());
        assert!(parse_jpeg_exif(b"not a jpeg").is_none());
    }

    #[test]
    fn slugs_become_readable_titles() {
        assert_eq!(title_from_slug("day-6-dresden"), "Day 6 Dresden");
        assert_eq!(title_from_slug(""), "");
    }

    #[test]
    fn displays_can_have_their_own_caption() {
        let mut s = AppSettings {
            photo_show_time: true,
            ..Default::default()
        };
        s.photo_caption_by_monitor.insert(
            "right".into(),
            PhotoCaptionParts {
                time: false,
                album: true,
                exif: true,
            },
        );
        s.photo_caption_by_monitor
            .insert("off".into(), PhotoCaptionParts::default());
        let c = CaptionSettings::from_settings(&s);
        // No entry: the default.
        assert_eq!(
            c.for_monitor("left"),
            OverlayPrefs {
                time: true,
                album: false,
                exif: false
            }
        );
        assert_eq!(
            c.for_monitor("right"),
            OverlayPrefs {
                time: false,
                album: true,
                exif: true
            }
        );
        assert!(!c.for_monitor("off").any());
        assert!(c.any() && c.album_anywhere() && c.time_anywhere());

        // Everything off by default, one display with only the album.
        let mut s = AppSettings::default();
        s.photo_caption_by_monitor.insert(
            "one".into(),
            PhotoCaptionParts {
                album: true,
                ..Default::default()
            },
        );
        let c = CaptionSettings::from_settings(&s);
        assert!(c.any() && c.album_anywhere() && !c.time_anywhere());
        assert!(!CaptionSettings::from_settings(&AppSettings::default()).any());
    }

    #[test]
    fn per_display_captions_survive_a_settings_round_trip() {
        let mut s = AppSettings::default();
        s.photo_caption_by_monitor.insert(
            "\\\\.\\DISPLAY2".into(),
            PhotoCaptionParts {
                time: true,
                album: false,
                exif: true,
            },
        );
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json["photoCaptionByMonitor"]["\\\\.\\DISPLAY2"],
            serde_json::json!({ "time": true, "album": false, "exif": true })
        );
        let back: AppSettings = serde_json::from_value(json).unwrap();
        assert_eq!(back.photo_caption_by_monitor, s.photo_caption_by_monitor);
        // Settings saved before this field existed still load.
        let mut old = serde_json::to_value(AppSettings::default()).unwrap();
        old.as_object_mut().unwrap().remove("photoCaptionByMonitor");
        let old: AppSettings = serde_json::from_value(old).unwrap();
        assert!(old.photo_caption_by_monitor.is_empty());
    }

    #[test]
    fn clock_follows_the_settings() {
        let now = Local.with_ymd_and_hms(2026, 11, 16, 13, 7, 0).unwrap();
        let caption = PhotoCaption {
            album_slug: "day-6-dresden".into(),
            album: Some("Day 6 - Dresden".into()),
            exif: Some("ISO 100".into()),
        };
        let all = OverlayPrefs {
            time: true,
            album: true,
            exif: true,
        };
        let text = overlay_text(all, Some(&caption), now, false);
        assert_eq!(text.time.as_deref(), Some("1:07 PM"));
        let late = Local.with_ymd_and_hms(2026, 11, 16, 21, 5, 0).unwrap();
        assert_eq!(
            overlay_text(all, None, late, true).time.as_deref(),
            Some("21:05")
        );
        assert_eq!(text.date.as_deref(), Some("Monday, November 16"));
        assert_eq!(text.album.as_deref(), Some("Day 6 - Dresden"));
        let none = overlay_text(OverlayPrefs::default(), Some(&caption), now, false);
        assert!(none.is_empty());
    }
}
