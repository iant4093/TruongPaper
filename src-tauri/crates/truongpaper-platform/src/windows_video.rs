//! Windows Media Foundation decoder for embedded `.tex` MP4 video textures
//! (`free_image_format == 35`).
//!
//! Compiled as `crate::win_video` (Windows only); the counterpart of `crate::mac_video`. The
//! renderer extracts the MP4 to `<wallpaper_dir>/.cache/<name>.mp4`; this module plays that file
//! with `IMFMediaEngine` in frame-server mode, the Media Foundation equivalent of `AVPlayer`:
//! hardware decode (D3D11VA through a DXGI device manager), audio output, gapless looping
//! (`SetLoop`), playback rate and volume all come from the engine.
//!
//! Each decoder owns a dedicated `truongpaper-video` thread (COM MTA) with its own D3D11 device.
//! The thread polls `OnVideoStreamTick`, and for every new presentation frame it calls
//! `TransferVideoFrame` into a `B8G8R8A8` render target, copies it to a staging texture, maps it
//! and stores tightly packed BGRA rows (alpha forced to 255) as the latest frame. The render
//! thread takes that frame without decoding anything itself; buffers rotate between the two
//! threads, so steady-state playback does not allocate.
//!
//! Opening is asynchronous like `AVPlayerItem`: [`WinVideoDecoder::open`] fails only when the file
//! is missing or Media Foundation / the engine cannot be created. A file the installed codecs
//! cannot decode (for example HEVC without the Store "HEVC Video Extensions") reports
//! [`WinVideoStatus::Failed`], logs a warning and never produces frames, so the scene keeps its
//! placeholder texture.
//!
//! `truongpaper-platform` does not depend on `truongpaper-renderer`, so frames are returned as
//! plain `(width, height, stride, bytes)` tuples; the app glue adapts them to the renderer's
//! `VideoFrameSource`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use windows::core::{Interface, BSTR, HRESULT, PCWSTR};
use windows::Win32::Foundation::{HMODULE, RECT, S_OK};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
    D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_FLAG, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFDXGIDeviceManager, IMFMediaEngine,
    IMFMediaEngineClassFactory, IMFMediaEngineNotify, IMFMediaEngineNotify_Impl,
    MFCreateAttributes, MFCreateDXGIDeviceManager, MFShutdown, MFStartup, MFARGB, MFSTARTUP_FULL,
    MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_DXGI_MANAGER, MF_MEDIA_ENGINE_EVENT_ERROR,
    MF_MEDIA_ENGINE_EVENT_FORMATCHANGE, MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA,
    MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

/// Same range as the renderer's `MIN_PLAYBACK_SPEED..=MAX_PLAYBACK_SPEED`.
pub const MIN_VIDEO_RATE: f32 = 0.1;
pub const MAX_VIDEO_RATE: f32 = 2.0;

/// Poll interval for `OnVideoStreamTick` while playing (well under one frame at 120 fps).
const PLAYING_POLL: Duration = Duration::from_millis(4);
/// Poll interval while paused after a first frame, or after a failure (only commands matter).
const IDLE_POLL: Duration = Duration::from_millis(100);
/// Upper bound for engine creation in [`WinVideoDecoder::open`] (D3D11 device + engine).
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

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

/// `(width, height, stride, bytes)` of a BGRA frame. Rows are tightly packed (`stride == width * 4`).
pub type BgraFrame = (u32, u32, usize, Vec<u8>);

/// Load state of the media engine (the `AVPlayerItem.status` equivalent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WinVideoStatus {
    /// Source set, metadata not loaded yet.
    Loading,
    /// Metadata loaded and a video stream is present.
    Ready {
        width: u32,
        height: u32,
        has_audio: bool,
    },
    /// The file cannot be played (missing codec, corrupt file, no video stream, ...).
    Failed(String),
}

/// Latest decoded frame shared between the decode thread and the render thread.
struct FrameSlot {
    width: u32,
    height: u32,
    data: Vec<u8>,
    /// `true` until the render thread takes the frame.
    fresh: bool,
}

struct Shared {
    slot: Mutex<FrameSlot>,
    status: Mutex<WinVideoStatus>,
}

enum Command {
    Play,
    Pause,
    Rate(f32),
    Volume(f32),
    Muted(bool),
    Shutdown,
}

