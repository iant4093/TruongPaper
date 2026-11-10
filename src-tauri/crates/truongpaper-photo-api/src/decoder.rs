//! EXIF orientation-aware dimension inspector and `<= 8192px` `wgpu` texture decoder
//! (docs/02 Section 5).
//!
//! `imagesize` reads raw SOF dimensions and ignores EXIF `Orientation` 5-8, so a portrait shot
//! stored as a landscape sensor buffer would be misclassified. These helpers apply the tag, and
//! clamp decoded textures to `min(device limit, 8192)` so `wgpu::Device::create_texture` never
//! panics on stitched panoramas or high-resolution bodies.

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, RgbaImage};
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Seek};
use std::path::Path;

use crate::error::{PhotoApiError, PhotoApiResult};

/// Hard ceiling for any decoded texture edge.
pub const MAX_TEXTURE_DIMENSION: u32 = 8192;
/// Floor for the clamp, so a bogus device limit cannot shrink photos to thumbnails.
pub const MIN_TEXTURE_DIMENSION: u32 = 2048;

fn swaps_axes(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

/// Reads the image header + EXIF Orientation (1..8) from any seekable reader without decoding
/// pixels. Swaps `(width, height)` when the orientation is 5, 6, 7 or 8.
pub fn inspect_display_dimensions_from_reader<R: BufRead + Seek>(reader: R) -> Option<(u32, u32)> {
    let reader = ImageReader::new(reader).with_guessed_format().ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let (raw_w, raw_h) = decoder.dimensions();
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    if swaps_axes(orientation) {
        Some((raw_h, raw_w))
    } else {
        Some((raw_w, raw_h))
    }
}

/// [`inspect_display_dimensions_from_reader`] for a file on disk.
pub fn inspect_display_dimensions(path: &Path) -> Option<(u32, u32)> {
    let file = File::open(path).ok()?;
    inspect_display_dimensions_from_reader(BufReader::new(file))
}

/// [`inspect_display_dimensions_from_reader`] for in-memory bytes.
pub fn inspect_display_dimensions_from_bytes(bytes: &[u8]) -> Option<(u32, u32)> {
    inspect_display_dimensions_from_reader(Cursor::new(bytes))
}

/// Decoded, orientation-corrected RGBA pixels ready for GPU upload.
pub struct DecodedGpuPhoto {
    pub rgba: RgbaImage,
    pub width: u32,
    pub height: u32,
}

/// Effective edge limit for a device: `device_max.min(8192).max(2048)`.
pub fn effective_max_dimension(device_max_texture_dim_2d: u32) -> u32 {
    device_max_texture_dim_2d.clamp(MIN_TEXTURE_DIMENSION, MAX_TEXTURE_DIMENSION)
}

fn decode_from_reader<R: BufRead + Seek>(
    reader: R,
    device_max_texture_dim_2d: u32,
) -> PhotoApiResult<DecodedGpuPhoto> {
    let max_dim = effective_max_dimension(device_max_texture_dim_2d);
    let reader = ImageReader::new(reader)
        .with_guessed_format()
        .map_err(PhotoApiError::Io)?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| PhotoApiError::Decode(e.to_string()))?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);

    let mut dyn_img =
        DynamicImage::from_decoder(decoder).map_err(|e| PhotoApiError::Decode(e.to_string()))?;
    dyn_img.apply_orientation(orientation);

    let (w, h) = (dyn_img.width(), dyn_img.height());
    if w > max_dim || h > max_dim {
        dyn_img = dyn_img.resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3);
    }

    let rgba = dyn_img.into_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    Ok(DecodedGpuPhoto {
        rgba,
        width,
        height,
    })
}

/// Decodes a baseline or progressive JPEG (or any enabled format) from disk, applies EXIF
/// Orientation (1..8) and downscales with Lanczos3 when either edge exceeds
/// `device_max_texture_dim_2d.min(8192)`.
pub fn decode_photo_for_wgpu(
    path: &Path,
    device_max_texture_dim_2d: u32,
) -> PhotoApiResult<DecodedGpuPhoto> {
    let file = File::open(path)?;
    decode_from_reader(BufReader::new(file), device_max_texture_dim_2d)
}

/// [`decode_photo_for_wgpu`] for in-memory bytes.
pub fn decode_photo_bytes_for_wgpu(
    bytes: &[u8],
    device_max_texture_dim_2d: u32,
) -> PhotoApiResult<DecodedGpuPhoto> {
    decode_from_reader(Cursor::new(bytes), device_max_texture_dim_2d)
}

