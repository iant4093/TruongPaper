//! Text layers: font loading, layout and CPU rasterization into an RGBA bitmap.
//!
//! Layout is device-free and generic over [`TextMetrics`] so it can be tested without a font
//! file; [`rasterize`] draws white glyph coverage into the alpha channel (the layer color,
//! brightness and alpha are applied by the GPU when the bitmap is composited).

use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use truongpaper_we_formats::WeVirtualFileSystem;

/// Largest bitmap side in pixels; larger layers are scaled by the sampler.
pub const MAX_TEXT_BITMAP_DIMENSION: u32 = 4096;
/// Point size used when the scene gives none.
pub const DEFAULT_POINT_SIZE: f32 = 32.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HAlign {
    Left,
    #[default]
    Center,
    Right,
}

impl HAlign {
    pub fn parse(s: Option<&str>) -> Self {
        match s.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("left") => HAlign::Left,
            Some("right") => HAlign::Right,
            _ => HAlign::Center,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VAlign {
    Top,
    #[default]
    Center,
    Bottom,
}

impl VAlign {
    pub fn parse(s: Option<&str>) -> Self {
        match s.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("top") => VAlign::Top,
            Some("bottom") => VAlign::Bottom,
            _ => VAlign::Center,
        }
    }
}

/// Everything besides the string, size and color that affects the rasterized text.
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    pub pointsize: f32,
    /// `systemfont_*` name or a VFS path to a font file.
    pub font: Option<String>,
    pub halign: HAlign,
    pub valign: VAlign,
    pub padding: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            pointsize: DEFAULT_POINT_SIZE,
            font: None,
            halign: HAlign::Center,
            valign: VAlign::Center,
            padding: 0.0,
        }
    }
}

/// Font measurements at a fixed pixel size.
pub trait TextMetrics {
    fn advance(&self, c: char) -> f32;
    /// Distance from the baseline up to the top of the line (positive).
    fn ascent(&self) -> f32;
    /// Distance from the baseline down to the bottom of the line (negative).
    fn descent(&self) -> f32;
    fn line_gap(&self) -> f32;
    fn kern(&self, _left: char, _right: char) -> f32 {
        0.0
    }

    fn line_height(&self) -> f32 {
        self.ascent() - self.descent() + self.line_gap()
    }

    fn line_width(&self, line: &str) -> f32 {
        let mut width = 0.0;
        let mut prev: Option<char> = None;
        for c in line.chars() {
            if let Some(p) = prev {
                width += self.kern(p, c);
            }
            width += self.advance(c);
            prev = Some(c);
        }
        width
    }
}

/// One laid-out line: `x` is the left edge and `baseline` the baseline, in bitmap pixels
/// (y down).
#[derive(Debug, Clone, PartialEq)]
pub struct LaidOutLine {
    pub text: String,
    pub x: f32,
    pub baseline: f32,
    pub width: f32,
}

fn lines_of(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

/// Natural `(width, height)` of `text` including `padding` on every side.
pub fn measure_text(metrics: &impl TextMetrics, text: &str, padding: f32) -> (f32, f32) {
    let lines = lines_of(text);
    let w = lines
        .iter()
        .map(|l| metrics.line_width(l))
        .fold(0.0f32, f32::max);
    (
        w + 2.0 * padding.max(0.0),
        lines.len() as f32 * metrics.line_height() + 2.0 * padding.max(0.0),
    )
}

/// Places every line of `text` inside a `size` box according to the style's alignment.
///
/// `padding` does not inset the box: Wallpaper Engine sizes a text layer's box to the text
/// itself (a one-line box is one line high even with the default `padding` of 32; see
/// 3122339805's 23-unit window headers), so insetting would push left/top/right/bottom aligned
/// text out of its box.
pub fn layout_text(
    metrics: &impl TextMetrics,
    text: &str,
    size: (f32, f32),
    style: &TextStyle,
) -> Vec<LaidOutLine> {
    let pad = 0.0f32;
    let _ = style.padding;
    let lines = lines_of(text);
    let line_h = metrics.line_height();
    let block_h = lines.len() as f32 * line_h;
    let avail_w = (size.0 - 2.0 * pad).max(0.0);
    let avail_h = (size.1 - 2.0 * pad).max(0.0);
    let top = match style.valign {
        VAlign::Top => pad,
        VAlign::Center => pad + (avail_h - block_h) / 2.0,
        VAlign::Bottom => size.1 - pad - block_h,
    };
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let width = metrics.line_width(line);
            let x = match style.halign {
                HAlign::Left => pad,
                HAlign::Center => pad + (avail_w - width) / 2.0,
                HAlign::Right => size.0 - pad - width,
            };
            LaidOutLine {
                text: (*line).to_string(),
                x,
                baseline: top + i as f32 * line_h + metrics.ascent(),
                width,
            }
        })
        .collect()
}

