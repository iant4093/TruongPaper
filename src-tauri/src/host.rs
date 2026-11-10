//! In-process wallpaper host runtime state: the active wallpaper's 4-layer VFS (served over
//! `wpe://`), per-monitor source / compositor uniforms / pause flags / active runtime, and the
//! clean-shutdown hook.
//!
//! Commands record *what* each monitor shows here ([`ActiveRuntime`]); the desktop render thread
//! (`crate::desktop_host`, photos and scenes) and the wallpaper webview manager
//! (`crate::wallpaper_webview`, Web and Video wallpapers) read it and own the live objects
//! (`SceneRuntime` is not `Send`, so it only ever exists on the render thread).

use crate::models::{UniversalAdjustments, WallpaperSource};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use truongpaper_platform::MonitorPlaybackPolicy;
use truongpaper_renderer::{CompositorUniforms, RgbaImageData, WallpaperRuntimeKind};
use truongpaper_we_formats::WeVirtualFileSystem;

/// Runtime that plays a library wallpaper of `project_type` on this OS. Application wallpapers
/// are Windows-only (docs/03 §10); unknown types (e.g. `preset`) are rejected.
pub fn runtime_kind_for(project_type: &str) -> Result<WallpaperRuntimeKind, String> {
    match WallpaperRuntimeKind::from_project_type(project_type) {
        Some(WallpaperRuntimeKind::Application)
            if !truongpaper_platform::application_wallpapers_supported() =>
        {
            Err("Application wallpapers are Windows-only".into())
        }
        Some(kind) => Ok(kind),
        None => Err(format!(
            "Unsupported wallpaper type '{}'",
            project_type.trim()
        )),
    }
}

/// Webview-hosted wallpaper kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebviewKind {
    Web,
    Video,
}

/// What the render thread needs to load a scene wallpaper (it mounts its own VFS).
#[derive(Debug, Clone, PartialEq)]
pub struct SceneDescriptor {
    pub install_dir: PathBuf,
    pub core_assets_dir: PathBuf,
    /// `project.json` `file` (usually `scene.json`).
    pub scene_file: String,
    /// JSON object `{ key: value }` of every user property (project defaults + overrides).
    pub user_properties_json: String,
}

/// The live runtime a monitor is showing.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ActiveRuntime {
    /// Nothing applied since launch (or an unsupported type).
    #[default]
    None,
    /// Photo API source; `path` is `None` until the first photo is assigned.
    Photo { path: Option<PathBuf> },
    /// Scene wallpaper rendered by the desktop render thread.
    Scene(SceneDescriptor),
    /// Web or Video wallpaper in the desktop webview window `label`.
    Webview { kind: WebviewKind, label: String },
    /// Windows Application wallpaper process.
    Application { exe_path: PathBuf },
}

impl ActiveRuntime {
    pub fn is_scene(&self) -> bool {
        matches!(self, ActiveRuntime::Scene(_))
    }

    /// Web pages and scenes consume the system-audio spectrum.
    pub fn consumes_audio(&self) -> bool {
        matches!(
            self,
            ActiveRuntime::Scene(_)
                | ActiveRuntime::Webview {
                    kind: WebviewKind::Web,
                    ..
                }
        )
    }
}

/// Changes that must reach objects owned by the render thread.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderEvent {
    /// Changed user properties (`{ key: value }` JSON object) for scenes of `wallpaper_id`.
    SceneProperties {
        wallpaper_id: String,
        changed_json: String,
    },
}

#[derive(Debug, Clone)]
pub struct MonitorRuntime {
    pub source: WallpaperSource,
    /// Wallpaper whose adjustments feed `uniforms` (`None` for photo sources).
    pub wallpaper_id: Option<String>,
    pub uniforms: CompositorUniforms,
    pub user_paused: bool,
    pub policy: MonitorPlaybackPolicy,
    pub active: ActiveRuntime,
    /// Bumped every time `active` changes; the render thread reloads when it differs.
    pub generation: u64,
    /// Volume / mute / playback speed / audio sensitivity of the shown wallpaper.
    pub adjustments: UniversalAdjustments,
    /// `project.json` declares audio processing (drives the system-audio loopback worker).
    pub audio_reactive: bool,
}

