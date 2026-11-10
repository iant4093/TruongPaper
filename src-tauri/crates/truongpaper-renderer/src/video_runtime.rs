//! Video runtime (docs/05 Phase 5, docs/03 §6.2).
//!
//! Standalone `"type": "video"` wallpapers are played by the OS webview through
//! `wpe://localhost/__video_host__.html` (see [`crate::web_runtime::video_host_url`]); WKWebView
//! decodes with VideoToolbox and WebView2 with D3D11VA. This module holds the device-free
//! playback rules shared with that page (speed clamp, mute/volume), a drift-free loop clock, the
//! embedded `.tex` MP4 (`free_image_format == 35`) cache extraction, and the CPU-frame upload path
//! into a `wgpu::Texture` used by scene layers whose texture is an embedded MP4.

use std::borrow::Cow;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use truongpaper_we_formats::ParsedTex;

/// Slowest playback rate exposed to the user (docs/05 Phase 5).
pub const MIN_PLAYBACK_SPEED: f32 = 0.1;
/// Fastest playback rate exposed to the user (docs/05 Phase 5).
pub const MAX_PLAYBACK_SPEED: f32 = 3.0;
/// Volumes at or below this are treated as muted (same rule as `video-wallpaper-host.html`).
pub const MUTE_VOLUME_THRESHOLD: f32 = 0.001;

/// Clamps a playback rate to `0.1..=3.0`. NaN and infinities fall back to `1.0`.
pub fn clamp_playback_speed(speed: f32) -> f32 {
    if speed.is_finite() {
        speed.clamp(MIN_PLAYBACK_SPEED, MAX_PLAYBACK_SPEED)
    } else {
        1.0
    }
}

/// Clamps a volume to `0..=1`. NaN becomes `0.0` (silent).
pub fn clamp_volume(volume: f32) -> f32 {
    if volume.is_nan() {
        0.0
    } else {
        volume.clamp(0.0, 1.0)
    }
}

/// Per-monitor playback settings for video sources (from `UniversalAdjustments`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoPlaybackSettings {
    pub speed: f32,
    pub volume: f32,
    pub muted: bool,
}

impl Default for VideoPlaybackSettings {
    fn default() -> Self {
        Self {
            speed: 1.0,
            volume: 1.0,
            muted: false,
        }
    }
}

impl VideoPlaybackSettings {
    /// Returns a copy with speed and volume clamped to their valid ranges.
    pub fn clamped(self) -> Self {
        Self {
            speed: clamp_playback_speed(self.speed),
            volume: clamp_volume(self.volume),
            muted: self.muted,
        }
    }

    /// `true` when the player should be muted: explicitly muted or volume `<= 0.001`.
    pub fn is_effectively_muted(&self) -> bool {
        // `volume.is_nan()` also counts as muted.
        self.muted || self.volume.is_nan() || self.volume <= MUTE_VOLUME_THRESHOLD
    }

    /// Volume to hand to the decoder: `0.0` when muted, otherwise the clamped volume.
    pub fn effective_volume(&self) -> f32 {
        if self.is_effectively_muted() {
            0.0
        } else {
            clamp_volume(self.volume)
        }
    }
}

/// Gapless loop clock for a video of known duration. Positions are kept in `f64` and wrapped with
/// a modulo so long sessions do not accumulate drift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoLoopClock {
    pub duration_secs: f64,
    pub position_secs: f64,
    pub paused: bool,
}

impl VideoLoopClock {
    pub fn new(duration_secs: f64) -> Self {
        Self {
            duration_secs,
            position_secs: 0.0,
            paused: false,
        }
    }

    fn has_valid_duration(&self) -> bool {
        self.duration_secs.is_finite() && self.duration_secs > 0.0
    }

    /// Advances by `dt_secs * clamp_playback_speed(speed)` and returns how many times playback
    /// wrapped back to the start. Paused clocks, non-positive or non-finite `dt`, and clocks with
    /// a duration `<= 0` do not move (position is pinned to `0` for an invalid duration).
    pub fn advance(&mut self, dt_secs: f64, speed: f32) -> u32 {
        if !self.has_valid_duration() {
            self.position_secs = 0.0;
            return 0;
        }
        if self.paused || !dt_secs.is_finite() || dt_secs <= 0.0 {
            return 0;
        }
        let advanced =
            self.position_secs.max(0.0) + dt_secs * f64::from(clamp_playback_speed(speed));
        let wraps = (advanced / self.duration_secs).floor();
        let mut position = advanced - wraps * self.duration_secs;
        // Guard against `advanced` landing a hair below a multiple of the duration.
        if position >= self.duration_secs || position < 0.0 {
            position = position.rem_euclid(self.duration_secs);
        }
        self.position_secs = position;
        if wraps >= f64::from(u32::MAX) {
            u32::MAX
        } else {
            wraps as u32
        }
    }

