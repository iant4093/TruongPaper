//! Real-machine check for `truongpaper_platform::win_video::WinVideoDecoder`.
//!
//! ```powershell
//! cargo run -p truongpaper-platform --example win_video_probe -- C:\path\to\clip.mp4
//! cargo run -p truongpaper-platform --example win_video_probe   # encodes a 320x240 H.264 clip
//! ```
//!
//! Without an argument the probe encodes a small H.264 clip with Media Foundation's sink writer
//! (skipping with a message when no H.264 encoder is installed). Prints `PASS` after 10 decoded
//! BGRA frames with non-zero pixels.

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    use std::time::{Duration, Instant};
    use truongpaper_platform::win_video::{write_test_clip_h264, WinVideoDecoder};

    let (path, generated) = match std::env::args().nth(1) {
        Some(path) => (std::path::PathBuf::from(path), false),
        None => {
            let path = std::env::temp_dir().join("truongpaper-win-video-probe.mp4");
            if let Err(err) = write_test_clip_h264(&path, 320, 240, 90) {
                println!("SKIP: cannot encode an H.264 test clip on this machine: {err:#}");
                return Ok(());
            }
            println!("encoded {}", path.display());
            (path, true)
        }
    };

    let mut decoder = WinVideoDecoder::open(&path)?;
    decoder.set_muted(true);
    decoder.play();

    let started = Instant::now();
    let deadline = started + Duration::from_secs(10);
    let mut frames = 0;
    let mut buf = Vec::new();
    while Instant::now() < deadline && frames < 10 {
        if let Some((w, h, stride)) = decoder.copy_latest_bgra_frame_into(&mut buf) {
            let lit = (0..buf.len() / 4)
                .filter(|i| buf[i * 4] | buf[i * 4 + 1] | buf[i * 4 + 2] != 0)
                .count();
            println!(
                "frame {frames} at {:>4} ms: {w}x{h} stride {stride} bytes {} non-black px {lit} first px {:?}",
                started.elapsed().as_millis(),
                buf.len(),
                &buf[..4]
            );
            if lit == 0 {
                anyhow::bail!("frame {frames} is entirely black");
            }
            frames += 1;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    println!("status {:?}", decoder.status());
    if frames < 10 {
        anyhow::bail!("only {frames} frames decoded");
    }

    // Pause must stop frame delivery.
    decoder.pause();
    std::thread::sleep(Duration::from_millis(100));
    let _ = decoder.copy_latest_bgra_frame_into(&mut buf);
    std::thread::sleep(Duration::from_millis(400));
    let paused_frames = usize::from(decoder.copy_latest_bgra_frame_into(&mut buf).is_some());
    println!("frames while paused: {paused_frames}");
    if paused_frames != 0 {
        anyhow::bail!("frames kept arriving while paused");
    }

    // Rate 2x for 4 s: crosses the end of a 3 s generated clip, so this also checks looping.
    decoder.set_rate(2.0);
    decoder.play();
    let window = Instant::now();
    let mut rate_frames = 0;
    while window.elapsed() < Duration::from_secs(4) {
        if decoder.copy_latest_bgra_frame_into(&mut buf).is_some() {
            rate_frames += 1;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    println!(
        "frames in 4 s at 2x: {rate_frames} (~{:.0} fps)",
        rate_frames as f32 / 4.0
    );
    if rate_frames < 60 {
        anyhow::bail!("playback stalled at 2x (looping or rate broken)");
    }
    drop(decoder);
    if generated {
        let _ = std::fs::remove_file(&path);
    }
    println!("PASS");
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("win_video_probe only runs on Windows");
}