/// Scene units per em for one text `pointsize` point. Wallpaper Engine rasterizes text at
/// 300 DPI (`300 / 72` units per point): across the sampled Workshop scenes a one-line text
/// box is ~4.2 x pointsize x the font's line-height ratio.
pub const TEXT_UNITS_PER_POINT: f32 = 300.0 / 72.0;

/// [`TextMetrics`] for an `ab_glyph` font at a Wallpaper Engine `pointsize`.
pub struct FontMetrics<'a> {
    font: &'a FontVec,
    scale: PxScale,
}

impl<'a> FontMetrics<'a> {
    pub fn new(font: &'a FontVec, pointsize: f32) -> Self {
        Self {
            font,
            scale: PxScale::from(text_px_scale(font, pointsize)),
        }
    }
}

/// `ab_glyph` scale (ascent-to-descent height in units) for `pointsize`.
pub fn text_px_scale(font: &FontVec, pointsize: f32) -> f32 {
    let em = pointsize * TEXT_UNITS_PER_POINT;
    let ratio = font
        .units_per_em()
        .filter(|u| *u > 0.0)
        .map(|u| font.height_unscaled() / u)
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or(1.0);
    (em * ratio).clamp(1.0, 4096.0)
}

impl TextMetrics for FontMetrics<'_> {
    fn advance(&self, c: char) -> f32 {
        let f = self.font.as_scaled(self.scale);
        f.h_advance(f.glyph_id(c))
    }
    fn ascent(&self) -> f32 {
        self.font.as_scaled(self.scale).ascent()
    }
    fn descent(&self) -> f32 {
        self.font.as_scaled(self.scale).descent()
    }
    fn line_gap(&self) -> f32 {
        self.font.as_scaled(self.scale).line_gap()
    }
    fn kern(&self, left: char, right: char) -> f32 {
        let f = self.font.as_scaled(self.scale);
        f.kern(f.glyph_id(left), f.glyph_id(right))
    }
}

/// Straight-alpha RGBA pixels (RGB is white, coverage in alpha).
#[derive(Debug, Clone, PartialEq)]
pub struct TextBitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl TextBitmap {
    pub fn blank(width: u32, height: u32) -> Self {
        let mut rgba = vec![255u8; (width * height * 4) as usize];
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 0;
        }
        Self {
            width,
            height,
            rgba,
        }
    }
}

/// Grows a text box that is too small for `text` (scripts often write longer strings than the
/// editor text the box was sized for; Wallpaper Engine does not clip them). The box grows away
/// from the text alignment: left-aligned text grows to the right, top-aligned text downward,
/// centered text on both sides. Returns `(size, center offset)` in layer units (y up).
pub fn grow_text_box(
    metrics: &impl TextMetrics,
    text: &str,
    style: &TextStyle,
    size: (f32, f32),
) -> ((f32, f32), (f32, f32)) {
    let (mw, mh) = measure_text(metrics, text, 0.0);
    let (w, h) = (size.0.max(mw.ceil()), size.1.max(mh.ceil()));
    let (gw, gh) = (w - size.0, h - size.1);
    let dx = match style.halign {
        HAlign::Left => gw / 2.0,
        HAlign::Center => 0.0,
        HAlign::Right => -gw / 2.0,
    };
    let dy = match style.valign {
        VAlign::Top => -gh / 2.0,
        VAlign::Center => 0.0,
        VAlign::Bottom => gh / 2.0,
    };
    ((w, h), (dx, dy))
}

/// Bitmap size for a layer: the layer size, or the measured text when the layer has none.
pub fn bitmap_size(font: &FontVec, text: &str, style: &TextStyle, layer: (f32, f32)) -> (u32, u32) {
    let clamp = |v: f32| (v.ceil().max(1.0) as u32).min(MAX_TEXT_BITMAP_DIMENSION);
    let valid = |v: f32| v.is_finite() && v >= 1.0;
    let (mut w, mut h) = layer;
    if valid(w) && valid(h) {
        let ((gw, gh), _) =
            grow_text_box(&FontMetrics::new(font, style.pointsize), text, style, layer);
        return (clamp(gw), clamp(gh));
    }
    if !valid(w) || !valid(h) {
        let (mw, mh) = measure_text(
            &FontMetrics::new(font, style.pointsize),
            text,
            style.padding,
        );
        if !valid(w) {
            w = mw;
        }
        if !valid(h) {
            h = mh;
        }
    }
    (clamp(w), clamp(h))
}