/// Looping `IMFMediaEngine` over a local MP4 file producing BGRA frames.
///
/// Created paused at rate `1.0`; call [`WinVideoDecoder::play`] to start. All Media Foundation
/// objects live on the decoder's own thread, so the handle itself is `Send`.
pub struct WinVideoDecoder {
    shared: Arc<Shared>,
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    rate: f32,
    paused: bool,
}

impl WinVideoDecoder {
    /// Opens `path` (an extracted `.cache/<name>.mp4`). The file must exist; decoding errors
    /// surface asynchronously as [`WinVideoStatus::Failed`] and an absence of frames.
    pub fn open(path: &Path) -> Result<Self> {
        let meta = std::fs::metadata(path)
            .with_context(|| format!("video file not found: {}", path.display()))?;
        if !meta.is_file() {
            bail!("video path is not a file: {}", path.display());
        }
        let shared = Arc::new(Shared {
            slot: Mutex::new(FrameSlot {
                width: 0,
                height: 0,
                data: Vec::new(),
                fresh: false,
            }),
            status: Mutex::new(WinVideoStatus::Loading),
        });
        let (commands, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread_shared = Arc::clone(&shared);
        let thread_path = path.to_path_buf();
        let thread = std::thread::Builder::new()
            .name("truongpaper-video".into())
            .spawn(move || video_thread(thread_path, thread_shared, command_rx, ready_tx))
            .context("spawn video decode thread")?;
        match ready_rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = thread.join();
                return Err(err);
            }
            Err(_) => {
                // The thread is stuck inside Media Foundation; detach it rather than block.
                let _ = commands.send(Command::Shutdown);
                bail!("Media Foundation engine creation timed out");
            }
        }
        tracing::debug!(path = %path.display(), "opened Media Foundation video decoder");
        Ok(Self {
            shared,
            commands,
            thread: Some(thread),
            rate: 1.0,
            paused: true,
        })
    }

    /// Copies the newest decoded frame into a fresh `Vec` (see
    /// [`Self::copy_latest_bgra_frame_into`] for the allocation-free variant).
    pub fn copy_latest_bgra_frame(&mut self) -> Option<BgraFrame> {
        let mut bytes = Vec::new();
        let (width, height, stride) = self.copy_latest_bgra_frame_into(&mut bytes)?;
        Some((width, height, stride, bytes))
    }

    /// Moves the newest frame decoded since the last call into `buf` and returns
    /// `(width, height, stride)`. `buf`'s previous allocation is handed to the decode thread for
    /// a later frame, so a recycled buffer costs no allocation. Returns `None` when no new frame
    /// was decoded (always the case while paused after the first frame).
    pub fn copy_latest_bgra_frame_into(&mut self, buf: &mut Vec<u8>) -> Option<(u32, u32, usize)> {
        let mut slot = self.shared.slot.lock();
        if !slot.fresh {
            return None;
        }
        slot.fresh = false;
        std::mem::swap(buf, &mut slot.data);
        Some((slot.width, slot.height, slot.width as usize * 4))
    }

    /// Sets the playback rate (clamped to `0.1..=2.0`). Applied immediately.
    pub fn set_rate(&mut self, rate: f32) {
        self.rate = clamp_video_rate(rate);
        let _ = self.commands.send(Command::Rate(self.rate));
    }

    pub fn set_volume(&mut self, volume: f32) {
        let _ = self
            .commands
            .send(Command::Volume(clamp_video_volume(volume)));
    }

    pub fn set_muted(&mut self, muted: bool) {
        let _ = self.commands.send(Command::Muted(muted));
    }

    pub fn play(&mut self) {
        self.paused = false;
        let _ = self.commands.send(Command::Play);
    }

    pub fn pause(&mut self) {
        self.paused = true;
        let _ = self.commands.send(Command::Pause);
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }

    /// Current load state; lets callers detect files Media Foundation cannot decode.
    pub fn status(&self) -> WinVideoStatus {
        self.shared.status.lock().clone()
    }
}