#[cfg(test)]
pub(crate) mod test_util {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageEncoder, RgbImage};

    /// Encodes a flat-colour JPEG of the given raw sensor dimensions.
    pub fn make_jpeg(width: u32, height: u32) -> Vec<u8> {
        let img = RgbImage::from_pixel(width, height, image::Rgb([200, 120, 40]));
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 80)
            .write_image(img.as_raw(), width, height, ExtendedColorType::Rgb8)
            .expect("encode jpeg");
        out
    }

    /// Splices a big-endian EXIF APP1 segment carrying `Orientation = orientation` right after SOI.
    pub fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "not a JPEG");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"Exif\0\0");
        payload.extend_from_slice(&[b'M', b'M', 0, 42, 0, 0, 0, 8]); // TIFF header, IFD0 at 8
        payload.extend_from_slice(&[0, 1]); // one entry
        payload.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1]); // tag 0x0112, SHORT, count 1
        payload.extend_from_slice(&orientation.to_be_bytes());
        payload.extend_from_slice(&[0, 0]); // value padding
        payload.extend_from_slice(&[0, 0, 0, 0]); // no next IFD
        let len = (payload.len() + 2) as u16;
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&payload);
        out.extend_from_slice(&jpeg[2..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;

    #[test]
    fn plain_jpeg_reports_raw_dimensions() {
        let jpeg = make_jpeg(64, 48);
        assert_eq!(inspect_display_dimensions_from_bytes(&jpeg), Some((64, 48)));
    }

    #[test]
    fn exif_orientations_1_to_8_swap_axes_only_for_5_to_8() {
        let base = make_jpeg(64, 48);
        for orientation in 1..=8u16 {
            let jpeg = with_exif_orientation(&base, orientation);
            let expected = if orientation >= 5 { (48, 64) } else { (64, 48) };
            assert_eq!(
                inspect_display_dimensions_from_bytes(&jpeg),
                Some(expected),
                "EXIF orientation {orientation}"
            );
        }
    }

    #[test]
    fn garbage_bytes_yield_none() {
        assert_eq!(inspect_display_dimensions_from_bytes(b"not an image"), None);
        assert_eq!(inspect_display_dimensions_from_bytes(&[]), None);
        assert!(inspect_display_dimensions(Path::new("/definitely/not/here.jpg")).is_none());
    }

    #[test]
    fn inspect_reads_from_disk() {
        let jpeg = with_exif_orientation(&make_jpeg(40, 20), 6);
        let path =
            std::env::temp_dir().join(format!("truongpaper-decoder-{}.jpg", std::process::id()));
        std::fs::write(&path, &jpeg).unwrap();
        assert_eq!(inspect_display_dimensions(&path), Some((20, 40)));
        let decoded = decode_photo_for_wgpu(&path, 8192).unwrap();
        assert_eq!((decoded.width, decoded.height), (20, 40));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn decode_applies_orientation_to_pixels() {
        let jpeg = with_exif_orientation(&make_jpeg(64, 48), 6);
        let decoded = decode_photo_bytes_for_wgpu(&jpeg, 8192).unwrap();
        assert_eq!((decoded.width, decoded.height), (48, 64));
        assert_eq!(decoded.rgba.dimensions(), (48, 64));
        let upright = decode_photo_bytes_for_wgpu(&make_jpeg(64, 48), 8192).unwrap();
        assert_eq!((upright.width, upright.height), (64, 48));
    }

    #[test]
    fn decode_downscales_to_device_limit_keeping_aspect_ratio() {
        // device limit 2048 is the floor, so build an image larger than that.
        let jpeg = with_exif_orientation(&make_jpeg(4096, 1024), 8);
        let decoded = decode_photo_bytes_for_wgpu(&jpeg, 2048).unwrap();
        // Rotated to 1024x4096, then clamped so the long edge is 2048.
        assert_eq!((decoded.width, decoded.height), (512, 2048));
    }

    #[test]
    fn small_images_are_not_resized() {
        let decoded = decode_photo_bytes_for_wgpu(&make_jpeg(100, 50), 8192).unwrap();
        assert_eq!((decoded.width, decoded.height), (100, 50));
    }

    #[test]
    fn effective_max_dimension_clamps_between_2048_and_8192() {
        assert_eq!(effective_max_dimension(16384), 8192);
        assert_eq!(effective_max_dimension(8192), 8192);
        assert_eq!(effective_max_dimension(4096), 4096);
        assert_eq!(effective_max_dimension(512), 2048);
        assert_eq!(effective_max_dimension(0), 2048);
    }

    #[test]
    fn decode_garbage_is_an_error_not_a_panic() {
        assert!(matches!(
            decode_photo_bytes_for_wgpu(b"garbage", 8192),
            Err(PhotoApiError::Decode(_)) | Err(PhotoApiError::Io(_))
        ));
        assert!(decode_photo_for_wgpu(Path::new("/definitely/not/here.jpg"), 8192).is_err());
    }
}
