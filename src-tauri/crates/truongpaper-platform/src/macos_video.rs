//! macOS AVFoundation decoder for embedded `.tex` MP4 video textures
//! (`free_image_format == 35`, docs/03 §6.2 item 2; docs/05 Phase 5 "Video Runtime").
//!
//! Compiled as `crate::mac_video` (macOS only). The renderer extracts the MP4 to
//! `<wallpaper_dir>/.cache/<name>.mp4`; this module plays that file with `AVPlayer` (hardware
//! decode through VideoToolbox) and reads BGRA frames from an `AVPlayerItemVideoOutput`
//! configured with `kCVPixelFormatType_32BGRA`. Looping seeks back to zero on
//! `AVPlayerItemDidPlayToEndTimeNotification` with `actionAtItemEnd = None`, so playback never
//! stops at the end of the item.
//!
//! Standalone `"type": "video"` wallpapers do not use this module; they are played by WKWebView
//! through `wpe://localhost/__video_host__.html`.
//!
//! `truongpaper-platform` does not depend on `truongpaper-renderer`, so frames are returned as
//! plain `(width, height, stride, bytes)` tuples; the app glue adapts them to the renderer's
//! `VideoFrameSource`.

use std::ffi::c_void;
use std::marker::{PhantomData, PhantomPinned};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use block2::RcBlock;
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::{NSNumber, NSString};

/// Same range as the renderer's `MIN_PLAYBACK_SPEED..=MAX_PLAYBACK_SPEED` (docs/05 Phase 5).
pub const MIN_VIDEO_RATE: f32 = 0.1;
pub const MAX_VIDEO_RATE: f32 = 2.0;

/// `kCVPixelFormatType_32BGRA` (`'BGRA'`).
pub const K_CV_PIXEL_FORMAT_TYPE_32BGRA: u32 = 0x4247_5241;
/// `kCVPixelBufferLock_ReadOnly`.
const K_CV_PIXEL_BUFFER_LOCK_READ_ONLY: u64 = 1;
/// `kCVReturnSuccess`.
const K_CV_RETURN_SUCCESS: i32 = 0;
/// `AVPlayerActionAtItemEndNone`.
const AV_PLAYER_ACTION_AT_ITEM_END_NONE: isize = 2;
/// `kCMTimeFlags_Valid`.
const K_CM_TIME_FLAGS_VALID: u32 = 1;

/// CoreMedia `CMTime` (`{?=qiIq}`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CMTime {
    pub value: i64,
    pub timescale: i32,
    pub flags: u32,
    pub epoch: i64,
}

impl CMTime {
    pub fn is_valid(&self) -> bool {
        self.flags & K_CM_TIME_FLAGS_VALID != 0
    }
}

// SAFETY: `CMTime` is `#[repr(C)]` and matches the CoreMedia definition field for field.
unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING],
    );
}

// SAFETY: pointer to the struct above.
unsafe impl RefEncode for CMTime {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

/// Opaque CoreVideo `__CVBuffer` (`CVPixelBufferRef` points to it).
#[repr(C)]
pub struct CVBuffer {
    _inner: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

// SAFETY: matches the `^{__CVBuffer=}` encoding used by AVFoundation method signatures.
unsafe impl RefEncode for CVBuffer {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__CVBuffer", &[]));
}

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVPlayerItemDidPlayToEndTimeNotification: *const AnyObject;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    static kCMTimeZero: CMTime;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    /// `CFStringRef`, toll-free bridged to `NSString`.
    static kCVPixelBufferPixelFormatTypeKey: *const AnyObject;
    fn CVPixelBufferLockBaseAddress(buffer: *mut CVBuffer, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: *mut CVBuffer, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: *mut CVBuffer) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(buffer: *mut CVBuffer) -> usize;
    fn CVPixelBufferGetWidth(buffer: *mut CVBuffer) -> usize;
    fn CVPixelBufferGetHeight(buffer: *mut CVBuffer) -> usize;
    fn CVPixelBufferGetDataSize(buffer: *mut CVBuffer) -> usize;
    fn CVPixelBufferGetPixelFormatType(buffer: *mut CVBuffer) -> u32;
    fn CVPixelBufferIsPlanar(buffer: *mut CVBuffer) -> u8;
    fn CVBufferRelease(buffer: *mut CVBuffer);
}

extern "C" {
    fn mach_absolute_time() -> u64;
}

/// Clamps a playback rate to `0.1..=2.0`; non-finite values fall back to `1.0`.
pub fn clamp_video_rate(rate: f32) -> f32 {
    if rate.is_finite() {
        rate.clamp(MIN_VIDEO_RATE, MAX_VIDEO_RATE)
    } else {
        1.0
    }
}

/// Clamps a volume to `0..=1`; NaN becomes `0.0`.
pub fn clamp_video_volume(volume: f32) -> f32 {
    if volume.is_nan() {
        0.0
    } else {
        volume.clamp(0.0, 1.0)
    }
}

/// `(width, height, stride, bytes)` of a BGRA frame. `bytes.len() >= stride * (height - 1) + width * 4`.
pub type BgraFrame = (u32, u32, usize, Vec<u8>);

/// `AVPlayerItemStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacVideoItemStatus {
    Unknown,
    ReadyToPlay,
    Failed,
}

