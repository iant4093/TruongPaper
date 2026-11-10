//! Real-machine check for `truongpaper_platform::mac_video::MacVideoDecoder`.
//!
//! ```bash
//! cargo run -p truongpaper-platform --example mac_video_probe -- /path/to/clip.mp4
//! ```
//!
//! AVPlayer drives its state machine on the main dispatch queue, so this runs on the main thread
//! and pumps the main run loop (libtest runs tests off the main thread, which is why this is an
//! example and not a unit test). Prints `PASS` after 10 decoded BGRA frames.

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop};
    use std::time::{Duration, Instant};
    use truongpaper_platform::mac_video::MacVideoDecoder;

    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: mac_video_probe <file.mp4>"))?;
    let mut decoder = MacVideoDecoder::open(std::path::Path::new(&path))?;
    decoder.set_muted(true);
    decoder.play();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut frames = 0;
    while Instant::now() < deadline && frames < 10 {
        if let Some((w, h, stride, bytes)) = decoder.copy_latest_bgra_frame() {
            println!(
                "frame {frames}: {w}x{h} stride {stride} bytes {}",
                bytes.len()
            );
            frames += 1;
        }
        // SAFETY: `kCFRunLoopDefaultMode` is an immutable CoreFoundation constant.
        let mode = unsafe { kCFRunLoopDefaultMode };
        CFRunLoop::run_in_mode(mode, Duration::from_millis(16), false);
    }
    if frames == 0 {
        anyhow::bail!(
            "no frames decoded (item status {:?}, error {:?})",
            decoder.item_status(),
            decoder.item_error()
        );
    }
    println!("PASS");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("mac_video_probe only runs on macOS");
}
