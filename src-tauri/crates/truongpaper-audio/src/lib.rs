//! `truongpaper-audio`: System audio loopback capture (Windows WASAPI Loopback + macOS CoreAudio Tap),
//! 2048-point Hann-windowed `rustfft` analyzer (16/32/64 stereo bands at 60 Hz), and `kira` sound layer player.
//! Complete specification: `docs/01-SYSTEM-ARCHITECTURE.md` (Section 4) &
//! `docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` (Section 8.1).
//!
//! Architecture:
//! - A [`LoopbackBackend`] (OS capture, `cpal` on Windows/macOS) pushes PCM into a shared
//!   [`StereoRingBuffer`] which records `last_write`.
//! - A dedicated 60 Hz ticker thread drives [`AudioTicker::tick`], which snapshots the ring (or
//!   feeds `[0.0; 2048]` once `last_write` is older than 35 ms, Pitfall #11) and runs the
//!   [`SpectrumAnalyzer`]. WASAPI stops invoking its callback during silence, so the analyzer
//!   must never be stepped from the capture callback.
//! - The finished [`AudioSpectrumFrame`] is published through an [`ArcSwap`].

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use rustfft::{num_complex::Complex, FftPlanner};
use std::collections::VecDeque;
use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub mod sound_layer;
pub use sound_layer::{
    effective_volume, plan_scene_sounds, select_sound_path, SceneSoundPlayer, SoundLayerPlan,
    SoundPlaybackMode,
};

/// FFT window length in samples.
pub const FFT_SIZE: usize = 2048;
/// Silence watchdog threshold: no PCM written for longer than this means "silence".
pub const SILENCE_TIMEOUT: Duration = Duration::from_millis(35);
/// Analyzer tick period (60 Hz).
pub const TICK_PERIOD: Duration = Duration::from_nanos(16_666_667);
/// Smoothed band values below this are snapped to exactly `0.0` so silence converges to zeros.
const DENORMAL_FLOOR: f32 = 1.0e-4;

/// One published spectrum frame (smoothed, normalized to `[0, 1]`).
#[derive(Clone, Debug)]
pub struct AudioSpectrumFrame {
    pub left_16: [f32; 16],
    pub right_16: [f32; 16],
    pub left_32: [f32; 32],
    pub right_32: [f32; 32],
    pub left_64: [f32; 64],
    pub right_64: [f32; 64],
    /// Concatenated `[left_64[0..64], right_64[0..64]]` for Web `wallpaperRegisterAudioListener`.
    pub web_stereo_128: [f32; 128],
}

impl Default for AudioSpectrumFrame {
    fn default() -> Self {
        Self {
            left_16: [0.0; 16],
            right_16: [0.0; 16],
            left_32: [0.0; 32],
            right_32: [0.0; 32],
            left_64: [0.0; 64],
            right_64: [0.0; 64],
            web_stereo_128: [0.0; 128],
        }
    }
}

impl AudioSpectrumFrame {
    /// `true` when every band of every resolution is exactly `0.0`.
    pub fn is_silent(&self) -> bool {
        self.web_stereo_128.iter().all(|&v| v == 0.0)
            && self.left_16.iter().all(|&v| v == 0.0)
            && self.right_16.iter().all(|&v| v == 0.0)
            && self.left_32.iter().all(|&v| v == 0.0)
            && self.right_32.iter().all(|&v| v == 0.0)
    }
}

/// Last-`FFT_SIZE`-samples stereo ring buffer shared between the capture callback and the ticker.
pub struct StereoRingBuffer {
    pub left: VecDeque<f32>,
    pub right: VecDeque<f32>,
    pub last_write: Instant,
}