/// Draws `text` into a `width` x `height` bitmap.
pub fn rasterize(
    font: &FontVec,
    text: &str,
    width: u32,
    height: u32,
    style: &TextStyle,
) -> TextBitmap {
    let mut bitmap = TextBitmap::blank(width, height);
    let metrics = FontMetrics::new(font, style.pointsize);
    let scaled = font.as_scaled(metrics.scale);
    for line in layout_text(&metrics, text, (width as f32, height as f32), style) {
        let mut x = line.x;
        let mut prev: Option<ab_glyph::GlyphId> = None;
        for c in line.text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            prev = Some(id);
            let glyph = id.with_scale_and_position(metrics.scale, point(x, line.baseline));
            x += scaled.h_advance(id);
            let Some(outlined) = font.outline_glyph(glyph) else {
                continue;
            };
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i64 + gx as i64;
                let py = bounds.min.y as i64 + gy as i64;
                if px < 0 || py < 0 || px >= width as i64 || py >= height as i64 {
                    return;
                }
                let idx = ((py as u32 * width + px as u32) * 4 + 3) as usize;
                let a = (coverage.clamp(0.0, 1.0) * 255.0).round() as u16;
                bitmap.rgba[idx] = (bitmap.rgba[idx] as u16 + a).min(255) as u8;
            });
        }
    }
    bitmap
}

/// Inputs that determine a rasterized bitmap.
#[derive(Debug, Clone, PartialEq)]
pub struct TextKey {
    pub text: String,
    pub style: TextStyle,
    pub width: u32,
    pub height: u32,
}

/// Remembers the last rasterization so unchanged text is never drawn twice.
#[derive(Debug, Default)]
pub struct TextCache {
    key: Option<TextKey>,
    bitmap: Option<TextBitmap>,
    renders: u32,
}

impl TextCache {
    /// Runs `render` only when `key` differs from the previous call. Returns `true` when the
    /// bitmap was (re)built.
    pub fn ensure(&mut self, key: TextKey, render: impl FnOnce(&TextKey) -> TextBitmap) -> bool {
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        self.bitmap = Some(render(&key));
        self.key = Some(key);
        self.renders += 1;
        true
    }

    pub fn bitmap(&self) -> Option<&TextBitmap> {
        self.bitmap.as_ref()
    }

    /// How many times the bitmap has been rebuilt.
    pub fn render_count(&self) -> u32 {
        self.renders
    }
}

/// System fonts tried when a scene font is missing or unreadable, most preferred first.
const SYSTEM_FONT_FALLBACKS: &[&str] = &[
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/SFNS.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/Library/Fonts/Arial.ttf",
    r"C:\Windows\Fonts\segoeui.ttf",
    r"C:\Windows\Fonts\arial.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
];

/// System font file for a `systemfont_<name>` value, when one is known.
fn system_font_candidates(font: &str) -> Vec<&'static str> {
    match font
        .trim()
        .to_ascii_lowercase()
        .trim_start_matches("systemfont_")
    {
        "arial" => vec![
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
            r"C:\Windows\Fonts\arial.ttf",
        ],
        "segoeui" | "segoe ui" => vec![r"C:\Windows\Fonts\segoeui.ttf"],
        "times" | "timesnewroman" | "times new roman" => vec![
            "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
            r"C:\Windows\Fonts\times.ttf",
        ],
        "courier" | "couriernew" | "courier new" => vec![
            "/System/Library/Fonts/Supplemental/Courier New.ttf",
            r"C:\Windows\Fonts\cour.ttf",
        ],
        _ => Vec::new(),
    }
}

fn font_from_bytes(bytes: Vec<u8>) -> Option<FontVec> {
    FontVec::try_from_vec_and_index(bytes, 0).ok()
}