/// Looping `AVPlayer` over a local MP4 file with a BGRA `AVPlayerItemVideoOutput`.
///
/// Created paused at rate `1.0`; call [`MacVideoDecoder::play`] to start.
pub struct MacVideoDecoder {
    player: Retained<AnyObject>,
    item: Retained<AnyObject>,
    output: Retained<AnyObject>,
    end_observer: Option<Retained<AnyObject>>,
    rate: f32,
    paused: bool,
}

// SAFETY: AVPlayer, AVPlayerItem and AVPlayerItemVideoOutput are reference counted Objective-C
// objects with atomic retain/release. The decoder only issues playback control messages
// (`setRate:`, `pause`, `setVolume:`, `setMuted:`) and `AVPlayerItemVideoOutput` pixel-buffer
// queries, which AVFoundation allows from any thread (frames are typically pulled from a render
// or display-link thread). The struct is not `Sync`, so calls are never concurrent.
unsafe impl Send for MacVideoDecoder {}

impl MacVideoDecoder {
    /// Opens `path` (an extracted `.cache/<name>.mp4`). The file must exist; decoding errors
    /// inside AVFoundation surface as an absence of frames.
    pub fn open(path: &Path) -> Result<Self> {
        let meta = std::fs::metadata(path)
            .with_context(|| format!("video file not found: {}", path.display()))?;
        if !meta.is_file() {
            bail!("video path is not a file: {}", path.display());
        }
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow!("video path is not valid UTF-8: {}", path.display()))?;
        let ns_path = NSString::from_str(path_str);