    /// Frame number at the current position for a video running at `fps`. Returns `0` for an
    /// invalid frame rate or duration.
    pub fn frame_index(&self, fps: f64) -> u64 {
        if !self.has_valid_duration() || !fps.is_finite() || fps <= 0.0 {
            return 0;
        }
        (self.position_secs.max(0.0) * fps).floor() as u64
    }

    /// Seeks back to the start (used when a new wallpaper replaces the current one).
    pub fn reset(&mut self) {
        self.position_secs = 0.0;
    }
}

// ---------------------------------------------------------------------------------------------
// Embedded `.tex` MP4 (`free_image_format == 35`)
// ---------------------------------------------------------------------------------------------

/// `true` when `bytes` start with an ISO-BMFF `ftyp` box (`bytes[4..8] == b"ftyp"`).
pub fn is_mp4_container(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && &bytes[4..8] == b"ftyp"
}

/// `<wallpaper_dir>/.cache/<sanitized name>.mp4` for a `.tex` virtual path such as
/// `materials/videos/clip.tex` (-> `materials_videos_clip.mp4`).
///
/// The `.tex` extension is stripped, `/` and `\` become `_`, and `.`/empty segments are dropped.
/// Returns `None` for an empty name or when any segment is `..`.
pub fn embedded_mp4_cache_path(wallpaper_dir: &Path, tex_virtual_path: &str) -> Option<PathBuf> {
    let mut segments = Vec::new();
    for seg in tex_virtual_path.split(['/', '\\']) {
        let seg = seg.trim();
        match seg {
            "" | "." => continue,
            ".." => return None,
            s => segments.push(s),
        }
    }
    let mut name = segments.join("_");
    if name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(".tex") {
        name.truncate(name.len() - 4);
    }
    // Keep the file name portable (Windows reserves these characters).
    let name: String = name
        .chars()
        .map(|c| match c {
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if name.is_empty() || name.chars().all(|c| c == '.' || c == '_') {
        return None;
    }
    Some(wallpaper_dir.join(".cache").join(format!("{name}.mp4")))
}

/// Writes the MP4 container embedded in an `FIF_MP4` `.tex` to
/// `<wallpaper_dir>/.cache/<name>.mp4` (docs/03 §6.2 item 2) and returns its path.
///
/// Returns `Ok(None)` when the texture is not an MP4 or its first mipmap is not an `ftyp`
/// container. Reuse requires matching contents; Workshop updates can preserve the file length.
/// Writes go through a `.mp4.part` temp file followed by a rename so a crash never leaves a
/// truncated cache file behind.
pub fn extract_embedded_tex_mp4(
    tex: &ParsedTex,
    wallpaper_dir: &Path,
    tex_virtual_path: &str,
) -> io::Result<Option<PathBuf>> {
    if !tex.is_mp4() {
        return Ok(None);
    }
    let Some(bytes) = tex
        .images
        .first()
        .and_then(|mips| mips.first())
        .map(|mip| mip.raw_bytes.as_slice())
    else {
        return Ok(None);
    };
    if !is_mp4_container(bytes) {
        return Ok(None);
    }
    let path = embedded_mp4_cache_path(wallpaper_dir, tex_virtual_path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid embedded video texture path: {tex_virtual_path}"),
        )
    })?;

    if cached_video_matches(&path, bytes) {
        return Ok(Some(path));
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let part = path.with_extension("mp4.part");
    if let Err(err) = fs::write(&part, bytes).and_then(|_| fs::rename(&part, &path)) {
        let _ = fs::remove_file(&part);
        return Err(err);
    }
    tracing::debug!(path = %path.display(), bytes = bytes.len(), "extracted embedded tex mp4");
    Ok(Some(path))
}

/// Compare in bounded chunks rather than allocating a second copy of a large MP4.
fn cached_video_matches(path: &Path, bytes: &[u8]) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    if !file
        .metadata()
        .is_ok_and(|meta| meta.is_file() && meta.len() == bytes.len() as u64)
    {
        return false;
    }
    let mut buffer = [0u8; 64 * 1024];
    for chunk in bytes.chunks(buffer.len()) {
        if file.read_exact(&mut buffer[..chunk.len()]).is_err() || buffer[..chunk.len()] != *chunk {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------------------------
// Decoded frames -> wgpu texture
// ---------------------------------------------------------------------------------------------

/// Pixel layout of a decoded CPU frame (4 bytes per pixel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPixelFormat {
    /// `kCVPixelFormatType_32BGRA` (AVFoundation) / `MFVideoFormat_ARGB32` (Media Foundation).
    Bgra8,
    Rgba8,
}

impl VideoPixelFormat {
    pub fn wgpu_format(self) -> wgpu::TextureFormat {
        match self {
            VideoPixelFormat::Bgra8 => wgpu::TextureFormat::Bgra8UnormSrgb,
            VideoPixelFormat::Rgba8 => wgpu::TextureFormat::Rgba8UnormSrgb,
        }
    }
}

/// One decoded video frame. `stride` is the source row pitch in bytes (`>= width * 4`; decoders
/// usually pad rows to 16/64 bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: VideoPixelFormat,
    pub data: Vec<u8>,
}

impl VideoFrame {
    /// Bytes per tightly packed row (`width * 4`).
    pub fn row_bytes(&self) -> usize {
        self.width as usize * 4
    }

    /// `true` when the dimensions are non-zero and `data` holds every row at `stride`.
    pub fn is_valid(&self) -> bool {
        let row = self.row_bytes();
        if self.width == 0 || self.height == 0 || self.stride < row {
            return false;
        }
        let needed = self
            .stride
            .checked_mul(self.height as usize - 1)
            .and_then(|n| n.checked_add(row));
        matches!(needed, Some(n) if self.data.len() >= n)
    }
}

/// Returns the frame's pixels with row padding removed (`width * 4 * height` bytes). Borrows when
/// the rows are already tightly packed. Invalid frames (see [`VideoFrame::is_valid`]) yield an
/// empty slice.
pub fn tightly_packed_rows(frame: &VideoFrame) -> Cow<'_, [u8]> {
    if !frame.is_valid() {
        return Cow::Borrowed(&[]);
    }
    let row = frame.row_bytes();
    let height = frame.height as usize;
    if frame.stride == row {
        return Cow::Borrowed(&frame.data[..row * height]);
    }
    let mut packed = Vec::with_capacity(row * height);
    for y in 0..height {
        let start = y * frame.stride;
        packed.extend_from_slice(&frame.data[start..start + row]);
    }
    Cow::Owned(packed)
}