/// Loads the layer's font from the wallpaper VFS when it names a font file, else the first
/// readable system font.
pub fn load_font(vfs: &WeVirtualFileSystem, font: Option<&str>) -> Result<FontVec, String> {
    if let Some(name) = font.map(str::trim).filter(|f| !f.is_empty()) {
        let lower = name.to_ascii_lowercase();
        if [".ttf", ".otf", ".ttc"].iter().any(|e| lower.ends_with(e)) {
            match vfs.read_bytes(name) {
                Ok(bytes) => match font_from_bytes(bytes) {
                    Some(f) => return Ok(f),
                    None => {
                        tracing::debug!("font {name} is not a valid font file; using a system font")
                    }
                },
                Err(e) => tracing::debug!("cannot read font {name}: {e}; using a system font"),
            }
        } else {
            for path in system_font_candidates(name) {
                if let Some(f) = std::fs::read(path).ok().and_then(font_from_bytes) {
                    return Ok(f);
                }
            }
        }
    }
    SYSTEM_FONT_FALLBACKS
        .iter()
        .find_map(|p| std::fs::read(p).ok().and_then(font_from_bytes))
        .ok_or_else(|| "no usable font found".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 px per glyph, ascent 8, descent -2, no gap: line height 10.
    struct Fixed;
    impl TextMetrics for Fixed {
        fn advance(&self, _: char) -> f32 {
            10.0
        }
        fn ascent(&self) -> f32 {
            8.0
        }
        fn descent(&self) -> f32 {
            -2.0
        }
        fn line_gap(&self) -> f32 {
            0.0
        }
    }

    fn style(h: HAlign, v: VAlign, padding: f32) -> TextStyle {
        TextStyle {
            halign: h,
            valign: v,
            padding,
            ..TextStyle::default()
        }
    }

    /// Regression (3122339805): script text longer than the editor box is not clipped; the box
    /// grows away from the alignment.
    #[test]
    fn long_text_grows_the_box_away_from_the_alignment() {
        let box_ = (30.0, 10.0);
        let st = |h| style(h, VAlign::Center, 0.0);
        assert_eq!(
            grow_text_box(&Fixed, "abcde", &st(HAlign::Left), box_),
            ((50.0, 10.0), (10.0, 0.0))
        );
        assert_eq!(
            grow_text_box(&Fixed, "abcde", &st(HAlign::Right), box_),
            ((50.0, 10.0), (-10.0, 0.0))
        );
        assert_eq!(
            grow_text_box(
                &Fixed,
                "ab\ncd",
                &style(HAlign::Center, VAlign::Top, 0.0),
                box_
            ),
            ((30.0, 20.0), (0.0, -5.0))
        );
        assert_eq!(
            grow_text_box(&Fixed, "ab", &st(HAlign::Left), box_),
            (box_, (0.0, 0.0))
        );
    }

    #[test]
    fn measure_includes_padding_and_longest_line() {
        assert_eq!(measure_text(&Fixed, "ab\nabcd", 5.0), (50.0, 30.0));
        assert_eq!(measure_text(&Fixed, "", 0.0), (0.0, 10.0));
    }

    #[test]
    fn alignment_places_lines_inside_the_box() {
        let size = (100.0, 40.0);
        let l = layout_text(&Fixed, "abc", size, &style(HAlign::Left, VAlign::Top, 0.0));
        assert_eq!((l[0].x, l[0].baseline), (0.0, 8.0));

        let l = layout_text(
            &Fixed,
            "abc",
            size,
            &style(HAlign::Center, VAlign::Center, 0.0),
        );
        assert_eq!((l[0].x, l[0].baseline), (35.0, 15.0 + 8.0));

        let l = layout_text(
            &Fixed,
            "abc",
            size,
            &style(HAlign::Right, VAlign::Bottom, 0.0),
        );
        assert_eq!((l[0].x, l[0].baseline), (70.0, 30.0 + 8.0));
    }

    /// Regression (3122339805): the default padding of 32 must not push left-aligned header
    /// text out of a one-line box.
    #[test]
    fn padding_does_not_inset_the_box() {
        let size = (100.0, 40.0);
        let l = layout_text(&Fixed, "abc", size, &style(HAlign::Left, VAlign::Top, 32.0));
        assert_eq!((l[0].x, l[0].baseline), (0.0, 8.0));
        let l = layout_text(
            &Fixed,
            "abc",
            size,
            &style(HAlign::Right, VAlign::Bottom, 32.0),
        );
        assert_eq!((l[0].x, l[0].baseline), (70.0, 30.0 + 8.0));
    }

    #[test]
    fn multiple_lines_stack_and_align_independently() {
        let l = layout_text(
            &Fixed,
            "a\r\nabcd",
            (100.0, 100.0),
            &style(HAlign::Right, VAlign::Top, 0.0),
        );
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].x, l[0].baseline), (90.0, 8.0));
        assert_eq!((l[1].x, l[1].baseline), (60.0, 18.0));
    }

    #[test]
    fn cache_only_renders_when_the_key_changes() {
        let mut cache = TextCache::default();
        let key = |text: &str, pointsize: f32| TextKey {
            text: text.to_string(),
            style: TextStyle {
                pointsize,
                ..TextStyle::default()
            },
            width: 4,
            height: 4,
        };
        let mut calls = 0;
        let mut draw = |k: &TextKey| {
            calls += 1;
            TextBitmap::blank(k.width, k.height)
        };
        assert!(cache.ensure(key("12:00", 20.0), &mut draw));
        assert!(!cache.ensure(key("12:00", 20.0), &mut draw));
        assert!(!cache.ensure(key("12:00", 20.0), &mut draw));
        assert!(cache.ensure(key("12:01", 20.0), &mut draw));
        assert!(cache.ensure(key("12:01", 24.0), &mut draw));
        assert!(!cache.ensure(key("12:01", 24.0), &mut draw));
        assert_eq!(cache.render_count(), 3);
        assert!(cache.bitmap().is_some());
        assert_eq!(calls, 3);
    }

    #[test]
    fn blank_bitmap_is_transparent_white() {
        let b = TextBitmap::blank(2, 1);
        assert_eq!(b.rgba, vec![255, 255, 255, 0, 255, 255, 255, 0]);
    }

    #[test]
    fn alignment_strings_parse_with_center_default() {
        assert_eq!(HAlign::parse(Some("Left")), HAlign::Left);
        assert_eq!(HAlign::parse(Some("right")), HAlign::Right);
        assert_eq!(HAlign::parse(None), HAlign::Center);
        assert_eq!(VAlign::parse(Some("TOP")), VAlign::Top);
        assert_eq!(VAlign::parse(Some("bottom")), VAlign::Bottom);
        assert_eq!(VAlign::parse(Some("middle")), VAlign::Center);
    }

    #[test]
    fn rasterizes_with_a_system_font_when_available() {
        let vfs = WeVirtualFileSystem::mount(
            std::env::temp_dir().join("truongpaper-text-none"),
            std::env::temp_dir().join("truongpaper-text-none-core"),
        );
        let Ok(font) = load_font(&vfs, None) else {
            return; // no system font on this machine
        };
        let style = TextStyle {
            pointsize: 24.0,
            ..TextStyle::default()
        };
        let (w, h) = bitmap_size(&font, "Hi", &style, (0.0, 0.0));
        assert!(w > 1 && h > 1);
        let bmp = rasterize(&font, "Hi", w, h, &style);
        assert!(bmp.rgba.chunks_exact(4).any(|p| p[3] > 128));
    }

    #[test]
    fn text_bitmap_uses_the_layer_box_not_the_pointsize() {
        // A text layer's texture is its scene-unit size; `pointsize` only scales the glyphs in it.
        let vfs = WeVirtualFileSystem::mount(
            std::env::temp_dir().join("tp-no-wallpaper"),
            std::env::temp_dir().join("tp-no-wallpaper-core"),
        );
        let Ok(font) = load_font(&vfs, None) else {
            return; // no system font on this machine
        };
        let small = TextStyle {
            pointsize: 10.0,
            ..TextStyle::default()
        };
        let large = TextStyle {
            pointsize: 45.0,
            ..TextStyle::default()
        };
        assert_eq!(
            bitmap_size(&font, "12:34", &small, (626.0, 366.0)),
            (626, 366)
        );
        assert_eq!(
            bitmap_size(&font, "12:34", &large, (626.0, 366.0)),
            (626, 366)
        );
        let ink = |style: &TextStyle| {
            rasterize(&font, "12:34", 626, 366, style)
                .rgba
                .chunks_exact(4)
                .map(|p| p[3] as u64)
                .sum::<u64>()
        };
        assert!(ink(&large) > ink(&small) * 4);
    }

    /// Regression (3122339805): `pointsize` is in points at 300 DPI, so one line of 9 pt text
    /// fills a ~23-unit header box (em ~37 units).
    #[test]
    fn pointsize_maps_to_300_dpi_em() {
        let vfs = WeVirtualFileSystem::mount(
            std::env::temp_dir().join("tp-no-wallpaper"),
            std::env::temp_dir().join("tp-no-wallpaper-core"),
        );
        let Ok(font) = load_font(&vfs, None) else {
            return;
        };
        let m = FontMetrics::new(&font, 9.0);
        let em = 9.0 * TEXT_UNITS_PER_POINT;
        let h = m.ascent() - m.descent();
        let ratio = font.height_unscaled() / font.units_per_em().unwrap();
        assert!((h - em * ratio).abs() < 0.5, "{h} vs {}", em * ratio);
        assert!((37.0..38.0).contains(&em));
    }
}
