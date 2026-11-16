//! Wallpaper Engine's "Adjust Windows color": the wallpaper's `schemecolor` property becomes
//! the Windows accent color (Start, taskbar, title bars, selection highlights).
//!
//! This module is the OS-independent part: parsing the property and building the palette
//! Windows stores next to the accent. `win_accent` writes it.

/// Parses a Wallpaper Engine color (`"0.14902 0.45098 0.85098"`, floats `0..=1`; a few
/// projects store `0..=255` integers) into RGB bytes.
pub fn parse_we_color(raw: &str) -> Option<[u8; 3]> {
    let parts: Vec<f32> = raw
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|p| !p.is_empty())
        .map(str::parse::<f32>)
        .collect::<Result<_, _>>()
        .ok()?;
    if parts.len() < 3 || parts.iter().take(3).any(|v| !v.is_finite()) {
        return None;
    }
    let scale = if parts.iter().take(3).any(|v| *v > 1.0) {
        1.0
    } else {
        255.0
    };
    let byte = |v: f32| (v * scale).round().clamp(0.0, 255.0) as u8;
    Some([byte(parts[0]), byte(parts[1]), byte(parts[2])])
}

fn rgb_to_hsl([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    (h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [byte(r), byte(g), byte(b)]
}

/// The eight shades Windows keeps in `Explorer\Accent\AccentPalette`, lightest first; index 3
/// is the accent itself.
pub fn accent_palette(accent: [u8; 3]) -> [[u8; 3]; 8] {
    let (h, s, l) = rgb_to_hsl(accent);
    let shade = |dl: f32| hsl_to_rgb(h, s, (l + dl).clamp(0.0, 1.0));
    [
        shade(0.30),
        shade(0.20),
        shade(0.10),
        accent,
        shade(-0.10),
        shade(-0.20),
        shade(-0.30),
        shade(-0.40),
    ]
}

/// `AccentPalette` as stored in the registry: RGBA bytes, alpha 0.
pub fn accent_palette_bytes(accent: [u8; 3]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, [r, g, b]) in accent_palette(accent).into_iter().enumerate() {
        out[i * 4..i * 4 + 3].copy_from_slice(&[r, g, b]);
    }
    out
}

/// `0xAABBGGRR` (the byte order of `AccentColorMenu`, `StartColorMenu`, DWM `AccentColor`).
pub fn abgr([r, g, b]: [u8; 3]) -> u32 {
    0xFF00_0000 | (u32::from(b) << 16) | (u32::from(g) << 8) | u32::from(r)
}

/// `0xAARRGGBB` (DWM `ColorizationColor` / `ColorizationAfterglow`).
pub fn argb([r, g, b]: [u8; 3], alpha: u8) -> u32 {
    (u32::from(alpha) << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn we_colors_parse_as_fractions_or_bytes() {
        assert_eq!(
            parse_we_color("0.14902 0.45098 0.85098"),
            Some([38, 115, 217])
        );
        assert_eq!(parse_we_color("1 1 1"), Some([255, 255, 255]));
        assert_eq!(parse_we_color("0 0 0"), Some([0, 0, 0]));
        assert_eq!(parse_we_color("255 128 0"), Some([255, 128, 0]));
        assert_eq!(parse_we_color(" 0.5, 0.5, 0.5 "), Some([128, 128, 128]));
        assert_eq!(parse_we_color("0.5 0.5"), None);
        assert_eq!(parse_we_color("red"), None);
        assert_eq!(parse_we_color("NaN 0 0"), None);
    }

    #[test]
    fn palette_runs_light_to_dark_around_the_accent() {
        let accent = [38, 115, 217];
        let p = accent_palette(accent);
        assert_eq!(p[3], accent);
        let lum = |[r, g, b]: [u8; 3]| u32::from(r) + u32::from(g) + u32::from(b);
        for pair in p.windows(2) {
            assert!(lum(pair[0]) >= lum(pair[1]), "{p:?}");
        }
        let bytes = accent_palette_bytes(accent);
        assert_eq!(&bytes[12..16], &[38, 115, 217, 0]);
        // Grays stay gray.
        assert!(accent_palette([128, 128, 128])
            .iter()
            .all(|[r, g, b]| r == g && g == b));
    }

    #[test]
    fn registry_byte_orders() {
        assert_eq!(abgr([0x11, 0x22, 0x33]), 0xFF33_2211);
        assert_eq!(argb([0x11, 0x22, 0x33], 0xC4), 0xC411_2233);
    }
}