/// Platform video decoder producing CPU frames (macOS: `truongpaper_platform::mac_video`).
pub trait VideoFrameSource {
    /// Newest frame decoded since the last call, if any.
    fn latest_frame(&mut self) -> Option<VideoFrame>;
    fn apply_settings(&mut self, settings: VideoPlaybackSettings);
    fn set_paused(&mut self, paused: bool);
}

/// GPU texture fed by a [`VideoFrameSource`]. Recreated whenever the frame size or format
/// changes.
pub struct VideoTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    format: VideoPixelFormat,
}

impl VideoTexture {
    pub fn new(device: &wgpu::Device, width: u32, height: u32, format: VideoPixelFormat) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("truongpaper-video-texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: format.wgpu_format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            width,
            height,
            format,
        }
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn format(&self) -> VideoPixelFormat {
        self.format
    }

    /// Uploads `frame`. Returns `true` when the texture was recreated (bind groups that reference
    /// [`Self::view`] must be rebuilt). Invalid frames are ignored.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &VideoFrame,
    ) -> bool {
        if !frame.is_valid() {
            return false;
        }
        let recreated = if (frame.width, frame.height) != (self.width, self.height)
            || frame.format != self.format
        {
            *self = Self::new(device, frame.width, frame.height, frame.format);
            true
        } else {
            false
        };
        let pixels = tightly_packed_rows(frame);
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(frame.width * 4),
                rows_per_image: Some(frame.height),
            },
            wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
        );
        recreated
    }

    /// Pulls the newest frame from `source` and uploads it. Returns `true` when the texture was
    /// recreated.
    pub fn update_from_source(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &mut dyn VideoFrameSource,
    ) -> bool {
        match source.latest_frame() {
            Some(frame) => self.upload(device, queue, &frame),
            None => false,
        }
    }
}
