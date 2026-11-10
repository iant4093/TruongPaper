//! System-audio loopback bridge (docs/01 §6.1 item 4).
//!
//! The `truongpaper-audio` loopback worker is started lazily the first time an audio-reactive
//! scene or Web wallpaper is visible. While no such wallpaper is playing (all monitors paused or
//! none audio-reactive) its `all_monitors_paused` flag is raised, which suspends the 60 Hz FFT
//! thread. `spawn_audio_loopback_worker` returns no stop handle, so the capture stream itself
//! stays open once started.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use truongpaper_audio::AudioSpectrumFrame;

type FrameReader = Box<dyn Fn() -> Arc<AudioSpectrumFrame> + Send + Sync>;

struct AudioBridge {
    read: FrameReader,
    all_paused: Arc<AtomicBool>,
    gain: Arc<AtomicU32>,
}

static BRIDGE: OnceLock<AudioBridge> = OnceLock::new();

fn start() -> AudioBridge {
    let all_paused = Arc::new(AtomicBool::new(false));
    let gain = Arc::new(AtomicU32::new(1.0f32.to_bits()));
    // `Arc<ArcSwap<AudioSpectrumFrame>>`; the type comes from the worker signature (the
    // `arc-swap` crate is not a direct dependency of this package).
    let latest = Arc::default();
    let _ring = truongpaper_audio::spawn_audio_loopback_worker(
        Arc::clone(&latest),
        Arc::clone(&gain),
        Arc::clone(&all_paused),
    );
    tracing::info!("system audio loopback worker started");
    AudioBridge {
        read: Box::new(move || latest.load_full()),
        all_paused,
        gain,
    }
}

/// Sanitized analyzer gain for `UniversalAdjustments::audio_sensitivity`.
pub fn sensitivity_gain(sensitivity: f32) -> f32 {
    if sensitivity.is_finite() {
        sensitivity.clamp(0.0, 5.0)
    } else {
        1.0
    }
}

/// `Some(sensitivity)` = an audio-reactive wallpaper is playing (starts / resumes the worker);
/// `None` suspends it.
pub fn set_demand(demand: Option<f32>) {
    match demand {
        Some(sensitivity) => {
            let bridge = BRIDGE.get_or_init(start);
            bridge
                .gain
                .store(sensitivity_gain(sensitivity).to_bits(), Ordering::Relaxed);
            bridge.all_paused.store(false, Ordering::Relaxed);
        }
        None => {
            if let Some(bridge) = BRIDGE.get() {
                bridge.all_paused.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Newest spectrum frame, or `None` while the worker is not running / suspended.
pub fn latest_frame() -> Option<Arc<AudioSpectrumFrame>> {
    let bridge = BRIDGE.get()?;
    if bridge.all_paused.load(Ordering::Relaxed) {
        return None;
    }
    Some((bridge.read)())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitivity_is_clamped() {
        assert_eq!(sensitivity_gain(2.0), 2.0);
        assert_eq!(sensitivity_gain(9.0), 5.0);
        assert_eq!(sensitivity_gain(-1.0), 0.0);
        assert_eq!(sensitivity_gain(f32::NAN), 1.0);
    }

    #[test]
    fn no_frames_before_the_worker_starts() {
        // Only the `None` path runs here: starting the worker would open an audio device.
        set_demand(None);
        assert!(BRIDGE.get().is_none());
        assert!(latest_frame().is_none());
    }
}