impl Default for MonitorRuntime {
    fn default() -> Self {
        Self {
            source: WallpaperSource::PhotoApi,
            wallpaper_id: None,
            uniforms: CompositorUniforms::default(),
            user_paused: false,
            policy: MonitorPlaybackPolicy::Playing,
            active: ActiveRuntime::None,
            generation: 0,
            adjustments: UniversalAdjustments::default(),
            audio_reactive: false,
        }
    }
}

impl MonitorRuntime {
    /// Should the render loop advance frames for this monitor?
    pub fn should_render(&self) -> bool {
        !self.user_paused && !self.policy.is_paused()
    }
}

#[derive(Default)]
pub struct WallpaperHost {
    active_vfs: RwLock<Option<(String, Arc<WeVirtualFileSystem>)>>,
    /// `wpe://` VFS per wallpaper webview label (`wallpaper-<monitor>`), so each Web / Video page
    /// streams from its own wallpaper folder on mixed multi-monitor setups.
    webview_vfs: RwLock<HashMap<String, (String, Arc<WeVirtualFileSystem>)>>,
    monitors: Mutex<HashMap<String, MonitorRuntime>>,
    /// Pause requested for all monitors (`playback_set_paused` without `monitorId`).
    global_paused: Mutex<bool>,
    /// Set by `desktop_host` once at least one desktop surface renders.
    live_renderer: AtomicBool,
    next_generation: AtomicU64,
    /// Photos decoded by `photo_rotate_next` for the render thread: `monitor -> (generation, image)`.
    pending_photos: Mutex<HashMap<String, (u64, Arc<RgbaImageData>)>>,
    render_events: Mutex<Vec<RenderEvent>>,
    shutting_down: AtomicBool,
    #[cfg(target_os = "windows")]
    applications:
        Mutex<HashMap<String, truongpaper_platform::application::ApplicationWallpaperProcess>>,
}