impl Default for StereoRingBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl StereoRingBuffer {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            left: VecDeque::from(vec![0.0; FFT_SIZE]),
            right: VecDeque::from(vec![0.0; FFT_SIZE]),
            // Start "long ago" so the watchdog reports silence until real PCM arrives.
            last_write: now.checked_sub(Duration::from_secs(10)).unwrap_or(now),
        }
    }

    /// Pushes interleaved PCM (`channels` samples per frame). Mono is duplicated to both sides.
    pub fn push_interleaved(&mut self, data: &[f32], channels: usize) {
        if channels == 0 {
            return;
        }
        for frame in data.chunks_exact(channels) {
            let l = frame[0];
            let r = if channels >= 2 { frame[1] } else { l };
            if self.left.len() >= FFT_SIZE {
                self.left.pop_front();
                self.right.pop_front();
            }
            self.left.push_back(l);
            self.right.push_back(r);
        }
        self.last_write = Instant::now();
    }

    /// Snapshots 2048 stereo samples, or returns silence if the OS stopped sending packets
    /// more than 35 ms ago.
    pub fn snapshot_or_silence(&self) -> ([f32; FFT_SIZE], [f32; FFT_SIZE]) {
        self.snapshot_or_silence_at(Instant::now())
    }

    /// Same as [`Self::snapshot_or_silence`] with an explicit clock (deterministic in tests).
    pub fn snapshot_or_silence_at(&self, now: Instant) -> ([f32; FFT_SIZE], [f32; FFT_SIZE]) {
        let mut l = [0.0_f32; FFT_SIZE];
        let mut r = [0.0_f32; FFT_SIZE];
        if now.saturating_duration_since(self.last_write) > SILENCE_TIMEOUT {
            return (l, r);
        }
        for (dst, &src) in l.iter_mut().zip(self.left.iter()) {
            *dst = src;
        }
        for (dst, &src) in r.iter_mut().zip(self.right.iter()) {
            *dst = src;
        }
        (l, r)
    }
}

/// 2048-point Hann-windowed stereo FFT with log-spaced 16/32/64 band reduction and smoothing.
pub struct SpectrumAnalyzer {
    fft: Arc<dyn rustfft::Fft<f32>>,
    hann_window: Vec<f32>,
    scratch_left: Vec<Complex<f32>>,
    scratch_right: Vec<Complex<f32>>,
    prev_frame: AudioSpectrumFrame,
    sample_rate: f32,
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: f32) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let hann_window = (0..FFT_SIZE)
            .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f32 / (FFT_SIZE as f32 - 1.0)).cos()))
            .collect();

        Self {
            fft,
            hann_window,
            scratch_left: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            scratch_right: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            prev_frame: AudioSpectrumFrame::default(),
            sample_rate,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn process_stereo_pcm(
        &mut self,
        left_pcm: &[f32; FFT_SIZE],
        right_pcm: &[f32; FFT_SIZE],
        sensitivity_gain: f32,
    ) -> &AudioSpectrumFrame {
        for i in 0..FFT_SIZE {
            self.scratch_left[i] = Complex::new(left_pcm[i] * self.hann_window[i], 0.0);
            self.scratch_right[i] = Complex::new(right_pcm[i] * self.hann_window[i], 0.0);
        }

        self.fft.process(&mut self.scratch_left);
        self.fft.process(&mut self.scratch_right);

        let nyquist_bins = FFT_SIZE / 2;
        let norm = 2.0 / (FFT_SIZE as f32).sqrt();

        let mag_left: Vec<f32> = self.scratch_left[..nyquist_bins]
            .iter()
            .map(|c| (c.norm() * norm * sensitivity_gain).clamp(0.0, 1.0))
            .collect();
        let mag_right: Vec<f32> = self.scratch_right[..nyquist_bins]
            .iter()
            .map(|c| (c.norm() * norm * sensitivity_gain).clamp(0.0, 1.0))
            .collect();

        Self::bin_logarithmic(&mag_left, &mut self.prev_frame.left_16, self.sample_rate);
        Self::bin_logarithmic(&mag_right, &mut self.prev_frame.right_16, self.sample_rate);
        Self::bin_logarithmic(&mag_left, &mut self.prev_frame.left_32, self.sample_rate);
        Self::bin_logarithmic(&mag_right, &mut self.prev_frame.right_32, self.sample_rate);
        Self::bin_logarithmic(&mag_left, &mut self.prev_frame.left_64, self.sample_rate);
        Self::bin_logarithmic(&mag_right, &mut self.prev_frame.right_64, self.sample_rate);

        self.prev_frame.web_stereo_128[..64].copy_from_slice(&self.prev_frame.left_64);
        self.prev_frame.web_stereo_128[64..].copy_from_slice(&self.prev_frame.right_64);

        &self.prev_frame
    }

    fn bin_logarithmic<const N: usize>(
        magnitudes: &[f32],
        out_smoothed: &mut [f32; N],
        sample_rate: f32,
    ) {
        let min_freq = 20.0_f32;
        let max_freq = (sample_rate * 0.5).min(16000.0);
        let bin_hz = sample_rate / FFT_SIZE as f32;

        for (i, slot) in out_smoothed.iter_mut().enumerate() {
            let t0 = i as f32 / N as f32;
            let t1 = (i + 1) as f32 / N as f32;
            let f0 = min_freq * (max_freq / min_freq).powf(t0);
            let f1 = min_freq * (max_freq / min_freq).powf(t1);

            let idx0 = ((f0 / bin_hz).floor() as usize).clamp(1, magnitudes.len() - 1);
            let idx1 = ((f1 / bin_hz).ceil() as usize).clamp(idx0 + 1, magnitudes.len());

            let peak = magnitudes[idx0..idx1]
                .iter()
                .copied()
                .fold(0.0_f32, f32::max);

            // Fast attack (0.65), smooth decay (0.22).
            let prev = *slot;
            let alpha = if peak > prev { 0.65 } else { 0.22 };
            let next = (prev + alpha * (peak - prev)).clamp(0.0, 1.0);
            *slot = if next < DENORMAL_FLOOR { 0.0 } else { next };
        }
    }
}