        // SAFETY: every message below targets a non-nil object (checked), uses the documented
        // selector signature, and every returned object is held as `Retained`.
        unsafe {
            let url: Option<Retained<AnyObject>> =
                msg_send![class!(NSURL), fileURLWithPath: &*ns_path];
            let url = url.ok_or_else(|| anyhow!("NSURL fileURLWithPath failed"))?;

            let item: Option<Retained<AnyObject>> =
                msg_send![class!(AVPlayerItem), playerItemWithURL: &*url];
            let item = item.ok_or_else(|| anyhow!("AVPlayerItem playerItemWithURL failed"))?;

            let format_key = kCVPixelBufferPixelFormatTypeKey;
            if format_key.is_null() {
                bail!("kCVPixelBufferPixelFormatTypeKey is unavailable");
            }
            let format_value = NSNumber::new_u32(K_CV_PIXEL_FORMAT_TYPE_32BGRA);
            let attributes: Option<Retained<AnyObject>> = msg_send![
                class!(NSDictionary),
                dictionaryWithObject: &*format_value,
                forKey: &*format_key
            ];
            let attributes =
                attributes.ok_or_else(|| anyhow!("pixel buffer attribute dictionary failed"))?;

            let output_alloc: Allocated<AnyObject> =
                msg_send![class!(AVPlayerItemVideoOutput), alloc];
            let output: Option<Retained<AnyObject>> =
                msg_send![output_alloc, initWithPixelBufferAttributes: &*attributes];
            let output = output.ok_or_else(|| anyhow!("AVPlayerItemVideoOutput init failed"))?;
            let _: () = msg_send![&*item, addOutput: &*output];

            let player: Option<Retained<AnyObject>> =
                msg_send![class!(AVPlayer), playerWithPlayerItem: &*item];
            let player = player.ok_or_else(|| anyhow!("AVPlayer playerWithPlayerItem failed"))?;
            let _: () = msg_send![&*player, setActionAtItemEnd: AV_PLAYER_ACTION_AT_ITEM_END_NONE];
            // A wallpaper must never keep the display awake.
            let _: () = msg_send![&*player, setPreventsDisplaySleepDuringVideoPlayback: false];
            let _: () = msg_send![&*player, pause];

            // Gapless loop: seek back to zero when the item reaches its end.
            let end_observer = {
                let name = AVPlayerItemDidPlayToEndTimeNotification;
                if name.is_null() {
                    None
                } else {
                    let center: Retained<AnyObject> =
                        msg_send![class!(NSNotificationCenter), defaultCenter];
                    let loop_player = player.clone();
                    let block = RcBlock::new(move |_note: *mut AnyObject| {
                        let _: () = msg_send![&*loop_player, seekToTime: kCMTimeZero];
                    });
                    let no_queue: *mut AnyObject = std::ptr::null_mut();
                    let token: Option<Retained<AnyObject>> = msg_send![
                        &*center,
                        addObserverForName: &*name,
                        object: &*item,
                        queue: no_queue,
                        usingBlock: &*block
                    ];
                    token
                }
            };

            tracing::debug!(path = %path.display(), "opened AVFoundation video decoder");
            Ok(Self {
                player,
                item,
                output,
                end_observer,
                rate: 1.0,
                paused: true,
            })
        }
    }

    /// Copies the newest decoded frame if the output has one for the current host time.
    pub fn copy_latest_bgra_frame(&mut self) -> Option<BgraFrame> {
        // SAFETY: `output` is a live AVPlayerItemVideoOutput; the returned pixel buffer is +1
        // (copy rule) and released below after its base address is unlocked.
        unsafe {
            let host_time = mach_absolute_time() as i64;
            let item_time: CMTime =
                msg_send![&*self.output, itemTimeForMachAbsoluteTime: host_time];
            if !item_time.is_valid() {
                return None;
            }
            let has_new: bool = msg_send![&*self.output, hasNewPixelBufferForItemTime: item_time];
            if !has_new {
                return None;
            }
            let no_display_time: *mut CMTime = std::ptr::null_mut();
            let buffer: *mut CVBuffer = msg_send![
                &*self.output,
                copyPixelBufferForItemTime: item_time,
                itemTimeForDisplay: no_display_time
            ];
            if buffer.is_null() {
                return None;
            }
            let frame = read_bgra_pixel_buffer(buffer);
            CVBufferRelease(buffer);
            frame
        }
    }

    /// Sets the playback rate (clamped to `0.1..=2.0`). Applied immediately while playing.
    pub fn set_rate(&mut self, rate: f32) {
        self.rate = clamp_video_rate(rate);
        if !self.paused {
            // SAFETY: `player` is a live AVPlayer; `setRate:` takes a `float`.
            unsafe {
                let _: () = msg_send![&*self.player, setRate: self.rate];
            }
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        // SAFETY: `player` is a live AVPlayer; `setVolume:` takes a `float`.
        unsafe {
            let _: () = msg_send![&*self.player, setVolume: clamp_video_volume(volume)];
        }
    }

    pub fn set_muted(&mut self, muted: bool) {
        // SAFETY: `player` is a live AVPlayer; `setMuted:` takes a `BOOL`.
        unsafe {
            let _: () = msg_send![&*self.player, setMuted: muted];
        }
    }

    pub fn play(&mut self) {
        self.paused = false;
        // SAFETY: `player` is a live AVPlayer; a non-zero rate starts playback.
        unsafe {
            let _: () = msg_send![&*self.player, setRate: self.rate];
        }
    }

    pub fn pause(&mut self) {
        self.paused = true;
        // SAFETY: `player` is a live AVPlayer.
        unsafe {
            let _: () = msg_send![&*self.player, pause];
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// `AVPlayerItem.status`: lets callers detect files AVFoundation cannot decode.
    pub fn item_status(&self) -> MacVideoItemStatus {
        // SAFETY: `item` is a live AVPlayerItem; `status` returns an NSInteger enum.
        let raw: isize = unsafe { msg_send![&*self.item, status] };
        match raw {
            1 => MacVideoItemStatus::ReadyToPlay,
            2 => MacVideoItemStatus::Failed,
            _ => MacVideoItemStatus::Unknown,
        }
    }

    /// `AVPlayerItem.error.localizedDescription` once the item has failed.
    pub fn item_error(&self) -> Option<String> {
        // SAFETY: `item` is a live AVPlayerItem; `error` is a nullable NSError and
        // `localizedDescription` a non-null NSString.
        unsafe {
            let error: Option<Retained<AnyObject>> = msg_send![&*self.item, error];
            let error = error?;
            let description: Option<Retained<NSString>> = msg_send![&*error, description];
            description.map(|d| d.to_string())
        }
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }
}

impl Drop for MacVideoDecoder {
    fn drop(&mut self) {
        // SAFETY: plain messages to live objects; removing the observer releases the loop block
        // (and its player reference) before the player itself is released.
        unsafe {
            if let Some(token) = self.end_observer.take() {
                let center: Retained<AnyObject> =
                    msg_send![class!(NSNotificationCenter), defaultCenter];
                let _: () = msg_send![&*center, removeObserver: &*token];
            }
            let _: () = msg_send![&*self.player, pause];
            let _: () = msg_send![&*self.item, removeOutput: &*self.output];
            let no_item: *mut AnyObject = std::ptr::null_mut();
            let _: () = msg_send![&*self.player, replaceCurrentItemWithPlayerItem: no_item];
        }
    }
}

/// Copies a non-planar BGRA `CVPixelBuffer` into `(width, height, stride, bytes)`.
///
/// # Safety
/// `buffer` must be a valid `CVPixelBufferRef`.
unsafe fn read_bgra_pixel_buffer(buffer: *mut CVBuffer) -> Option<BgraFrame> {
    if CVPixelBufferIsPlanar(buffer) != 0
        || CVPixelBufferGetPixelFormatType(buffer) != K_CV_PIXEL_FORMAT_TYPE_32BGRA
    {
        return None;
    }
    if CVPixelBufferLockBaseAddress(buffer, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY) != K_CV_RETURN_SUCCESS
    {
        return None;
    }
    let width = CVPixelBufferGetWidth(buffer);
    let height = CVPixelBufferGetHeight(buffer);
    let stride = CVPixelBufferGetBytesPerRow(buffer);
    let base = CVPixelBufferGetBaseAddress(buffer) as *const u8;
    let len = stride
        .checked_mul(height)
        .map(|n| n.min(CVPixelBufferGetDataSize(buffer)));
    let frame = match len {
        Some(len)
            if !base.is_null()
                && width > 0
                && height > 0
                && stride >= width * 4
                && len >= stride * (height - 1) + width * 4 =>
        {
            let bytes = std::slice::from_raw_parts(base, len).to_vec();
            Some((width as u32, height as u32, stride, bytes))
        }
        _ => None,
    };
    CVPixelBufferUnlockBaseAddress(buffer, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_and_volume_clamps() {
        assert_eq!(clamp_video_rate(0.01), 0.1);
        assert_eq!(clamp_video_rate(2.0), 2.0);
        assert_eq!(clamp_video_rate(10.0), 2.0);
        assert_eq!(clamp_video_rate(f32::NAN), 1.0);
        assert_eq!(clamp_video_volume(-1.0), 0.0);
        assert_eq!(clamp_video_volume(0.3), 0.3);
        assert_eq!(clamp_video_volume(2.0), 1.0);
        assert_eq!(clamp_video_volume(f32::NAN), 0.0);
    }

    #[test]
    fn cmtime_layout_matches_coremedia() {
        assert_eq!(std::mem::size_of::<CMTime>(), 24);
        assert!(CMTime::ENCODING.equivalent_to_str("{?=qiIq}"));
        assert!(CVBuffer::ENCODING_REF.equivalent_to_str("^{__CVBuffer=}"));
    }

    #[test]
    fn open_rejects_missing_files() {
        let missing = std::env::temp_dir().join("truongpaper-mac-video-missing-file.mp4");
        let _ = std::fs::remove_file(&missing);
        assert!(MacVideoDecoder::open(&missing).is_err());
        assert!(MacVideoDecoder::open(&std::env::temp_dir()).is_err());
    }

    /// Exercises every Objective-C message signature headlessly (no window, no network): the
    /// file is not a decodable MP4, so AVFoundation fails asynchronously and yields no frames.
    #[test]
    fn decoder_messages_have_valid_signatures() {
        let path =
            std::env::temp_dir().join(format!("truongpaper-mac-video-{}.mp4", std::process::id()));
        std::fs::write(&path, b"\0\0\0\x18ftypisom-not-a-real-movie").unwrap();
        {
            let mut decoder = MacVideoDecoder::open(&path).expect("AVPlayer created");
            assert!(decoder.is_paused());
            decoder.set_rate(5.0);
            assert_eq!(decoder.rate(), 2.0);
            decoder.set_volume(0.5);
            decoder.set_muted(true);
            decoder.play();
            assert!(!decoder.is_paused());
            decoder.set_rate(0.5);
            assert!(decoder.copy_latest_bgra_frame().is_none());
            decoder.pause();
            assert!(decoder.is_paused());
        }
        let _ = std::fs::remove_file(&path);
    }
}