impl WallpaperHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the desktop render thread created at least one surface (and a GPU device).
    pub fn live_renderer_attached(&self) -> bool {
        self.live_renderer.load(Ordering::Acquire)
    }

    pub fn set_live_renderer_attached(&self, attached: bool) {
        self.live_renderer.store(attached, Ordering::Release);
    }

    /// `true` after [`Self::shutdown`]: the render thread and webview manager exit.
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    /// Records the runtime `monitor_id` now shows (bumping its generation) and returns the
    /// previous one. Any pending decoded photo and (Windows) application process of the
    /// monitor are dropped.
    pub fn set_runtime(
        &self,
        monitor_id: &str,
        active: ActiveRuntime,
        adjustments: UniversalAdjustments,
        audio_reactive: bool,
    ) -> ActiveRuntime {
        self.replace_runtime(monitor_id, active, adjustments, audio_reactive, None)
            .0
    }

    /// Shows the photo at `path` on `monitor_id`; `decoded` hands an already decoded image to
    /// the render thread (it decodes `path` itself otherwise). Returns the new generation.
    pub fn set_photo(
        &self,
        monitor_id: &str,
        path: PathBuf,
        decoded: Option<Arc<RgbaImageData>>,
    ) -> u64 {
        self.replace_runtime(
            monitor_id,
            ActiveRuntime::Photo { path: Some(path) },
            UniversalAdjustments::default(),
            false,
            decoded,
        )
        .1
    }

    fn replace_runtime(
        &self,
        monitor_id: &str,
        active: ActiveRuntime,
        adjustments: UniversalAdjustments,
        audio_reactive: bool,
        decoded: Option<Arc<RgbaImageData>>,
    ) -> (ActiveRuntime, u64) {
        let generation = self.next_generation.fetch_add(1, Ordering::AcqRel) + 1;
        {
            // The image is stored before the new generation becomes visible to the render thread.
            let mut pending = self.pending_photos.lock();
            pending.remove(monitor_id);
            if let Some(img) = decoded {
                pending.insert(monitor_id.to_string(), (generation, img));
            }
        }
        #[cfg(target_os = "windows")]
        if let Some(mut process) = self.applications.lock().remove(monitor_id) {
            process.stop();
        }
        let global = *self.global_paused.lock();
        let mut monitors = self.monitors.lock();
        let rt = monitors
            .entry(monitor_id.to_string())
            .or_insert_with(|| MonitorRuntime {
                user_paused: global,
                ..MonitorRuntime::default()
            });
        rt.generation = generation;
        rt.adjustments = adjustments;
        rt.audio_reactive = audio_reactive;
        let previous = std::mem::replace(&mut rt.active, active);
        if let ActiveRuntime::Webview { label, .. } = &previous {
            let same_window =
                matches!(&rt.active, ActiveRuntime::Webview { label: l, .. } if l == label);
            if !same_window {
                self.webview_vfs.write().remove(label);
            }
        }
        (previous, generation)
    }

    /// Marks `monitor_id` as a Photo API monitor without dropping the photo it already shows.
    pub fn ensure_photo_runtime(&self, monitor_id: &str) {
        let already = self
            .monitors
            .lock()
            .get(monitor_id)
            .is_some_and(|rt| matches!(rt.active, ActiveRuntime::Photo { .. }));
        if !already {
            self.set_runtime(
                monitor_id,
                ActiveRuntime::Photo { path: None },
                UniversalAdjustments::default(),
                false,
            );
        }
    }

    /// Takes the decoded photo stored for `generation` (stale entries are discarded).
    pub fn take_pending_photo(
        &self,
        monitor_id: &str,
        generation: u64,
    ) -> Option<Arc<RgbaImageData>> {
        let mut pending = self.pending_photos.lock();
        match pending.remove(monitor_id) {
            Some((g, img)) if g == generation => Some(img),
            _ => None,
        }
    }

    /// Clones every monitor's runtime state (render loop / webview manager tick).
    pub fn monitors_snapshot(&self) -> Vec<(String, MonitorRuntime)> {
        let mut out: Vec<(String, MonitorRuntime)> = self
            .monitors
            .lock()
            .iter()
            .map(|(id, rt)| (id.clone(), rt.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Monitors currently showing `wallpaper_id`.
    pub fn monitors_showing(&self, wallpaper_id: &str) -> Vec<(String, MonitorRuntime)> {
        self.monitors_snapshot()
            .into_iter()
            .filter(|(_, rt)| rt.wallpaper_id.as_deref() == Some(wallpaper_id))
            .collect()
    }

    /// Updates adjustments and compositor uniforms on every monitor showing `wallpaper_id`.
    pub fn update_adjustments_for_wallpaper(
        &self,
        wallpaper_id: &str,
        adjustments: UniversalAdjustments,
        uniforms: CompositorUniforms,
    ) {
        for rt in self.monitors.lock().values_mut() {
            if rt.wallpaper_id.as_deref() == Some(wallpaper_id) {
                rt.adjustments = adjustments;
                rt.uniforms = uniforms;
            }
        }
    }

    /// Whether any playing monitor shows an audio-reactive scene or Web wallpaper, and the
    /// audio sensitivity of the first such monitor (docs/01 §6.1 item 4).
    pub fn audio_demand(&self) -> Option<f32> {
        self.monitors_snapshot()
            .into_iter()
            .find(|(_, rt)| rt.should_render() && rt.audio_reactive && rt.active.consumes_audio())
            .map(|(_, rt)| rt.adjustments.audio_sensitivity)
    }

    /// Queues an event for the render thread (dropped when no renderer is attached).
    pub fn push_render_event(&self, event: RenderEvent) {
        if self.live_renderer_attached() {
            let mut events = self.render_events.lock();
            // Bounded: a stalled render thread must not grow memory without limit.
            if events.len() >= 256 {
                events.remove(0);
            }
            events.push(event);
        }
    }

    pub fn drain_render_events(&self) -> Vec<RenderEvent> {
        std::mem::take(&mut *self.render_events.lock())
    }

    /// Merges changed user properties into the scene descriptors of monitors showing
    /// `wallpaper_id`, so a reload (e.g. after a display change) keeps them. The generation is
    /// not bumped: the live scene gets the change through [`RenderEvent::SceneProperties`].
    pub fn merge_scene_properties(
        &self,
        wallpaper_id: &str,
        changed: &serde_json::Map<String, serde_json::Value>,
    ) {
        for rt in self.monitors.lock().values_mut() {
            if rt.wallpaper_id.as_deref() != Some(wallpaper_id) {
                continue;
            }
            if let ActiveRuntime::Scene(desc) = &mut rt.active {
                let mut props: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(&desc.user_properties_json).unwrap_or_default();
                for (k, v) in changed {
                    props.insert(k.clone(), v.clone());
                }
                desc.user_properties_json = serde_json::Value::Object(props).to_string();
            }
        }
    }

    /// Keeps the running Application wallpaper process of `monitor_id` (killed when the monitor
    /// switches runtime or on shutdown).
    #[cfg(target_os = "windows")]
    pub fn set_application_process(
        &self,
        monitor_id: &str,
        process: truongpaper_platform::application::ApplicationWallpaperProcess,
    ) {
        if let Some(mut old) = self
            .applications
            .lock()
            .insert(monitor_id.to_string(), process)
        {
            old.stop();
        }
    }

    pub fn set_active_vfs(&self, wallpaper_id: &str, vfs: Arc<WeVirtualFileSystem>) {
        *self.active_vfs.write() = Some((wallpaper_id.to_string(), vfs));
    }

    /// Mounts `vfs` (of `wallpaper_id`) for the wallpaper webview `label`. Dropped when the
    /// monitor of that window switches to another runtime, on unmount and on shutdown.
    pub fn set_webview_vfs(&self, label: &str, wallpaper_id: &str, vfs: Arc<WeVirtualFileSystem>) {
        self.webview_vfs
            .write()
            .insert(label.to_string(), (wallpaper_id.to_string(), vfs));
    }

    /// The VFS `wpe://` serves to webview `label`: its own wallpaper's VFS, else the active one.
    pub fn vfs_for_webview(&self, label: &str) -> Option<Arc<WeVirtualFileSystem>> {
        self.webview_vfs
            .read()
            .get(label)
            .map(|(_, v)| Arc::clone(v))
            .or_else(|| self.active_vfs())
    }

    pub fn active_vfs(&self) -> Option<Arc<WeVirtualFileSystem>> {
        self.active_vfs.read().as_ref().map(|(_, v)| Arc::clone(v))
    }

    pub fn active_wallpaper_id(&self) -> Option<String> {
        self.active_vfs.read().as_ref().map(|(id, _)| id.clone())
    }

    /// Unmounts every VFS (active and per-webview) that belongs to `wallpaper_id`.
    pub fn unmount_if(&self, wallpaper_id: &str) {
        self.webview_vfs
            .write()
            .retain(|_, (id, _)| id != wallpaper_id);
        let mut guard = self.active_vfs.write();
        if guard.as_ref().is_some_and(|(id, _)| id == wallpaper_id) {
            *guard = None;
        }
    }

    pub fn set_source(
        &self,
        monitor_id: &str,
        source: WallpaperSource,
        wallpaper_id: Option<String>,
        uniforms: CompositorUniforms,
    ) {
        let global = *self.global_paused.lock();
        let mut monitors = self.monitors.lock();
        let rt = monitors
            .entry(monitor_id.to_string())
            .or_insert_with(|| MonitorRuntime {
                user_paused: global,
                ..MonitorRuntime::default()
            });
        rt.source = source;
        rt.wallpaper_id = wallpaper_id;
        rt.uniforms = uniforms;
    }

    /// Updates compositor uniforms on every monitor currently showing `wallpaper_id`.
    pub fn update_uniforms_for_wallpaper(&self, wallpaper_id: &str, uniforms: CompositorUniforms) {
        for rt in self.monitors.lock().values_mut() {
            if rt.wallpaper_id.as_deref() == Some(wallpaper_id) {
                rt.uniforms = uniforms;
            }
        }
    }

    /// `monitor_id = None` pauses/resumes every monitor (and future ones).
    pub fn set_user_paused(&self, monitor_id: Option<&str>, paused: bool) {
        let mut monitors = self.monitors.lock();
        match monitor_id {
            Some(id) => {
                monitors.entry(id.to_string()).or_default().user_paused = paused;
            }
            None => {
                *self.global_paused.lock() = paused;
                for rt in monitors.values_mut() {
                    rt.user_paused = paused;
                }
            }
        }
    }

    pub fn set_policy(&self, monitor_id: &str, policy: MonitorPlaybackPolicy) {
        self.monitors
            .lock()
            .entry(monitor_id.to_string())
            .or_default()
            .policy = policy;
    }

    pub fn monitor(&self, monitor_id: &str) -> Option<MonitorRuntime> {
        self.monitors.lock().get(monitor_id).cloned()
    }

    /// [`MonitorRuntime::should_render`] for `monitor_id`; a monitor without runtime state yet
    /// (nothing applied since launch) follows the global pause flag.
    pub fn monitor_should_render(&self, monitor_id: &str) -> bool {
        match self.monitors.lock().get(monitor_id) {
            Some(rt) => rt.should_render(),
            None => !*self.global_paused.lock(),
        }
    }

    /// Drops every mounted VFS and per-monitor runtime, kills Application wallpaper processes
    /// and tells the render thread / webview manager to stop (update relaunch / app exit).
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        *self.active_vfs.write() = None;
        self.webview_vfs.write().clear();
        self.monitors.lock().clear();
        self.pending_photos.lock().clear();
        self.render_events.lock().clear();
        #[cfg(target_os = "windows")]
        for (_, mut process) in self.applications.lock().drain() {
            process.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_flags_and_policy_combine() {
        let host = WallpaperHost::new();
        host.set_source(
            "A",
            WallpaperSource::PhotoApi,
            None,
            CompositorUniforms::default(),
        );
        assert!(host.monitor("A").unwrap().should_render());
        host.set_user_paused(None, true);
        assert!(!host.monitor("A").unwrap().should_render());
        // Monitors that appear after a global pause start paused too.
        host.set_source(
            "B",
            WallpaperSource::PhotoApi,
            None,
            CompositorUniforms::default(),
        );
        assert!(host.monitor("B").unwrap().user_paused);
        host.set_user_paused(Some("A"), false);
        assert!(host.monitor("A").unwrap().should_render());
        host.set_policy("A", MonitorPlaybackPolicy::PausedFullscreen);
        assert!(!host.monitor("A").unwrap().should_render());
        assert!(!host.monitor_should_render("A"));
        assert!(
            !host.monitor_should_render("unknown"),
            "global pause applies"
        );
        host.set_user_paused(None, false);
        assert!(host.monitor_should_render("unknown"));
        assert!(host.monitor_should_render("B"));
    }

    #[test]
    fn uniforms_follow_wallpaper_and_shutdown_clears() {
        let host = WallpaperHost::new();
        let ws = WallpaperSource::Workshop {
            wallpaper_id: "workshop:1".into(),
        };
        host.set_source(
            "A",
            ws,
            Some("workshop:1".into()),
            CompositorUniforms::default(),
        );
        let u = CompositorUniforms {
            zoom: 1.7,
            ..Default::default()
        };
        host.update_uniforms_for_wallpaper("workshop:1", u);
        assert_eq!(host.monitor("A").unwrap().uniforms.zoom, 1.7);
        host.update_uniforms_for_wallpaper("workshop:2", CompositorUniforms::default());
        assert_eq!(host.monitor("A").unwrap().uniforms.zoom, 1.7);

        let dir = std::env::temp_dir();
        host.set_active_vfs(
            "workshop:1",
            Arc::new(WeVirtualFileSystem::mount(dir.clone(), dir)),
        );
        assert_eq!(host.active_wallpaper_id().as_deref(), Some("workshop:1"));
        host.unmount_if("workshop:2");
        assert!(host.active_vfs().is_some());
        host.shutdown();
        assert!(host.active_vfs().is_none());
        assert!(host.monitor("A").is_none());
    }

    fn scene() -> ActiveRuntime {
        ActiveRuntime::Scene(SceneDescriptor {
            install_dir: PathBuf::from("/w/1"),
            core_assets_dir: PathBuf::from("/core"),
            scene_file: "scene.json".into(),
            user_properties_json: "{}".into(),
        })
    }

    #[test]
    fn runtime_kind_follows_project_type_and_os() {
        assert_eq!(runtime_kind_for("scene"), Ok(WallpaperRuntimeKind::Scene));
        assert_eq!(runtime_kind_for(" Video "), Ok(WallpaperRuntimeKind::Video));
        assert_eq!(runtime_kind_for("web"), Ok(WallpaperRuntimeKind::Web));
        assert_eq!(
            runtime_kind_for("application").is_ok(),
            cfg!(target_os = "windows")
        );
        assert!(runtime_kind_for("preset").is_err());
        assert!(runtime_kind_for("").is_err());
    }

    #[test]
    fn live_renderer_flag_round_trips() {
        let host = WallpaperHost::new();
        assert!(!host.live_renderer_attached());
        host.set_live_renderer_attached(true);
        assert!(host.live_renderer_attached());
        host.set_live_renderer_attached(false);
        assert!(!host.live_renderer_attached());
    }

    #[test]
    fn switching_scene_to_photo_drops_the_scene_runtime() {
        let host = WallpaperHost::new();
        let prev = host.set_runtime("A", scene(), UniversalAdjustments::default(), true);
        assert_eq!(prev, ActiveRuntime::None);
        let scene_gen = host.monitor("A").unwrap().generation;
        assert!(host.monitor("A").unwrap().active.is_scene());

        // A Photo API source replaces the scene; the generation moves on.
        host.ensure_photo_runtime("A");
        let rt = host.monitor("A").unwrap();
        assert_eq!(rt.active, ActiveRuntime::Photo { path: None });
        assert!(rt.generation > scene_gen);
        assert!(!rt.audio_reactive);

        // Assigning a photo keeps the decoded image only for that generation.
        let img = Arc::new(RgbaImageData {
            width: 1,
            height: 1,
            pixels: vec![0, 0, 0, 255],
        });
        let g = host.set_photo("A", PathBuf::from("/p/1.jpg"), Some(img.clone()));
        assert_eq!(
            host.monitor("A").unwrap().active,
            ActiveRuntime::Photo {
                path: Some(PathBuf::from("/p/1.jpg"))
            }
        );
        assert!(
            host.take_pending_photo("A", g - 1).is_none(),
            "stale generation"
        );
        let g = host.set_photo("A", PathBuf::from("/p/2.jpg"), Some(img));
        assert!(host.take_pending_photo("A", g).is_some());
        assert!(host.take_pending_photo("A", g).is_none(), "taken once");

        // ensure_photo_runtime keeps the photo already on screen.
        host.ensure_photo_runtime("A");
        assert_eq!(host.monitor("A").unwrap().generation, g);
    }

    #[test]
    fn audio_demand_needs_a_playing_audio_reactive_consumer() {
        let host = WallpaperHost::new();
        assert_eq!(host.audio_demand(), None);
        let adj = UniversalAdjustments {
            audio_sensitivity: 2.5,
            ..UniversalAdjustments::default()
        };
        host.set_runtime(
            "V",
            ActiveRuntime::Webview {
                kind: WebviewKind::Video,
                label: "wallpaper-V".into(),
            },
            adj,
            true,
        );
        assert_eq!(
            host.audio_demand(),
            None,
            "video pages do not consume audio"
        );
        host.set_runtime("A", scene(), adj, false);
        assert_eq!(host.audio_demand(), None, "not audio reactive");
        host.set_runtime("A", scene(), adj, true);
        assert_eq!(host.audio_demand(), Some(2.5));
        host.set_policy("A", MonitorPlaybackPolicy::PausedMaximized);
        assert_eq!(host.audio_demand(), None, "all consumers paused");
        host.set_policy("A", MonitorPlaybackPolicy::Playing);
        host.set_user_paused(None, true);
        assert_eq!(host.audio_demand(), None);
    }

    #[test]
    fn render_events_need_an_attached_renderer_and_shutdown_clears_everything() {
        let host = WallpaperHost::new();
        let ev = RenderEvent::SceneProperties {
            wallpaper_id: "w".into(),
            changed_json: "{}".into(),
        };
        host.push_render_event(ev.clone());
        assert!(host.drain_render_events().is_empty());
        host.set_live_renderer_attached(true);
        host.push_render_event(ev.clone());
        assert_eq!(host.drain_render_events(), vec![ev.clone()]);
        assert!(host.drain_render_events().is_empty());

        host.set_runtime("A", scene(), UniversalAdjustments::default(), true);
        host.set_photo("B", PathBuf::from("/p.jpg"), None);
        host.push_render_event(ev);
        assert_eq!(host.monitors_snapshot().len(), 2);
        assert!(!host.is_shutting_down());
        host.shutdown();
        assert!(host.is_shutting_down());
        assert!(host.monitors_snapshot().is_empty());
        assert!(host.drain_render_events().is_empty());
    }

    #[test]
    fn webview_vfs_is_per_window_and_follows_the_runtime() {
        let host = WallpaperHost::new();
        let dir = std::env::temp_dir();
        let mount = || Arc::new(WeVirtualFileSystem::mount(dir.clone(), dir.clone()));
        let (va, vb, vs) = (mount(), mount(), mount());
        let web = |label: &str| ActiveRuntime::Webview {
            kind: WebviewKind::Web,
            label: label.into(),
        };
        assert!(host.vfs_for_webview("wallpaper-A").is_none());

        host.set_runtime(
            "A",
            web("wallpaper-A"),
            UniversalAdjustments::default(),
            false,
        );
        host.set_webview_vfs("wallpaper-A", "wa", va.clone());
        host.set_runtime(
            "B",
            web("wallpaper-B"),
            UniversalAdjustments::default(),
            false,
        );
        host.set_webview_vfs("wallpaper-B", "wb", vb.clone());
        host.set_active_vfs("scene", vs.clone());
        // Each page keeps its own wallpaper folder whatever was mounted last.
        assert!(Arc::ptr_eq(
            &host.vfs_for_webview("wallpaper-A").unwrap(),
            &va
        ));
        assert!(Arc::ptr_eq(
            &host.vfs_for_webview("wallpaper-B").unwrap(),
            &vb
        ));
        assert!(Arc::ptr_eq(&host.vfs_for_webview("main").unwrap(), &vs));

        // Same window, next Web wallpaper: the entry survives until it is replaced.
        host.set_runtime(
            "A",
            web("wallpaper-A"),
            UniversalAdjustments::default(),
            false,
        );
        assert!(Arc::ptr_eq(
            &host.vfs_for_webview("wallpaper-A").unwrap(),
            &va
        ));
        // Switching the monitor to a scene drops the window's VFS.
        host.set_runtime("A", scene(), UniversalAdjustments::default(), false);
        assert!(Arc::ptr_eq(
            &host.vfs_for_webview("wallpaper-A").unwrap(),
            &vs
        ));

        host.unmount_if("wb");
        assert!(Arc::ptr_eq(
            &host.vfs_for_webview("wallpaper-B").unwrap(),
            &vs
        ));
        host.set_webview_vfs("wallpaper-B", "wb", vb);
        host.shutdown();
        assert!(host.vfs_for_webview("wallpaper-B").is_none());
    }

    #[test]
    fn adjustments_follow_wallpaper() {
        let host = WallpaperHost::new();
        host.set_source(
            "A",
            WallpaperSource::Workshop {
                wallpaper_id: "w".into(),
            },
            Some("w".into()),
            CompositorUniforms::default(),
        );
        host.set_runtime("A", scene(), UniversalAdjustments::default(), false);
        let adj = UniversalAdjustments {
            volume: 0.25,
            ..UniversalAdjustments::default()
        };
        let u = CompositorUniforms {
            zoom: 1.5,
            ..Default::default()
        };
        host.update_adjustments_for_wallpaper("w", adj, u);
        let rt = host.monitor("A").unwrap();
        assert_eq!(rt.adjustments.volume, 0.25);
        assert_eq!(rt.uniforms.zoom, 1.5);
        assert_eq!(host.monitors_showing("w").len(), 1);
        assert!(host.monitors_showing("x").is_empty());
    }
}