/// Drives one analyzer step per call from the shared ring (the body of the 60 Hz ticker thread).
pub struct AudioTicker {
    ring: Arc<Mutex<StereoRingBuffer>>,
    analyzer: SpectrumAnalyzer,
}

impl AudioTicker {
    pub fn new(ring: Arc<Mutex<StereoRingBuffer>>, sample_rate: f32) -> Self {
        Self {
            ring,
            analyzer: SpectrumAnalyzer::new(sample_rate),
        }
    }

    /// One tick using the real clock. Feeds silence when the ring is stale (watchdog).
    pub fn tick(&mut self, sensitivity_gain: f32) -> AudioSpectrumFrame {
        self.tick_at(Instant::now(), sensitivity_gain)
    }

    /// One tick with an explicit clock.
    pub fn tick_at(&mut self, now: Instant, sensitivity_gain: f32) -> AudioSpectrumFrame {
        let (left, right) = self.ring.lock().snapshot_or_silence_at(now);
        self.analyzer
            .process_stereo_pcm(&left, &right, sensitivity_gain)
            .clone()
    }
}

/// OS loopback capture abstraction. Implementations push PCM into the ring from their own
/// callback/thread. Constructed *inside* the worker thread (cpal streams are not `Send`),
/// so the backend itself needs no `Send` bound.
pub trait LoopbackBackend {
    /// Starts capture and returns the capture sample rate in Hz.
    fn start(&mut self, ring: Arc<Mutex<StereoRingBuffer>>) -> anyhow::Result<f32>;
}

/// Handle to the running 60 Hz analyzer thread.
#[derive(Clone)]
pub struct AudioWorkerHandle {
    /// Ring shared with the capture backend. External PCM injectors (e.g. macOS ScreenCaptureKit
    /// `SCStreamOutputTypeAudio`) can call `push_interleaved` on it.
    pub ring: Arc<Mutex<StereoRingBuffer>>,
    stop: Arc<AtomicBool>,
}