impl Drop for WinVideoDecoder {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Copies `height` rows of `width` BGRA pixels from `src` (row pitch `pitch`) into `dst` as
/// tightly packed rows and forces every alpha byte to 255 (video is opaque; some drivers leave
/// the X channel of `B8G8R8X8`-style output undefined). Returns `false` when `src` is too short.
fn copy_rows_opaque(
    dst: &mut Vec<u8>,
    src: &[u8],
    width: usize,
    height: usize,
    pitch: usize,
) -> bool {
    let row = width * 4;
    if width == 0 || height == 0 || pitch < row {
        return false;
    }
    let needed = pitch * (height - 1) + row;
    if src.len() < needed {
        return false;
    }
    dst.clear();
    dst.reserve(row * height);
    for y in 0..height {
        let start = y * pitch;
        dst.extend_from_slice(&src[start..start + row]);
    }
    for alpha in dst.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    true
}

// ---------------------------------------------------------------------------------------------
// Decode thread
// ---------------------------------------------------------------------------------------------

/// `(event, param1, param2)` from `IMFMediaEngineNotify::EventNotify`.
type EngineEvent = (u32, usize, u32);

#[windows_core::implement(IMFMediaEngineNotify)]
struct EngineNotify {
    events: Mutex<Sender<EngineEvent>>,
}

impl IMFMediaEngineNotify_Impl for EngineNotify_Impl {
    fn EventNotify(&self, event: u32, param1: usize, param2: u32) -> windows::core::Result<()> {
        let _ = self.events.lock().send((event, param1, param2));
        Ok(())
    }
}

/// Balances `CoInitializeEx` on drop.
struct ComGuard;

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: paired with a successful `CoInitializeEx` on this thread.
        unsafe { CoUninitialize() };
    }
}

/// Balances `MFStartup` on drop.
struct MfGuard;

impl Drop for MfGuard {
    fn drop(&mut self) {
        // SAFETY: paired with a successful `MFStartup` on this thread.
        let _ = unsafe { MFShutdown() };
    }
}

/// Starts COM (MTA) and Media Foundation on the current thread.
fn start_media_foundation() -> Result<(ComGuard, MfGuard)> {
    // SAFETY: plain COM/MF initialization calls; the guards undo them in reverse order.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .context("CoInitializeEx")?;
        let com = ComGuard;
        MFStartup(MF_VERSION, MFSTARTUP_FULL).context("MFStartup")?;
        Ok((com, MfGuard))
    }
}

fn video_thread(
    path: PathBuf,
    shared: Arc<Shared>,
    commands: Receiver<Command>,
    ready: SyncSender<Result<()>>,
) {
    // Declared first so they drop last, after the engine shut down.
    let _guards = match start_media_foundation() {
        Ok(guards) => guards,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    let (events_tx, events_rx) = mpsc::channel();
    // SAFETY: COM and Media Foundation are initialized on this thread for the engine's lifetime.
    let engine = match unsafe { Engine::create(&path, events_tx) } {
        Ok(engine) => engine,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    engine.run(&path, &shared, &commands, &events_rx);
}

struct RenderTargets {
    width: u32,
    height: u32,
    target: ID3D11Texture2D,
    staging: ID3D11Texture2D,
}

struct Engine {
    engine: IMFMediaEngine,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    _manager: IMFDXGIDeviceManager,
    targets: Option<RenderTargets>,
}

/// Creates a D3D11 device suitable for the media engine (video decode + BGRA render targets).
///
/// # Safety
/// COM must be initialized on the calling thread.
unsafe fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let attempts: [(D3D_DRIVER_TYPE, D3D11_CREATE_DEVICE_FLAG); 3] = [
        (
            D3D_DRIVER_TYPE_HARDWARE,
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        ),
        (
            D3D_DRIVER_TYPE_WARP,
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        ),
        (D3D_DRIVER_TYPE_WARP, D3D11_CREATE_DEVICE_BGRA_SUPPORT),
    ];
    let mut last_err = None;
    for (driver, flags) in attempts {
        let mut device = None;
        let mut context = None;
        match D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            flags,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        ) {
            Ok(()) => {
                if let (Some(device), Some(context)) = (device, context) {
                    return Ok((device, context));
                }
            }
            Err(err) => last_err = Some(err),
        }
    }
    Err(match last_err {
        Some(err) => anyhow!(err).context("D3D11CreateDevice"),
        None => anyhow!("D3D11CreateDevice returned no device"),
    })
}

