//! System-audio loopback bridge (docs/01 §6.1 item 4).
//!
//! Capture is demand-owned: removing or pausing the last audio-reactive wallpaper stops the
//! worker and releases its capture backend. Demand starts a fresh worker on resume.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use truongpaper_audio::AudioSpectrumFrame;

type FrameReader = Box<dyn Fn() -> Arc<AudioSpectrumFrame> + Send + Sync>;

struct AudioBridge {
    read: FrameReader,
    all_paused: Arc<AtomicBool>,
    gain: Arc<AtomicU32>,
    /// Dropping the bridge drops the last handle, which stops and joins the worker.
    _worker: truongpaper_audio::AudioWorkerHandle,
}

static BRIDGE: Mutex<Option<AudioBridge>> = parking_lot::const_mutex(None);

fn start() -> AudioBridge {
    let all_paused = Arc::new(AtomicBool::new(false));
    let gain = Arc::new(AtomicU32::new(1.0f32.to_bits()));
    // `Arc<ArcSwap<AudioSpectrumFrame>>`; the type comes from the worker signature (the
    // `arc-swap` crate is not a direct dependency of this package).
    let latest = Arc::default();
    let worker = truongpaper_audio::start_audio_loopback_worker(
        Arc::clone(&latest),
        Arc::clone(&gain),
        Arc::clone(&all_paused),
    );
    tracing::info!("system audio loopback worker started");
    AudioBridge {
        read: Box::new(move || latest.load_full()),
        all_paused,
        gain,
        _worker: worker,
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
/// `None` suspends it: the worker stops, joins and releases the capture device.
///
/// The capture device is started and stopped outside the `BRIDGE` mutex, so
/// [`latest_frame`] never waits on a slow device open or on a worker join.
pub fn set_demand(demand: Option<f32>) {
    match demand {
        Some(sensitivity) => {
            let gain = sensitivity_gain(sensitivity).to_bits();
            let running = {
                let current = BRIDGE.lock();
                current.as_ref().map(|bridge| {
                    bridge.gain.store(gain, Ordering::Relaxed);
                    bridge.all_paused.store(false, Ordering::Relaxed);
                })
            };
            if running.is_some() {
                return;
            }
            let bridge = start();
            bridge.gain.store(gain, Ordering::Relaxed);
            let loser = {
                let mut current = BRIDGE.lock();
                if current.is_none() {
                    *current = Some(bridge);
                    None
                } else {
                    Some(bridge)
                }
            };
            // A concurrent caller won the race: stop our duplicate outside the lock.
            drop(loser);
        }
        None => {
            let old = BRIDGE.lock().take();
            drop(old);
        }
    }
}

/// Newest spectrum frame, or `None` while the worker is not running / suspended.
pub fn latest_frame() -> Option<Arc<AudioSpectrumFrame>> {
    let current = BRIDGE.lock();
    let bridge = current.as_ref()?;
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
        assert!(BRIDGE.lock().is_none());
        assert!(latest_frame().is_none());
    }
}