impl AudioWorkerHandle {
    /// Asks the ticker thread to exit (the capture backend is dropped with it).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Spawns the capture backend plus the 60 Hz analyzer thread.
///
/// `make_backend` runs on the worker thread; returning `None` or a failing `start` leaves the
/// ring empty, so the watchdog keeps the spectrum at silence (external injectors still work).
pub fn spawn_audio_worker<F>(
    make_backend: F,
    latest_frame: Arc<ArcSwap<AudioSpectrumFrame>>,
    sensitivity_gain: Arc<AtomicU32>, // f32::to_bits
    all_monitors_paused: Arc<AtomicBool>,
) -> AudioWorkerHandle
where
    F: FnOnce() -> Option<Box<dyn LoopbackBackend>> + Send + 'static,
{
    let ring = Arc::new(Mutex::new(StereoRingBuffer::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let ring_for_thread = Arc::clone(&ring);
    let stop_for_thread = Arc::clone(&stop);

    std::thread::spawn(move || {
        let mut backend = make_backend();
        let mut sample_rate = 48_000.0_f32;
        if let Some(b) = backend.as_mut() {
            match b.start(Arc::clone(&ring_for_thread)) {
                Ok(sr) if sr > 0.0 => sample_rate = sr,
                Ok(_) => tracing::warn!("Audio loopback reported invalid sample rate"),
                Err(e) => tracing::warn!("Audio loopback start failed: {e}"),
            }
        }

        let mut ticker = AudioTicker::new(ring_for_thread, sample_rate);
        let mut next_deadline = Instant::now();
        while !stop_for_thread.load(Ordering::Relaxed) {
            if all_monitors_paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(250));
                next_deadline = Instant::now();
                continue;
            }
            // 60 Hz tick even during WASAPI silence (no callbacks).
            next_deadline += TICK_PERIOD;
            let now = Instant::now();
            if let Some(wait) = next_deadline.checked_duration_since(now) {
                std::thread::sleep(wait);
            } else {
                next_deadline = now; // fell behind; don't burst
            }
            let gain = f32::from_bits(sensitivity_gain.load(Ordering::Relaxed));
            latest_frame.store(Arc::new(ticker.tick(gain)));
        }
    });

    AudioWorkerHandle { ring, stop }
}

/// Starts cross-platform loopback capture:
/// - Windows: WASAPI Loopback (`host.default_output_device()` opened as an input stream).
/// - macOS: Virtual loopback audio device (`BlackHole` / `Loopback` / `Soundflower`) without ever
///   activating the hardware microphone, plus an external PCM injector handle
///   (`Arc<Mutex<StereoRingBuffer>>`) so `ScreenCaptureKit` can push native system audio directly.
/// - Other platforms: no capture; the ring can still be fed externally.
pub fn spawn_audio_loopback_worker(
    latest_frame: Arc<ArcSwap<AudioSpectrumFrame>>,
    sensitivity_gain: Arc<AtomicU32>, // f32::to_bits
    all_monitors_paused: Arc<AtomicBool>,
) -> Arc<Mutex<StereoRingBuffer>> {
    spawn_audio_worker(
        default_backend,
        latest_frame,
        sensitivity_gain,
        all_monitors_paused,
    )
    .ring
}

/// Starts loopback capture with an explicit stop handle for runtime ownership.
/// The legacy ring-only helper remains available for external PCM injectors.
pub fn start_audio_loopback_worker(
    latest_frame: Arc<ArcSwap<AudioSpectrumFrame>>,
    sensitivity_gain: Arc<AtomicU32>,
    all_monitors_paused: Arc<AtomicBool>,
) -> AudioWorkerHandle {
    spawn_audio_worker(
        default_backend,
        latest_frame,
        sensitivity_gain,
        all_monitors_paused,
    )
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn default_backend() -> Option<Box<dyn LoopbackBackend>> {
    Some(Box::new(cpal_backend::CpalLoopbackBackend::default()))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn default_backend() -> Option<Box<dyn LoopbackBackend>> {
    None
}

/// `cpal` OS loopback implementation (Windows WASAPI loopback, macOS virtual device).
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod cpal_backend {
    use super::{LoopbackBackend, StereoRingBuffer};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::SampleFormat;
    use parking_lot::Mutex;
    use std::sync::Arc;

    /// Holds the live `cpal::Stream` (kept alive for as long as the backend exists).
    #[derive(Default)]
    pub struct CpalLoopbackBackend {
        stream: Option<cpal::Stream>,
    }

    impl LoopbackBackend for CpalLoopbackBackend {
        fn start(&mut self, ring: Arc<Mutex<StereoRingBuffer>>) -> anyhow::Result<f32> {
            let host = cpal::default_host();
            let (device, config) = find_device(&host)
                .ok_or_else(|| anyhow::anyhow!("no loopback audio device available"))?;
            let sample_rate = config.sample_rate().0 as f32;
            let stream = build_capture_stream(&device, &config, ring)
                .ok_or_else(|| anyhow::anyhow!("unsupported loopback sample format"))?;
            stream.play()?;
            self.stream = Some(stream);
            Ok(sample_rate)
        }
    }

    #[cfg(target_os = "windows")]
    fn find_device(host: &cpal::Host) -> Option<(cpal::Device, cpal::SupportedStreamConfig)> {
        let dev = host.default_output_device()?;
        let cfg = dev
            .default_output_config()
            .or_else(|_| dev.default_input_config())
            .ok()?;
        Some((dev, cfg))
    }

    /// Searches input devices ONLY for known loopback/virtual drivers so the built-in
    /// microphone is never opened.
    #[cfg(target_os = "macos")]
    fn find_device(host: &cpal::Host) -> Option<(cpal::Device, cpal::SupportedStreamConfig)> {
        let devices = host.input_devices().ok()?;
        for dev in devices {
            let name = dev.name().unwrap_or_default().to_ascii_lowercase();
            if name.contains("blackhole")
                || name.contains("loopback")
                || name.contains("soundflower")
                || name.contains("tap")
            {
                if let Ok(cfg) = dev.default_input_config() {
                    return Some((dev, cfg));
                }
            }
        }
        None
    }

    /// Builds a stream supporting F32, I16 and U16 sample formats, pushing normalized f32 frames.
    fn build_capture_stream(
        device: &cpal::Device,
        config: &cpal::SupportedStreamConfig,
        ring: Arc<Mutex<StereoRingBuffer>>,
    ) -> Option<cpal::Stream> {
        let channels = config.channels() as usize;
        let stream_config: cpal::StreamConfig = config.clone().into();
        let err_fn = |err| tracing::warn!("Audio loopback stream error: {err}");

        match config.sample_format() {
            SampleFormat::F32 => device
                .build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        ring.lock().push_interleaved(data, channels);
                    },
                    err_fn,
                    None,
                )
                .ok(),
            SampleFormat::I16 => {
                let mut scratch: Vec<f32> = Vec::new();
                device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            scratch.clear();
                            scratch.extend(data.iter().map(|&s| s as f32 / 32768.0));
                            ring.lock().push_interleaved(&scratch, channels);
                        },
                        err_fn,
                        None,
                    )
                    .ok()
            }
            SampleFormat::U16 => {
                let mut scratch: Vec<f32> = Vec::new();
                device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[u16], _: &cpal::InputCallbackInfo| {
                            scratch.clear();
                            scratch.extend(data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0));
                            ring.lock().push_interleaved(&scratch, channels);
                        },
                        err_fn,
                        None,
                    )
                    .ok()
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f32 / SR).sin())
            .collect()
    }

    fn argmax(bands: &[f32]) -> usize {
        bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap()
    }

    /// Band index a frequency falls into for `n` log-spaced bands over 20 Hz..16 kHz.
    fn expected_band(freq: f32, n: usize) -> usize {
        let t = (freq / 20.0).ln() / (16000.0_f32 / 20.0).ln();
        ((t * n as f32).floor() as usize).min(n - 1)
    }

    fn stereo_snapshot(l: &[f32], r: &[f32]) -> ([f32; FFT_SIZE], [f32; FFT_SIZE]) {
        let mut ring = StereoRingBuffer::new();
        let inter: Vec<f32> = l.iter().zip(r).flat_map(|(a, b)| [*a, *b]).collect();
        ring.push_interleaved(&inter, 2);
        ring.snapshot_or_silence()
    }

    #[test]
    fn default_spectrum_frame_is_zeroed() {
        let frame = AudioSpectrumFrame::default();
        assert_eq!(frame.web_stereo_128.len(), 128);
        assert!(frame.web_stereo_128.iter().all(|&v| v == 0.0));
        assert!(frame.is_silent());
    }

    #[test]
    fn ring_keeps_latest_fft_size_samples_and_mono_is_duplicated() {
        let mut ring = StereoRingBuffer::new();
        let data: Vec<f32> = (0..FFT_SIZE + 100).map(|i| i as f32).collect();
        ring.push_interleaved(&data, 1);
        assert_eq!(ring.left.len(), FFT_SIZE);
        assert_eq!(ring.left.back().copied(), Some((FFT_SIZE + 99) as f32));
        assert_eq!(ring.left, ring.right);
        ring.push_interleaved(&[1.0], 0); // channels == 0 is a no-op
        assert_eq!(ring.left.len(), FFT_SIZE);
    }

    #[test]
    fn dominant_band_matches_sine_frequency() {
        for &freq in &[200.0_f32, 1000.0, 5000.0] {
            let pcm = sine(freq, 0.01, FFT_SIZE);
            let (l, r) = stereo_snapshot(&pcm, &pcm);
            let mut a = SpectrumAnalyzer::new(SR);
            let f = a.process_stereo_pcm(&l, &r, 1.0).clone();

            let got64 = argmax(&f.left_64);
            let exp64 = expected_band(freq, 64);
            assert!(
                got64.abs_diff(exp64) <= 1,
                "{freq} Hz: 64-band argmax {got64}, expected {exp64}"
            );
            let got32 = argmax(&f.left_32);
            assert!(got32.abs_diff(expected_band(freq, 32)) <= 1, "{freq} Hz 32");
            let got16 = argmax(&f.left_16);
            assert!(got16.abs_diff(expected_band(freq, 16)) <= 1, "{freq} Hz 16");
            assert!(f.left_64[got64] > 0.05);
            assert_eq!(f.left_64, f.right_64);
            assert_eq!(&f.web_stereo_128[..64], &f.left_64[..]);
            assert_eq!(&f.web_stereo_128[64..], &f.right_64[..]);
        }
    }

    #[test]
    fn stereo_channels_are_independent() {
        let tone = sine(1000.0, 0.01, FFT_SIZE);
        let zeros = vec![0.0; FFT_SIZE];
        let (l, r) = stereo_snapshot(&tone, &zeros);
        let mut a = SpectrumAnalyzer::new(SR);
        let f = a.process_stereo_pcm(&l, &r, 1.0).clone();
        assert!(f.left_64.iter().any(|&v| v > 0.05));
        assert!(f.right_64.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn silence_watchdog_returns_zero_pcm_after_35ms() {
        let mut ring = StereoRingBuffer::new();
        ring.push_interleaved(&sine(1000.0, 0.5, FFT_SIZE * 2), 1);
        let t0 = ring.last_write;

        let (l, _) = ring.snapshot_or_silence_at(t0 + Duration::from_millis(20));
        assert!(l.iter().any(|&v| v != 0.0), "fresh data must pass through");

        let (l, r) = ring.snapshot_or_silence_at(t0 + Duration::from_millis(36));
        assert!(l.iter().all(|&v| v == 0.0) && r.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn fresh_ring_reports_silence() {
        let ring = StereoRingBuffer::new();
        let (l, r) = ring.snapshot_or_silence();
        assert!(l.iter().all(|&v| v == 0.0) && r.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn stale_ring_zeros_the_spectrum() {
        let ring = Arc::new(Mutex::new(StereoRingBuffer::new()));
        ring.lock()
            .push_interleaved(&sine(1000.0, 0.01, FFT_SIZE), 1);
        let t0 = ring.lock().last_write;
        let mut ticker = AudioTicker::new(Arc::clone(&ring), SR);

        // Live audio: spectrum is non-zero.
        let live = ticker.tick_at(t0 + Duration::from_millis(5), 1.0);
        assert!(!live.is_silent());

        // Capture callback stopped (WASAPI silence): watchdog feeds zeros, bands decay to 0.
        let mut now = t0 + Duration::from_millis(40);
        let mut prev_peak = live.left_64.iter().copied().fold(0.0, f32::max);
        let mut frame = live;
        for _ in 0..120 {
            now += Duration::from_nanos(16_666_667);
            frame = ticker.tick_at(now, 1.0);
            let peak = frame.left_64.iter().copied().fold(0.0, f32::max);
            assert!(peak <= prev_peak, "decay must be monotonic");
            prev_peak = peak;
        }
        assert!(frame.is_silent(), "spectrum must converge to exactly zero");
        assert!(frame.left_64.iter().all(|&v| v == 0.0));
        assert!(frame.right_64.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn sensitivity_gain_scales_output() {
        let pcm = sine(1000.0, 0.001, FFT_SIZE);
        let (l, r) = stereo_snapshot(&pcm, &pcm);
        let lo = SpectrumAnalyzer::new(SR)
            .process_stereo_pcm(&l, &r, 1.0)
            .left_64
            .iter()
            .copied()
            .fold(0.0, f32::max);
        let hi = SpectrumAnalyzer::new(SR)
            .process_stereo_pcm(&l, &r, 4.0)
            .left_64
            .iter()
            .copied()
            .fold(0.0, f32::max);
        assert!(hi > lo * 2.0);
    }

    #[test]
    fn stopping_worker_releases_capture_backend() {
        struct CaptureProbe(std::sync::mpsc::Sender<()>);
        impl LoopbackBackend for CaptureProbe {
            fn start(&mut self, _: Arc<Mutex<StereoRingBuffer>>) -> anyhow::Result<f32> {
                Ok(SR)
            }
        }
        impl Drop for CaptureProbe {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let (released, receiver) = std::sync::mpsc::channel();
        let worker = spawn_audio_worker(
            move || Some(Box::new(CaptureProbe(released)) as Box<dyn LoopbackBackend>),
            Arc::new(ArcSwap::from_pointee(AudioSpectrumFrame::default())),
            Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            Arc::new(AtomicBool::new(false)),
        );
        worker.stop();
        receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("capture must be released on stop");
    }

    /// Backend that feeds synthetic PCM for ~300 ms from its own thread, then stops calling
    /// back, like WASAPI loopback when playback ends.
    struct BurstBackend;

    impl LoopbackBackend for BurstBackend {
        fn start(&mut self, ring: Arc<Mutex<StereoRingBuffer>>) -> anyhow::Result<f32> {
            std::thread::spawn(move || {
                let chunk = sine(1000.0, 0.01, 480);
                let end = Instant::now() + Duration::from_millis(300);
                while Instant::now() < end {
                    ring.lock().push_interleaved(&chunk, 1);
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
            Ok(SR)
        }
    }

    #[test]
    fn worker_publishes_then_watchdog_zeros_frame() {
        let latest = Arc::new(ArcSwap::from_pointee(AudioSpectrumFrame::default()));
        let handle = spawn_audio_worker(
            || Some(Box::new(BurstBackend) as Box<dyn LoopbackBackend>),
            Arc::clone(&latest),
            Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            Arc::new(AtomicBool::new(false)),
        );

        let wait_for = |pred: &dyn Fn(&AudioSpectrumFrame) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if pred(&latest.load()) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            false
        };

        assert!(wait_for(&|f| !f.is_silent()), "live audio must publish");
        assert!(
            wait_for(&|f| f.is_silent()),
            "frame must return to zeros without further callbacks"
        );
        handle.stop();
    }
}