impl Engine {
    /// # Safety
    /// COM and Media Foundation must be initialized on the calling thread, and every method of
    /// the returned engine must be called from that thread.
    unsafe fn create(path: &Path, events: Sender<EngineEvent>) -> Result<Self> {
        let (device, context) = create_device()?;
        // The engine uses the device from Media Foundation worker threads.
        // Returns the previous protection state, not an error.
        let _ = device
            .cast::<ID3D11Multithread>()
            .context("ID3D11Multithread")?
            .SetMultithreadProtected(true);

        let mut reset_token = 0u32;
        let mut manager: Option<IMFDXGIDeviceManager> = None;
        MFCreateDXGIDeviceManager(&mut reset_token, &mut manager)
            .context("MFCreateDXGIDeviceManager")?;
        let manager = manager.ok_or_else(|| anyhow!("MFCreateDXGIDeviceManager returned null"))?;
        manager
            .ResetDevice(&device, reset_token)
            .context("IMFDXGIDeviceManager::ResetDevice")?;

        let notify: IMFMediaEngineNotify = EngineNotify {
            events: Mutex::new(events),
        }
        .into();
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 3).context("MFCreateAttributes")?;
        let attributes = attributes.ok_or_else(|| anyhow!("MFCreateAttributes returned null"))?;
        attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
        attributes.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &manager)?;
        attributes.SetUINT32(
            &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
            DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
        )?;

        let factory: IMFMediaEngineClassFactory =
            CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
                .context("CoCreateInstance(MFMediaEngineClassFactory)")?;
        let engine = factory
            .CreateInstance(0, &attributes)
            .context("IMFMediaEngineClassFactory::CreateInstance")?;

        let configure = || -> windows::core::Result<()> {
            engine.SetAutoPlay(false)?;
            engine.SetLoop(true)?;
            // Muted until the host makes this monitor the scene's sound owner.
            engine.SetMuted(true)?;
            engine.SetSource(&BSTR::from(path.as_os_str().to_string_lossy().as_ref()))
        };
        if let Err(err) = configure() {
            let _ = engine.Shutdown();
            return Err(anyhow!(err).context("configure media engine"));
        }
        Ok(Self {
            engine,
            device,
            context,
            _manager: manager,
            targets: None,
        })
    }

    fn run(
        mut self,
        path: &Path,
        shared: &Shared,
        commands: &Receiver<Command>,
        events: &Receiver<EngineEvent>,
    ) {
        let mut paused = true;
        let mut delivered = false;
        let mut failed = false;
        let mut back: Vec<u8> = Vec::new();
        loop {
            let timeout = if failed || (paused && delivered) {
                IDLE_POLL
            } else {
                PLAYING_POLL
            };
            let mut next = match commands.recv_timeout(timeout) {
                Ok(cmd) => Some(cmd),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            while let Some(cmd) = next {
                if !self.apply(cmd, &mut paused) {
                    return;
                }
                next = commands.try_recv().ok();
            }

            while let Ok((event, param1, param2)) = events.try_recv() {
                if let Some(status) = self.status_for_event(event, param1, param2) {
                    if let WinVideoStatus::Failed(reason) = &status {
                        if !failed {
                            tracing::warn!(
                                "embedded video {} stays on its placeholder: {reason}",
                                path.display()
                            );
                        }
                        failed = true;
                    }
                    *shared.status.lock() = status;
                }
            }
            if failed {
                continue;
            }

            // SAFETY: the engine and device live on this thread (see `Engine::create`).
            match unsafe { self.next_frame(&mut back) } {
                Ok(Some((width, height))) => {
                    let mut slot = shared.slot.lock();
                    std::mem::swap(&mut slot.data, &mut back);
                    slot.width = width;
                    slot.height = height;
                    slot.fresh = true;
                    delivered = true;
                }
                Ok(None) => {}
                Err(err) => tracing::debug!("video frame transfer for {}: {err}", path.display()),
            }
        }
    }

    /// Applies one command. Returns `false` on [`Command::Shutdown`].
    fn apply(&mut self, cmd: Command, paused: &mut bool) -> bool {
        // SAFETY: engine methods called on its own thread.
        let result = unsafe {
            match cmd {
                Command::Play => {
                    *paused = false;
                    self.engine.Play()
                }
                Command::Pause => {
                    *paused = true;
                    self.engine.Pause()
                }
                Command::Rate(rate) => {
                    self.engine.SetPlaybackRate(f64::from(rate)).and_then(|()| {
                        // Keeps the rate across the engine's internal restart at each loop.
                        self.engine.SetDefaultPlaybackRate(f64::from(rate))
                    })
                }
                Command::Volume(volume) => self.engine.SetVolume(f64::from(volume)),
                Command::Muted(muted) => self.engine.SetMuted(muted),
                Command::Shutdown => return false,
            }
        };
        if let Err(err) = result {
            tracing::debug!("media engine command failed: {err}");
        }
        true
    }

    fn status_for_event(&self, event: u32, param1: usize, param2: u32) -> Option<WinVideoStatus> {
        if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
            let hr = HRESULT(param2 as i32);
            return Some(WinVideoStatus::Failed(format!(
                "Media Foundation error {} ({}, hr {:#010x}); HEVC/VP9 clips need the matching \
                 Microsoft Store video extension",
                media_engine_error_name(param1),
                hr.message(),
                param2
            )));
        }
        if event == MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA.0 as u32
            || event == MF_MEDIA_ENGINE_EVENT_FORMATCHANGE.0 as u32
        {
            // SAFETY: engine queries on its own thread.
            unsafe {
                if !self.engine.HasVideo().as_bool() {
                    return Some(WinVideoStatus::Failed(
                        "no decodable video stream".to_string(),
                    ));
                }
                let (mut width, mut height) = (0u32, 0u32);
                let _ = self
                    .engine
                    .GetNativeVideoSize(Some(&mut width), Some(&mut height));
                return Some(WinVideoStatus::Ready {
                    width,
                    height,
                    has_audio: self.engine.HasAudio().as_bool(),
                });
            }
        }
        None
    }

    /// Transfers the next presentation frame, if the engine has one, into `out` (tightly packed
    /// BGRA). Returns its size.
    ///
    /// # Safety
    /// Must run on the engine's thread.
    unsafe fn next_frame(&mut self, out: &mut Vec<u8>) -> Result<Option<(u32, u32)>> {
        let mut pts = 0i64;
        // Raw vtable call: the safe wrapper maps `S_FALSE` ("no new frame") to `Ok` as well.
        let hr = (Interface::vtable(&self.engine).OnVideoStreamTick)(
            Interface::as_raw(&self.engine),
            &mut pts,
        );
        if hr != S_OK {
            return Ok(None);
        }
        let (mut width, mut height) = (0u32, 0u32);
        self.engine
            .GetNativeVideoSize(Some(&mut width), Some(&mut height))?;
        if width == 0 || height == 0 {
            return Ok(None);
        }
        let (target, staging) = {
            let targets = self.ensure_targets(width, height)?;
            (targets.target.clone(), targets.staging.clone())
        };
        let rect = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        let border = MFARGB {
            rgbBlue: 0,
            rgbGreen: 0,
            rgbRed: 0,
            rgbAlpha: 255,
        };
        self.engine
            .TransferVideoFrame(&target, None, &rect, Some(&border))
            .context("TransferVideoFrame")?;
        self.context.CopyResource(&staging, &target);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        self.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .context("Map staging texture")?;
        let pitch = mapped.RowPitch as usize;
        let row = width as usize * 4;
        let copied = if mapped.pData.is_null() || pitch < row {
            false
        } else {
            let len = pitch * (height as usize - 1) + row;
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, len);
            copy_rows_opaque(out, src, width as usize, height as usize, pitch)
        };
        self.context.Unmap(&staging, 0);
        Ok(copied.then_some((width, height)))
    }

    /// (Re)creates the render target and staging textures for `width x height`.
    unsafe fn ensure_targets(&mut self, width: u32, height: u32) -> Result<&RenderTargets> {
        let stale = !matches!(&self.targets, Some(t) if t.width == width && t.height == height);
        if stale {
            let mut desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut target = None;
            self.device
                .CreateTexture2D(&desc, None, Some(&mut target))
                .context("create video render target")?;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .context("create video staging texture")?;
            self.targets = Some(RenderTargets {
                width,
                height,
                target: target.ok_or_else(|| anyhow!("null render target"))?,
                staging: staging.ok_or_else(|| anyhow!("null staging texture"))?,
            });
        }
        self.targets
            .as_ref()
            .ok_or_else(|| anyhow!("video render targets missing"))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: on the engine's thread; `Shutdown` releases the callback and the device.
        let _ = unsafe { self.engine.Shutdown() };
    }
}

/// `MF_MEDIA_ENGINE_ERR` names.
fn media_engine_error_name(code: usize) -> &'static str {
    match code {
        1 => "MF_MEDIA_ENGINE_ERR_ABORTED",
        2 => "MF_MEDIA_ENGINE_ERR_NETWORK",
        3 => "MF_MEDIA_ENGINE_ERR_DECODE",
        4 => "MF_MEDIA_ENGINE_ERR_SRC_NOT_SUPPORTED",
        5 => "MF_MEDIA_ENGINE_ERR_ENCRYPTED",
        _ => "MF_MEDIA_ENGINE_ERR_UNKNOWN",
    }
}

// ---------------------------------------------------------------------------------------------
// Test clip writer
// ---------------------------------------------------------------------------------------------

/// Encodes a small H.264 MP4 with Media Foundation's sink writer: the top half is red and the
/// bottom half blue, with a white bar that moves one step per frame. Used by the
/// `win_video_probe` example and the ignored decode test. Fails when the H.264 encoder MFT is
/// unavailable (Windows "N" editions without the Media Feature Pack, Server without the Media
/// Foundation feature).
#[doc(hidden)]
pub fn write_test_clip_h264(path: &Path, width: u32, height: u32, frames: u32) -> Result<()> {
    use windows::Win32::Media::MediaFoundation::{
        MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL,
        MFMediaType_Video, MFVideoFormat_H264, MFVideoFormat_RGB32, MFVideoInterlace_Progressive,
        MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
        MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    };
    if width == 0 || height == 0 || (width | height) & 1 != 0 {
        bail!("test clip size must be even and non-zero");
    }
    const FPS: u32 = 30;
    const FRAME_DURATION: i64 = 10_000_000 / FPS as i64;
    let size = (u64::from(width) << 32) | u64::from(height);
    let rate = (u64::from(FPS) << 32) | 1;
    let aspect = (1u64 << 32) | 1;
    let stride = width * 4;
    let wide: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let _guards = start_media_foundation()?;
    // SAFETY: MF is initialized on this thread; every buffer pointer is used within Lock/Unlock.
    unsafe {
        let writer = MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), None, None)
            .context("MFCreateSinkWriterFromURL")?;

        let output = MFCreateMediaType()?;
        output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, 2_000_000)?;
        output.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        output.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
        output.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
        output.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, aspect)?;
        let stream = writer.AddStream(&output).context("H.264 output stream")?;

        let input = MFCreateMediaType()?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        input.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
        input.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
        input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, aspect)?;
        // Positive stride: top-down rows.
        input.SetUINT32(&MF_MT_DEFAULT_STRIDE, stride)?;
        writer
            .SetInputMediaType(stream, &input, None)
            .context("RGB32 -> H.264 input type (is the H.264 encoder installed?)")?;
        writer.BeginWriting().context("BeginWriting")?;

        let len = stride * height;
        let (w, h) = (width as usize, height as usize);
        for frame in 0..frames {
            let buffer = MFCreateMemoryBuffer(len)?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            let pixels = std::slice::from_raw_parts_mut(ptr, len as usize);
            let bar = (frame as usize * 8) % w;
            for y in 0..h {
                for x in 0..w {
                    let px = &mut pixels[(y * w + x) * 4..][..4];
                    let bgra = if x >= bar && x < bar + 8 {
                        [255, 255, 255, 255]
                    } else if y < h / 2 {
                        [0, 0, 255, 255]
                    } else {
                        [255, 0, 0, 255]
                    };
                    px.copy_from_slice(&bgra);
                }
            }
            buffer.Unlock()?;
            buffer.SetCurrentLength(len)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(i64::from(frame) * FRAME_DURATION)?;
            sample.SetSampleDuration(FRAME_DURATION)?;
            writer.WriteSample(stream, &sample).context("WriteSample")?;
        }
        writer.Finalize().context("Finalize")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

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
    fn row_copy_drops_padding_and_forces_alpha() {
        // 2 px wide, 2 rows, pitch 12: 4 bytes of padding per row, none needed after the last.
        let src = [
            1, 2, 3, 0, 4, 5, 6, 7, 9, 9, 9, 9, //
            10, 11, 12, 13, 14, 15, 16, 0,
        ];
        let mut dst = Vec::new();
        assert!(copy_rows_opaque(&mut dst, &src, 2, 2, 12));
        assert_eq!(
            dst,
            [1, 2, 3, 255, 4, 5, 6, 255, 10, 11, 12, 255, 14, 15, 16, 255]
        );
        assert!(!copy_rows_opaque(&mut dst, &src[..19], 2, 2, 12));
        assert!(!copy_rows_opaque(&mut dst, &src, 2, 2, 7));
        assert!(!copy_rows_opaque(&mut dst, &src, 0, 2, 12));
        assert!(!copy_rows_opaque(&mut dst, &src, 2, 0, 12));
    }

    #[test]
    fn open_rejects_missing_files() {
        let missing = std::env::temp_dir().join("truongpaper-win-video-missing-file.mp4");
        let _ = std::fs::remove_file(&missing);
        assert!(WinVideoDecoder::open(&missing).is_err());
        assert!(WinVideoDecoder::open(&std::env::temp_dir()).is_err());
    }

    /// A file Media Foundation cannot parse must fail asynchronously, never crash, and never
    /// produce frames. Hosts without Media Foundation fail in `open` instead, which is also fine.
    #[test]
    fn undecodable_file_fails_without_frames() {
        let path =
            std::env::temp_dir().join(format!("truongpaper-win-video-{}.mp4", std::process::id()));
        std::fs::write(&path, b"\0\0\0\x18ftypisom-not-a-real-movie").unwrap();
        if let Ok(mut decoder) = WinVideoDecoder::open(&path) {
            assert!(decoder.is_paused());
            decoder.set_rate(5.0);
            assert_eq!(decoder.rate(), 2.0);
            decoder.set_volume(0.5);
            decoder.set_muted(true);
            decoder.play();
            assert!(!decoder.is_paused());
            let deadline = Instant::now() + Duration::from_secs(10);
            while decoder.status() == WinVideoStatus::Loading && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(
                matches!(decoder.status(), WinVideoStatus::Failed(_)),
                "status {:?}",
                decoder.status()
            );
            assert!(decoder.copy_latest_bgra_frame().is_none());
            decoder.pause();
            assert!(decoder.is_paused());
        }
        let _ = std::fs::remove_file(&path);
    }

    /// Encodes a 320x240 H.264 clip and decodes 10 frames through the media engine. Ignored by
    /// default because it needs the Media Foundation H.264 encoder and a D3D11 device; run with
    /// `cargo test -p truongpaper-platform -- --ignored win_video`.
    #[test]
    #[ignore]
    fn decodes_generated_h264_clip() {
        let path = std::env::temp_dir().join(format!(
            "truongpaper-win-video-h264-{}.mp4",
            std::process::id()
        ));
        if let Err(err) = write_test_clip_h264(&path, 320, 240, 60) {
            eprintln!("skipping: cannot encode an H.264 test clip here: {err:#}");
            return;
        }
        let result = (|| -> Result<()> {
            let mut decoder = WinVideoDecoder::open(&path)?;
            decoder.set_muted(true);
            decoder.play();
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut frames = 0;
            let mut buf = Vec::new();
            while frames < 10 && Instant::now() < deadline {
                if let Some((w, h, stride)) = decoder.copy_latest_bgra_frame_into(&mut buf) {
                    assert_eq!((w, h, stride), (320, 240, 1280));
                    assert_eq!(buf.len(), 1280 * 240);
                    // Top half red, bottom half blue (BGRA), sampled away from the moving bar.
                    let px = |x: usize, y: usize| &buf[(y * 320 + x) * 4..][..4];
                    let (top, bottom) = (px(300, 40), px(300, 200));
                    assert!(top[2] > 180 && top[0] < 80 && top[3] == 255, "top {top:?}");
                    assert!(bottom[0] > 180 && bottom[2] < 80, "bottom {bottom:?}");
                    frames += 1;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            if frames < 10 {
                bail!(
                    "only {frames} frames decoded, status {:?}",
                    decoder.status()
                );
            }
            Ok(())
        })();
        let _ = std::fs::remove_file(&path);
        result.unwrap();
    }
}
