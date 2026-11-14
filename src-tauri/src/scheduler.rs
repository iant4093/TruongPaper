//! Unified Playlist & Rotation Scheduler (docs/01 §1, docs/04 §2.4 "Rotation Interval & Trigger
//! Controls").
//!
//! - Pure part: [`next_due`], [`next_playlist_index`], [`monitor_interval_secs`],
//!   [`build_monitor_ticks`] and the [`RotationScheduler`] state machine decide which monitor
//!   rotates and why ([`RotationTrigger`]).
//! - Runtime part: [`spawn`] polls every [`TICK_INTERVAL`] and runs the planned actions through the
//!   same helpers the IPC commands use (`commands::monitor::{apply_source, show_photo,
//!   show_playlist_item}`, `commands::photo::rotate_photo_next`). No IPC commands are added.
//!
//! Intervals: Photo API monitors use `AppSettings::rotation_interval_seconds`, playlist monitors
//! use `Playlist::interval_seconds`, single Workshop monitors never rotate. `0` means
//! "Never (Manual Only)": no timer, launch or wake rotation (launch still restores the persisted
//! wallpaper). Paused monitors (`WallpaperHost::monitor_should_render() == false`, or the desktop
//! asleep/locked) do not rotate; an overdue timer or a pending wake rotation fires once they are
//! visible again. Every display keeps its own position and timer; positions are persisted in
//! the `playlist_positions` table and restored at launch. The runtime loop does cheap atomic
//! checks each second and only reads the database when something is due or changed.

use crate::commands::monitor::{
    apply_source, first_playlist_index, live_monitors, lock_monitor_apply, show_photo,
    show_playlist_item,
};
use crate::commands::photo::{rotate_photo_next, PhotoServiceState};
use crate::models::{Playlist, PlaylistItem, PlaylistMode, WallpaperSource};
use crate::state::{AppState, SETTING_MULTI_MONITOR_MODE};
use parking_lot::Mutex;
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// Scheduler poll period.
pub const TICK_INTERVAL: Duration = Duration::from_secs(1);
/// Delay before the first tick ("Rotate on App Launch") so the event loop and windows are up.
pub const LAUNCH_DELAY: Duration = Duration::from_secs(2);
/// The live monitor list needs a main-thread round trip; refresh it every N ticks.
const LIVE_MONITOR_REFRESH_TICKS: u32 = 5;
/// A paused / hidden monitor is re-checked this often for becoming visible again.
const NOT_RENDERABLE_RECHECK: Duration = Duration::from_secs(10);
/// Safety net: a full tick (database read) at least this often, to notice external changes.
const MAX_IDLE_BETWEEN_FULL_TICKS: Duration = Duration::from_secs(30);
/// Bound for queued manual notes when no scheduler is draining them (tests, early startup).
const MAX_PENDING_MANUAL_NOTES: usize = 256;
/// One rotation (apply, including the macOS web ready wait) may not stall the others longer.
const PLAN_TIMEOUT: Duration = Duration::from_secs(30);
/// Pause before the supervisor restarts a scheduler loop that panicked.
const RESTART_DELAY: Duration = Duration::from_secs(5);

/// Why a monitor rotates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationTrigger {
    /// The monitor's interval elapsed.
    Timer,
    /// First scheduler tick after the app started (also used the first time a hot-plugged
    /// monitor is seen, which is always a restore).
    AppLaunch,
    /// The desktop became visible again (wake from sleep, display wake, unlock).
    DisplayWake,
    /// The user rotated or applied a wallpaper; only restarts the monitor's timer.
    Manual,
}

/// Next instant (never earlier than `now`) at which a monitor rotated at `last_rotation` is due,
/// or `None` when `interval_secs == 0` ("Never (Manual Only)").
pub fn next_due(now: Instant, last_rotation: Instant, interval_secs: u64) -> Option<Instant> {
    if interval_secs == 0 {
        return None;
    }
    let due = last_rotation.checked_add(Duration::from_secs(interval_secs))?;
    Some(due.max(now))
}

/// `true` when [`next_due`] is `now` or earlier.
pub fn is_due(now: Instant, last_rotation: Instant, interval_secs: u64) -> bool {
    next_due(now, last_rotation, interval_secs).is_some_and(|due| due <= now)
}

/// Whether `mode` plays `item`: `PhotosOnly` skips Workshop items, `WorkshopOnly` skips Photo API
/// draws, `Mixed` plays everything.
pub fn playlist_item_allowed(mode: PlaylistMode, item: &PlaylistItem) -> bool {
    !matches!(
        (mode, item),
        (PlaylistMode::PhotosOnly, PlaylistItem::Workshop { .. })
            | (PlaylistMode::WorkshopOnly, PlaylistItem::PhotoApiDraw)
    )
}

/// Index after `current`, cycling through `playlist.items` and skipping items the mode does not
/// allow. Empty playlists give `0`; when no item is allowed `current` (wrapped) is returned, so
/// callers must still check [`playlist_item_allowed`].
pub fn next_playlist_index(playlist: &Playlist, current: usize) -> usize {
    let len = playlist.items.len();
    if len == 0 {
        return 0;
    }
    let current = current % len;
    (1..=len)
        .map(|step| (current + step) % len)
        .find(|&idx| playlist_item_allowed(playlist.mode, &playlist.items[idx]))
        .unwrap_or(current)
}

/// Rotation interval for a monitor showing `source` (`0` = never).
pub fn monitor_interval_secs(
    source: &WallpaperSource,
    settings_interval_secs: u64,
    playlist: Option<&Playlist>,
) -> u64 {
    match source {
        WallpaperSource::PhotoApi => settings_interval_secs,
        WallpaperSource::Playlist { .. } => playlist.map_or(0, |p| p.interval_seconds),
        WallpaperSource::Workshop { .. } => 0,
    }
}

/// Snapshot of one monitor for [`RotationScheduler::plan`].
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorTick {
    pub monitor_id: String,
    pub source: WallpaperSource,
    /// Result of [`monitor_interval_secs`].
    pub interval_secs: u64,
    /// `WallpaperHost::monitor_should_render` and the desktop is not asleep/locked.
    pub renderable: bool,
    /// The playlist behind a `Playlist` source (`None` when deleted).
    pub playlist: Option<Playlist>,
}

/// Builds the per-monitor snapshot for every live monitor (unassigned ones show the Photo API,
/// as in `photo_rotate_next`), sorted by id. Assigned monitors that are not connected are left
/// out: nothing is restored into a window on another screen and no photo is marked shown for
/// a display nobody sees. Their current playlist item is restored once they are connected again.
pub fn build_monitor_ticks(
    live: &[String],
    assigned: &HashMap<String, WallpaperSource>,
    playlists: &[Playlist],
    settings_interval_secs: u64,
    renderable: impl Fn(&str) -> bool,
) -> Vec<MonitorTick> {
    let ids: BTreeSet<&String> = live.iter().collect();
    ids.into_iter()
        .map(|id| {
            let source = assigned.get(id).cloned().unwrap_or_default();
            let playlist = match &source {
                WallpaperSource::Playlist { playlist_id } => playlists
                    .iter()
                    .find(|p| &p.playlist_id == playlist_id)
                    .cloned(),
                _ => None,
            };
            MonitorTick {
                monitor_id: id.clone(),
                interval_secs: monitor_interval_secs(
                    &source,
                    settings_interval_secs,
                    playlist.as_ref(),
                ),
                renderable: renderable(id),
                source,
                playlist,
            }
        })
        .collect()
}

/// What to do for a monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotationAction {
    /// Show the persisted source again without drawing anything new (launch with "Never",
    /// paused monitors at launch, single Workshop items, hot-plugged monitors).
    Restore,
    /// Draw the next Photo API photo.
    NextPhoto,
    /// Show playlist item `index`.
    ShowPlaylistItem {
        playlist_id: String,
        index: usize,
        item: PlaylistItem,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRotation {
    pub monitor_id: String,
    pub source: WallpaperSource,
    pub trigger: RotationTrigger,
    pub action: RotationAction,
}

#[derive(Debug, Clone)]
struct MonitorSchedule {
    source: WallpaperSource,
    last_rotation: Instant,
    playlist_index: usize,
    wake_pending: bool,
    restore_pending: bool,
}

/// A manual rotation reported by an IPC command (see [`note_manual_rotation`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualNote {
    /// `None` = every Photo API monitor (`photo_rotate_next` without `monitorId`).
    pub monitor_id: Option<String>,
    /// `monitor_apply_wallpaper` showed the playlist's first item.
    pub restart_playlist: bool,
}

/// Per-monitor rotation state machine (pure; driven by [`spawn`]).
#[derive(Debug)]
pub struct RotationScheduler {
    monitors: HashMap<String, MonitorSchedule>,
    launched: bool,
    wake_generation: u64,
    manual: Vec<ManualNote>,
    /// Monitors whose next playlist item was requested by the user (hotkey / tray).
    forced: Vec<String>,
    /// Last known persisted `monitor -> (playlist, index)`; seeds new monitors at launch.
    positions: HashMap<String, (String, usize)>,
    /// Positions that changed since [`Self::take_position_changes`].
    position_changes: Vec<(String, String, usize)>,
    /// Earliest instant at which [`Self::plan`] could have work again (`None` = nothing timed).
    next_wake: Option<Instant>,
}

impl RotationScheduler {
    /// `wake_generation` is the current `power::wake_generation()`.
    pub fn new(wake_generation: u64) -> Self {
        Self {
            monitors: HashMap::new(),
            launched: false,
            wake_generation,
            manual: Vec::new(),
            forced: Vec::new(),
            positions: HashMap::new(),
            position_changes: Vec::new(),
            next_wake: None,
        }
    }

    /// Loads persisted per-display playlist positions (call before the first [`Self::plan`]).
    pub fn restore_positions(&mut self, positions: HashMap<String, (String, usize)>) {
        self.positions = positions;
    }

    /// Positions to persist: `(monitor_id, playlist_id, index)`.
    pub fn take_position_changes(&mut self) -> Vec<(String, String, usize)> {
        std::mem::take(&mut self.position_changes)
    }

    /// When the next timed rotation (or a visibility re-check) is due after the last plan.
    pub fn next_wake(&self) -> Option<Instant> {
        self.next_wake
    }

    /// The user asked for the next item of `monitor_id` now; applied by the next [`Self::plan`].
    pub fn note_force_next(&mut self, monitor_id: &str) {
        self.forced.push(monitor_id.to_string());
    }

    /// Records a manual rotation; applied by the next [`Self::plan`].
    pub fn note_manual(&mut self, note: ManualNote) {
        self.manual.push(note);
    }

    /// Preserve playlist position while disconnected, but restore the closed window on replug.
    pub fn mark_disconnected(&mut self, monitor_id: &str) {
        if let Some(schedule) = self.monitors.get_mut(monitor_id) {
            schedule.restore_pending = true;
        }
    }

    /// Current playlist position of a monitor.
    pub fn playlist_index(&self, monitor_id: &str) -> Option<usize> {
        self.monitors.get(monitor_id).map(|s| s.playlist_index)
    }

    /// Decides this tick's rotations and marks them as done at `now` (failures are not retried
    /// before the next interval, so a missing wallpaper is not re-applied every second).
    pub fn plan(
        &mut self,
        now: Instant,
        monitors: &[MonitorTick],
        wake_generation: u64,
        clone_photos: bool,
    ) -> Vec<PlannedRotation> {
        if !self.launched && monitors.is_empty() {
            // The monitor list is not known yet: keep "Rotate on App Launch" for the first
            // tick that sees a display.
            return Vec::new();
        }
        let launch = !std::mem::replace(&mut self.launched, true);
        let woke = std::mem::replace(&mut self.wake_generation, wake_generation) != wake_generation;
        let manual = std::mem::take(&mut self.manual);
        let forced = std::mem::take(&mut self.forced);
        for (id, schedule) in &mut self.monitors {
            if !monitors.iter().any(|m| &m.monitor_id == id) {
                schedule.restore_pending = true;
            }
        }

        let mut out = Vec::new();
        for m in monitors {
            let first_index = m
                .playlist
                .as_ref()
                .and_then(first_playlist_index)
                .unwrap_or(0);
            let is_new = !self.monitors.contains_key(&m.monitor_id);
            let saved_index = match (&m.source, &m.playlist, self.positions.get(&m.monitor_id)) {
                (
                    WallpaperSource::Playlist { playlist_id },
                    Some(playlist),
                    Some((saved_id, idx)),
                ) if saved_id == playlist_id
                    && playlist
                        .items
                        .get(*idx)
                        .is_some_and(|i| playlist_item_allowed(playlist.mode, i)) =>
                {
                    Some(*idx)
                }
                _ => None,
            };
            let seeded = is_new && saved_index.is_some();
            let s = self
                .monitors
                .entry(m.monitor_id.clone())
                .or_insert_with(|| MonitorSchedule {
                    source: m.source.clone(),
                    last_rotation: now,
                    playlist_index: saved_index.unwrap_or(first_index),
                    wake_pending: false,
                    restore_pending: false,
                });

            let notes: Vec<&ManualNote> = manual
                .iter()
                .filter(|n| match &n.monitor_id {
                    Some(id) => id == &m.monitor_id,
                    None => m.source == WallpaperSource::PhotoApi,
                })
                .collect();
            if s.source != m.source {
                // Someone (monitor_apply_wallpaper) already showed the new source.
                s.source = m.source.clone();
                s.last_rotation = now;
                s.playlist_index = first_index;
                s.wake_pending = false;
                if !s.restore_pending {
                    continue;
                }
            }
            if !notes.is_empty() {
                s.last_rotation = now;
                s.wake_pending = false;
                if notes.iter().any(|n| n.restart_playlist) {
                    s.playlist_index = first_index;
                }
                if !s.restore_pending {
                    continue;
                }
            }
            if woke && m.interval_secs > 0 {
                s.wake_pending = true;
            }

            if launch || is_new || s.restore_pending {
                let restore = if s.restore_pending || seeded {
                    restored_playlist_action(s, m).unwrap_or(RotationAction::Restore)
                } else {
                    RotationAction::Restore
                };
                s.restore_pending = false;
                let rotate = launch && m.interval_secs > 0 && m.renderable;
                let action = rotate
                    .then(|| rotation_action(s, m, true))
                    .flatten()
                    .unwrap_or(restore);
                s.last_rotation = now;
                s.wake_pending = false;
                out.push(PlannedRotation {
                    monitor_id: m.monitor_id.clone(),
                    source: m.source.clone(),
                    trigger: RotationTrigger::AppLaunch,
                    action,
                });
                continue;
            }
            if forced.contains(&m.monitor_id) {
                // User-requested "next": advance now, even while paused or set to "Never".
                s.last_rotation = now;
                s.wake_pending = false;
                if let Some(action) = rotation_action(s, m, false) {
                    out.push(PlannedRotation {
                        monitor_id: m.monitor_id.clone(),
                        source: m.source.clone(),
                        trigger: RotationTrigger::Manual,
                        action,
                    });
                }
                continue;
            }
            if m.interval_secs == 0 {
                s.wake_pending = false;
                continue;
            }
            if !m.renderable {
                continue;
            }
            let trigger = if s.wake_pending {
                RotationTrigger::DisplayWake
            } else if is_due(now, s.last_rotation, m.interval_secs) {
                RotationTrigger::Timer
            } else {
                continue;
            };
            s.last_rotation = now;
            s.wake_pending = false;
            if let Some(action) = rotation_action(s, m, false) {
                out.push(PlannedRotation {
                    monitor_id: m.monitor_id.clone(),
                    source: m.source.clone(),
                    trigger,
                    action,
                });
            }
        }

        // Clone mode: one draw is mirrored to every Photo API monitor, so their timers restart.
        let cloned_draw = clone_photos
            && out.iter().any(|p| {
                p.action == RotationAction::NextPhoto && p.source == WallpaperSource::PhotoApi
            });
        if cloned_draw {
            for m in monitors
                .iter()
                .filter(|m| m.source == WallpaperSource::PhotoApi)
            {
                if let Some(s) = self.monitors.get_mut(&m.monitor_id) {
                    s.last_rotation = now;
                    s.wake_pending = false;
                }
            }
        }

        let mut next: Option<Instant> = None;
        for m in monitors {
            let Some(s) = self.monitors.get(&m.monitor_id) else {
                continue;
            };
            if let WallpaperSource::Playlist { playlist_id } = &m.source {
                let current = (playlist_id.clone(), s.playlist_index);
                if m.playlist.is_some() && self.positions.get(&m.monitor_id) != Some(&current) {
                    self.position_changes.push((
                        m.monitor_id.clone(),
                        current.0.clone(),
                        current.1,
                    ));
                    self.positions.insert(m.monitor_id.clone(), current);
                }
            }
            if m.interval_secs == 0 || matches!(m.source, WallpaperSource::Workshop { .. }) {
                continue;
            }
            let due = if !m.renderable || s.wake_pending {
                now + NOT_RENDERABLE_RECHECK
            } else {
                s.last_rotation + Duration::from_secs(m.interval_secs)
            };
            next = Some(next.map_or(due, |n| n.min(due)));
        }
        self.next_wake = next;
        out
    }
}

/// Reconnects resume the current playlist item without consuming another draw/position.
fn restored_playlist_action(s: &MonitorSchedule, m: &MonitorTick) -> Option<RotationAction> {
    let WallpaperSource::Playlist { playlist_id } = &m.source else {
        return None;
    };
    let playlist = m.playlist.as_ref()?;
    let index = if playlist
        .items
        .get(s.playlist_index)
        .is_some_and(|item| playlist_item_allowed(playlist.mode, item))
    {
        s.playlist_index
    } else {
        first_playlist_index(playlist)?
    };
    Some(RotationAction::ShowPlaylistItem {
        playlist_id: playlist_id.clone(),
        index,
        item: playlist.items[index].clone(),
    })
}

/// Next action for a rotating monitor; advances its playlist position.
fn rotation_action(
    s: &mut MonitorSchedule,
    m: &MonitorTick,
    launch: bool,
) -> Option<RotationAction> {
    match &m.source {
        WallpaperSource::PhotoApi => Some(RotationAction::NextPhoto),
        WallpaperSource::Workshop { .. } => None,
        WallpaperSource::Playlist { playlist_id } => {
            let playlist = m.playlist.as_ref()?;
            let index = next_playlist_index(playlist, s.playlist_index);
            let item = playlist.items.get(index)?;
            if !playlist_item_allowed(playlist.mode, item) {
                return None;
            }
            let unchanged = index == s.playlist_index;
            s.playlist_index = index;
            // A single Workshop item has nothing new to show (launch still shows it).
            if unchanged && !launch && matches!(item, PlaylistItem::Workshop { .. }) {
                return None;
            }
            Some(RotationAction::ShowPlaylistItem {
                playlist_id: playlist_id.clone(),
                index,
                item: item.clone(),
            })
        }
    }
}

static REPLAN: AtomicBool = AtomicBool::new(true);
static PLAYLIST_CACHE: Mutex<Option<Arc<Vec<Playlist>>>> = parking_lot::const_mutex(None);

/// Asks the runtime loop for a full tick on its next second.
pub fn request_replan() {
    REPLAN.store(true, Ordering::Relaxed);
}

/// Drops the cached playlists; call after any write to the `playlists` table.
pub fn invalidate_playlist_cache() {
    *PLAYLIST_CACHE.lock() = None;
    request_replan();
}

/// Playlists parsed once and kept until [`invalidate_playlist_cache`].
fn cached_playlists(conn: &rusqlite::Connection) -> Result<Arc<Vec<Playlist>>, String> {
    if let Some(cached) = PLAYLIST_CACHE.lock().as_ref() {
        return Ok(cached.clone());
    }
    let fresh = Arc::new(crate::db::list_playlists(conn)?);
    *PLAYLIST_CACHE.lock() = Some(fresh.clone());
    Ok(fresh)
}

static MANUAL_NOTES: parking_lot::Mutex<Vec<ManualNote>> = parking_lot::const_mutex(Vec::new());

/// Called by IPC commands after a user-initiated rotation / apply so the scheduler restarts the
/// monitor's timer instead of rotating again right away.
pub fn note_manual_rotation(monitor_id: Option<&str>, restart_playlist: bool) {
    let mut notes = MANUAL_NOTES.lock();
    if notes.len() >= MAX_PENDING_MANUAL_NOTES {
        notes.remove(0);
    }
    notes.push(ManualNote {
        monitor_id: monitor_id.map(str::to_string),
        restart_playlist,
    });
    request_replan();
}

static FORCED_NEXT: parking_lot::Mutex<Vec<String>> = parking_lot::const_mutex(Vec::new());

/// Advances the playlist of `monitor_id` on the next scheduler tick (global hotkey / tray Next).
pub fn request_next(monitor_id: &str) {
    let mut ids = FORCED_NEXT.lock();
    if ids.len() >= MAX_PENDING_MANUAL_NOTES {
        ids.remove(0);
    }
    ids.push(monitor_id.to_string());
    request_replan();
}

fn take_forced_next() -> Vec<String> {
    std::mem::take(&mut *FORCED_NEXT.lock())
}

fn take_manual_notes() -> Vec<ManualNote> {
    std::mem::take(&mut *MANUAL_NOTES.lock())
}

static DISCONNECTED: parking_lot::Mutex<Vec<String>> = parking_lot::const_mutex(Vec::new());

/// Called when a monitor's wallpaper window was closed because the monitor disconnected, so the
/// scheduler restores it on reconnect (see [`RotationScheduler::mark_disconnected`]).
pub fn note_monitor_disconnected(monitor_id: &str) {
    let mut ids = DISCONNECTED.lock();
    if ids.len() >= MAX_PENDING_MANUAL_NOTES {
        ids.remove(0);
    }
    ids.push(monitor_id.to_string());
    request_replan();
}

fn take_disconnected() -> Vec<String> {
    std::mem::take(&mut *DISCONNECTED.lock())
}

/// Whether the source persisted for a monitor right now (`None` = unassigned = Photo API) is
/// still the one a rotation was planned for.
pub fn planned_source_is_current(
    persisted: Option<&WallpaperSource>,
    planned: &WallpaperSource,
) -> bool {
    persisted.cloned().unwrap_or_default() == *planned
}

/// Starts the scheduler task (called once from `setup_app`).
///
/// The loop wakes every [`TICK_INTERVAL`] but only touches the database (a "full tick") when
/// something needs it: the first run, a note / replan request, a wake from sleep, a changed
/// rotation setting or monitor list, the planned next due time, or the idle safety net.
pub fn spawn(app: AppHandle) {
    // Supervisor: a panic inside a tick ends the inner task, not rotation for the session.
    tauri::async_runtime::spawn(async move {
        let mut first = true;
        loop {
            let handle = tauri::async_runtime::spawn(run_loop(app.clone(), first));
            first = false;
            match handle.await {
                Ok(()) => break,
                Err(err) => {
                    tracing::error!("rotation scheduler stopped ({err}); restarting");
                    REPLAN.store(true, Ordering::Relaxed);
                    tokio::time::sleep(RESTART_DELAY).await;
                }
            }
        }
    });
}

async fn run_loop(app: AppHandle, first_start: bool) {
    {
        if first_start {
            tokio::time::sleep(LAUNCH_DELAY).await;
        }
        let mut scheduler = RotationScheduler::new(crate::power::wake_generation());
        if let Some(state) = app.try_state::<AppState>() {
            match crate::db::load_playlist_positions(&state.db.lock()) {
                Ok(p) => scheduler.restore_positions(p),
                Err(err) => tracing::warn!("rotation scheduler: cannot load positions: {err}"),
            }
        }
        let mut live: Vec<String> = Vec::new();
        let mut refresh_in: u32 = 0;
        let mut last_full = Instant::now();
        let mut last_gen = crate::power::wake_generation();
        let mut last_suspended = crate::power::rendering_suspended();
        let mut last_interval: Option<u64> = None;
        let mut interval = tokio::time::interval(TICK_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let mut needed = false;
            if refresh_in == 0 {
                let fresh: Vec<String> = live_monitors(&app).into_iter().map(|m| m.id).collect();
                needed |= fresh != live;
                live = fresh;
                refresh_in = LIVE_MONITOR_REFRESH_TICKS;
            }
            refresh_in -= 1;
            let now = Instant::now();
            let generation = crate::power::wake_generation();
            let suspended = crate::power::rendering_suspended();
            let settings_interval = app
                .try_state::<AppState>()
                .map(|s| s.settings.lock().rotation_interval_seconds);
            needed |= REPLAN.swap(false, Ordering::Relaxed)
                || generation != last_gen
                || suspended != last_suspended
                || settings_interval != last_interval
                || scheduler.next_wake().is_some_and(|due| due <= now)
                || now.duration_since(last_full) >= MAX_IDLE_BETWEEN_FULL_TICKS;
            if !needed {
                continue;
            }
            last_gen = generation;
            last_suspended = suspended;
            last_interval = settings_interval;
            last_full = now;
            run_tick(&app, &mut scheduler, &live).await;
        }
    }
}

async fn run_tick(app: &AppHandle, scheduler: &mut RotationScheduler, live: &[String]) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let settings_interval = state.settings.lock().rotation_interval_seconds;
    let snapshot = {
        let conn = state.db.lock();
        crate::db::monitor_sources(&conn).and_then(|assigned| {
            let playlists = cached_playlists(&conn)?;
            let clone = truongpaper_core::get_setting::<String>(&conn, SETTING_MULTI_MONITOR_MODE)
                .ok()
                .flatten()
                .is_some_and(|m| m == "clone");
            Ok((assigned, playlists, clone))
        })
    };
    let (assigned, playlists, clone_photos) = match snapshot {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!("rotation scheduler: cannot read monitor sources: {err}");
            return;
        }
    };
    let suspended = crate::power::rendering_suspended();
    let monitors = build_monitor_ticks(live, &assigned, &playlists, settings_interval, |id| {
        !suspended && state.host.monitor_should_render(id)
    });
    for note in take_manual_notes() {
        scheduler.note_manual(note);
    }
    for id in take_disconnected() {
        scheduler.mark_disconnected(&id);
    }
    for id in take_forced_next() {
        scheduler.note_force_next(&id);
    }
    let plans = scheduler.plan(
        Instant::now(),
        &monitors,
        crate::power::wake_generation(),
        clone_photos,
    );
    let changes = scheduler.take_position_changes();
    if !changes.is_empty() {
        let conn = state.db.lock();
        for (monitor, playlist, index) in changes {
            if let Err(err) = crate::db::save_playlist_position(&conn, &monitor, &playlist, index) {
                tracing::warn!("rotation scheduler: cannot save position of {monitor}: {err}");
            }
        }
    }
    execute(app, plans, clone_photos).await;
}

/// Runs the planned rotations of different monitors in parallel; each plan has its own
/// timeout so a slow display does not delay the others.
async fn execute(app: &AppHandle, plans: Vec<PlannedRotation>, clone_photos: bool) {
    let mut set = tokio::task::JoinSet::new();
    for plan in plans {
        let app = app.clone();
        set.spawn(async move { run_plan_with_timeout(&app, plan, clone_photos).await });
    }
    let mut cloned_photo_draw = false;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(draw) => cloned_photo_draw |= draw,
            Err(err) => tracing::warn!("rotation scheduler: a rotation task failed: {err}"),
        }
    }
    if cloned_photo_draw {
        let Some(photo) = app.try_state::<PhotoServiceState>() else {
            return;
        };
        match tokio::time::timeout(PLAN_TIMEOUT, rotate_photo_next(app, &photo, None)).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                tracing::warn!("rotation scheduler: cloned photo rotation failed: {err}")
            }
            Err(_) => tracing::warn!("rotation scheduler: cloned photo rotation timed out"),
        }
    }
}

/// Returns true when the plan asks for the shared cloned-photo draw.
async fn run_plan_with_timeout(app: &AppHandle, plan: PlannedRotation, clone_photos: bool) -> bool {
    let id = plan.monitor_id.clone();
    with_plan_timeout(&id, PLAN_TIMEOUT, execute_plan(app, plan, clone_photos)).await
}

/// Bounds one plan; a timed-out plan counts as "no shared photo draw requested".
async fn with_plan_timeout(
    id: &str,
    limit: Duration,
    plan: impl std::future::Future<Output = bool>,
) -> bool {
    match tokio::time::timeout(limit, plan).await {
        Ok(draw) => draw,
        Err(_) => {
            tracing::warn!("rotation scheduler: rotation of {id} timed out after {limit:?}");
            false
        }
    }
}

async fn execute_plan(app: &AppHandle, p: PlannedRotation, clone_photos: bool) -> bool {
    let (Some(state), Some(photo)) = (
        app.try_state::<AppState>(),
        app.try_state::<PhotoServiceState>(),
    ) else {
        return false;
    };
    let (state, photo) = (&*state, &*photo);
    tracing::debug!(
        "rotation scheduler: {} {:?} -> {:?}",
        p.monitor_id,
        p.trigger,
        p.action
    );
    let id = p.monitor_id.as_str();
    // Same lock as `monitor_apply_wallpaper`; once held, a rotation planned from an older
    // snapshot is dropped if the user changed the monitor's source in the meantime.
    let _apply = lock_monitor_apply(id).await;
    let current = {
        let conn = state.db.lock();
        crate::db::monitor_sources(&conn).map(|mut s| s.remove(id))
    };
    match current {
        Ok(current) if planned_source_is_current(current.as_ref(), &p.source) => {}
        Ok(_) => {
            tracing::debug!("rotation scheduler: {id} changed source; rotation dropped");
            return false;
        }
        Err(err) => {
            tracing::warn!("rotation scheduler: cannot re-read the source of {id}: {err}");
            return false;
        }
    }
    let mut cloned_photo_draw = false;
    let res = match &p.action {
        RotationAction::Restore => match &p.source {
            // Unassigned monitors stay unassigned (no DB write).
            WallpaperSource::PhotoApi => show_photo(app, state, photo, id, false).await,
            other => apply_source(app, state, photo, id, other).await,
        },
        RotationAction::NextPhoto if clone_photos && p.source == WallpaperSource::PhotoApi => {
            cloned_photo_draw = true;
            Ok(())
        }
        RotationAction::NextPhoto => show_photo(app, state, photo, id, true).await,
        RotationAction::ShowPlaylistItem {
            playlist_id, item, ..
        } => show_playlist_item(app, state, photo, id, playlist_id, item, true).await,
    };
    if let Err(err) = res {
        tracing::warn!(
            "rotation scheduler: {:?} rotation of {id} failed: {err}",
            p.trigger
        );
    }
    cloned_photo_draw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_stuck_plan_times_out_without_blocking_the_others() {
        let stuck = with_plan_timeout("a", Duration::from_millis(20), async {
            std::future::pending::<bool>().await
        });
        let quick = with_plan_timeout("b", Duration::from_secs(5), async { true });
        let (stuck, quick) = tokio::join!(stuck, quick);
        assert!(!stuck);
        assert!(quick);
    }

    fn ws(id: &str) -> PlaylistItem {
        PlaylistItem::Workshop {
            wallpaper_id: id.into(),
        }
    }

    fn playlist(mode: PlaylistMode, items: Vec<PlaylistItem>) -> Playlist {
        Playlist {
            playlist_id: "p1".into(),
            name: "P".into(),
            mode,
            interval_seconds: 60,
            items,
            transition: None,
        }
    }

    fn photo_tick(id: &str, interval: u64) -> MonitorTick {
        MonitorTick {
            monitor_id: id.into(),
            source: WallpaperSource::PhotoApi,
            interval_secs: interval,
            renderable: true,
            playlist: None,
        }
    }

    fn playlist_tick(id: &str, p: &Playlist) -> MonitorTick {
        MonitorTick {
            monitor_id: id.into(),
            source: WallpaperSource::Playlist {
                playlist_id: p.playlist_id.clone(),
            },
            interval_secs: p.interval_seconds,
            renderable: true,
            playlist: Some(p.clone()),
        }
    }

    fn secs(t0: Instant, s: u64) -> Instant {
        t0 + Duration::from_secs(s)
    }

    fn summary(plans: &[PlannedRotation]) -> Vec<(String, RotationTrigger, RotationAction)> {
        plans
            .iter()
            .map(|p| (p.monitor_id.clone(), p.trigger, p.action.clone()))
            .collect()
    }

    #[test]
    fn next_due_treats_zero_as_never() {
        let t0 = Instant::now();
        assert_eq!(next_due(t0, t0, 0), None);
        assert!(!is_due(secs(t0, 1_000_000), t0, 0));
        assert_eq!(next_due(t0, t0, 60), Some(secs(t0, 60)));
        assert!(!is_due(secs(t0, 59), t0, 60));
        assert!(is_due(secs(t0, 60), t0, 60));
        // Overdue (e.g. after sleep) is due "now", not in the past.
        assert_eq!(next_due(secs(t0, 500), t0, 60), Some(secs(t0, 500)));
        assert_eq!(next_due(t0, t0, u64::MAX), None, "overflow never fires");
    }

    #[test]
    fn playlist_cycling_per_mode() {
        let items = vec![
            PlaylistItem::PhotoApiDraw,
            ws("a"),
            PlaylistItem::PhotoApiDraw,
            ws("b"),
        ];
        let mixed = playlist(PlaylistMode::Mixed, items.clone());
        assert_eq!(next_playlist_index(&mixed, 0), 1);
        assert_eq!(next_playlist_index(&mixed, 3), 0);
        assert_eq!(next_playlist_index(&mixed, 9), 2, "stale index wraps");

        let photos = playlist(PlaylistMode::PhotosOnly, items.clone());
        assert_eq!(next_playlist_index(&photos, 0), 2);
        assert_eq!(next_playlist_index(&photos, 2), 0);

        let workshop = playlist(PlaylistMode::WorkshopOnly, items);
        assert_eq!(next_playlist_index(&workshop, 0), 1);
        assert_eq!(next_playlist_index(&workshop, 1), 3);
        assert_eq!(next_playlist_index(&workshop, 3), 1);
        assert_eq!(first_playlist_index(&workshop), Some(1));
        assert_eq!(first_playlist_index(&photos), Some(0));
    }

    #[test]
    fn playlist_cycling_empty_single_and_nothing_allowed() {
        let empty = playlist(PlaylistMode::Mixed, vec![]);
        assert_eq!(next_playlist_index(&empty, 0), 0);
        assert_eq!(next_playlist_index(&empty, 5), 0);
        assert_eq!(first_playlist_index(&empty), None);

        let single = playlist(PlaylistMode::Mixed, vec![ws("a")]);
        assert_eq!(next_playlist_index(&single, 0), 0);

        let none_allowed = playlist(PlaylistMode::PhotosOnly, vec![ws("a"), ws("b")]);
        assert_eq!(next_playlist_index(&none_allowed, 1), 1);
        assert!(!playlist_item_allowed(
            none_allowed.mode,
            &none_allowed.items[1]
        ));
        assert_eq!(first_playlist_index(&none_allowed), None);
    }

    #[test]
    fn per_monitor_interval_selection() {
        let p = playlist(PlaylistMode::Mixed, vec![ws("a")]);
        assert_eq!(
            monitor_interval_secs(&WallpaperSource::PhotoApi, 300, None),
            300
        );
        assert_eq!(
            monitor_interval_secs(&WallpaperSource::PhotoApi, 0, None),
            0
        );
        let pl = WallpaperSource::Playlist {
            playlist_id: "p1".into(),
        };
        assert_eq!(monitor_interval_secs(&pl, 300, Some(&p)), 60);
        assert_eq!(monitor_interval_secs(&pl, 300, None), 0, "deleted playlist");
        let w = WallpaperSource::Workshop {
            wallpaper_id: "w".into(),
        };
        assert_eq!(monitor_interval_secs(&w, 300, None), 0);

        let mut assigned = HashMap::new();
        assigned.insert("B".to_string(), pl.clone());
        assigned.insert("C".to_string(), w);
        let ticks = build_monitor_ticks(
            &["A".to_string(), "B".to_string()],
            &assigned,
            std::slice::from_ref(&p),
            300,
            |id| id != "B",
        );
        let view: Vec<(&str, u64, bool, bool)> = ticks
            .iter()
            .map(|t| {
                (
                    t.monitor_id.as_str(),
                    t.interval_secs,
                    t.renderable,
                    t.playlist.is_some(),
                )
            })
            .collect();
        assert_eq!(
            view,
            [("A", 300, true, false), ("B", 60, false, true)],
            "C is assigned but not connected"
        );
        assert_eq!(
            ticks[0].source,
            WallpaperSource::PhotoApi,
            "unassigned = photo"
        );
    }

    #[test]
    fn disconnected_monitors_are_skipped_and_restored_on_reconnect() {
        let t0 = Instant::now();
        let mut assigned = HashMap::new();
        assigned.insert(
            "EXT".to_string(),
            WallpaperSource::Workshop {
                wallpaper_id: "web".into(),
            },
        );
        assigned.insert("LAPTOP".to_string(), WallpaperSource::PhotoApi);
        let ticks = |live: &[&str]| {
            let live: Vec<String> = live.iter().map(|s| s.to_string()).collect();
            build_monitor_ticks(&live, &assigned, &[], 60, |_| true)
        };
        let mut s = RotationScheduler::new(0);

        // The monitor list is not known on the first tick: launch rotation is kept.
        assert!(s.plan(t0, &ticks(&[]), 0, false).is_empty());
        // Launch with the external display unplugged: only the laptop rotates.
        let plans = s.plan(secs(t0, 1), &ticks(&["LAPTOP"]), 0, false);
        assert_eq!(
            summary(&plans),
            vec![(
                "LAPTOP".into(),
                RotationTrigger::AppLaunch,
                RotationAction::NextPhoto
            )]
        );
        // Plugging it in restores its Web wallpaper.
        let plans = s.plan(secs(t0, 2), &ticks(&["EXT", "LAPTOP"]), 0, false);
        assert_eq!(
            summary(&plans),
            vec![(
                "EXT".into(),
                RotationTrigger::AppLaunch,
                RotationAction::Restore
            )]
        );
        // Unplugged again: nothing is planned for it, and the photo monitor keeps its timer.
        assert!(s
            .plan(secs(t0, 30), &ticks(&["LAPTOP"]), 0, false)
            .is_empty());
        let plans = s.plan(secs(t0, 61), &ticks(&["LAPTOP"]), 0, false);
        assert!(plans.iter().all(|p| p.monitor_id == "LAPTOP"));

        // Quick replug between two monitor refreshes: the bridge closed the window and asked
        // the scheduler to forget the monitor, so it is restored even though it never left
        // the scheduler's live list.
        s.plan(secs(t0, 62), &ticks(&["EXT", "LAPTOP"]), 0, false);
        assert!(s
            .plan(secs(t0, 63), &ticks(&["EXT", "LAPTOP"]), 0, false)
            .is_empty());
        s.mark_disconnected("EXT");
        let plans = s.plan(secs(t0, 64), &ticks(&["EXT", "LAPTOP"]), 0, false);
        assert_eq!(
            summary(&plans),
            vec![(
                "EXT".into(),
                RotationTrigger::AppLaunch,
                RotationAction::Restore
            )]
        );
        note_monitor_disconnected("EXT");
        assert!(take_disconnected().contains(&"EXT".to_string()));
    }

    #[test]
    fn reconnect_resumes_playlist_position_instead_of_starting_over() {
        let t0 = Instant::now();
        let p = playlist(PlaylistMode::Mixed, vec![ws("a"), ws("b"), ws("c")]);
        let ticks = [playlist_tick("EXT", &p)];
        let mut scheduler = RotationScheduler::new(0);
        scheduler.plan(t0, &ticks, 0, false);
        scheduler.plan(secs(t0, 60), &ticks, 0, false);
        assert_eq!(scheduler.playlist_index("EXT"), Some(2));
        scheduler.plan(secs(t0, 61), &[], 0, false);
        let plans = scheduler.plan(secs(t0, 120), &ticks, 0, false);
        assert!(
            matches!(&plans[0].action, RotationAction::ShowPlaylistItem { index: 2, item, .. } if item == &ws("c"))
        );
        assert!(scheduler.plan(secs(t0, 121), &ticks, 0, false).is_empty());
        scheduler.mark_disconnected("EXT");
        assert!(matches!(
            scheduler.plan(secs(t0, 122), &ticks, 0, false)[0].action,
            RotationAction::ShowPlaylistItem { index: 2, .. }
        ));
    }

    #[test]
    fn launch_rotates_rotating_monitors_and_restores_the_rest() {
        let t0 = Instant::now();
        let p = playlist(
            PlaylistMode::Mixed,
            vec![ws("a"), PlaylistItem::PhotoApiDraw],
        );
        let mut paused = photo_tick("D", 60);
        paused.renderable = false;
        let monitors = vec![
            photo_tick("A", 60),
            playlist_tick("B", &p),
            MonitorTick {
                monitor_id: "C".into(),
                source: WallpaperSource::Workshop {
                    wallpaper_id: "w".into(),
                },
                interval_secs: 0,
                renderable: true,
                playlist: None,
            },
            paused,
            photo_tick("E", 0),
        ];
        let mut s = RotationScheduler::new(0);
        let plans = s.plan(t0, &monitors, 0, false);
        assert!(plans
            .iter()
            .all(|p| p.trigger == RotationTrigger::AppLaunch));
        assert_eq!(
            plans.iter().map(|p| p.action.clone()).collect::<Vec<_>>(),
            vec![
                RotationAction::NextPhoto,
                RotationAction::ShowPlaylistItem {
                    playlist_id: "p1".into(),
                    index: 1,
                    item: PlaylistItem::PhotoApiDraw
                },
                RotationAction::Restore,
                RotationAction::Restore,
                RotationAction::Restore,
            ]
        );
        assert_eq!(s.playlist_index("B"), Some(1));
        // Nothing else happens until an interval elapses.
        assert!(s.plan(secs(t0, 1), &monitors, 0, false).is_empty());
    }

    #[test]
    fn timer_rotation_skips_paused_monitors_until_visible() {
        let t0 = Instant::now();
        let p = playlist(
            PlaylistMode::WorkshopOnly,
            vec![ws("a"), PlaylistItem::PhotoApiDraw, ws("b")],
        );
        let mut monitors = vec![photo_tick("A", 30), playlist_tick("B", &p)];
        let mut s = RotationScheduler::new(0);
        s.plan(t0, &monitors, 0, false);
        assert_eq!(s.playlist_index("B"), Some(2), "launch advanced 0 -> 2");

        assert!(s.plan(secs(t0, 29), &monitors, 0, false).is_empty());
        let plans = s.plan(secs(t0, 30), &monitors, 0, false);
        assert_eq!(
            summary(&plans),
            vec![(
                "A".into(),
                RotationTrigger::Timer,
                RotationAction::NextPhoto
            )]
        );

        // B's 60 s interval elapses while it is paused: no rotation...
        monitors[1].renderable = false;
        assert!(s
            .plan(secs(t0, 61), &monitors, 0, false)
            .iter()
            .all(|p| p.monitor_id != "B"));
        // ...until it renders again; WorkshopOnly skips the photo draw.
        monitors[1].renderable = true;
        let plans = s.plan(secs(t0, 62), &monitors, 0, false);
        assert_eq!(
            summary(&plans),
            vec![(
                "B".into(),
                RotationTrigger::Timer,
                RotationAction::ShowPlaylistItem {
                    playlist_id: "p1".into(),
                    index: 0,
                    item: ws("a")
                }
            )]
        );
    }

    #[test]
    fn wake_rotates_once_when_visible_and_never_with_interval_zero() {
        let t0 = Instant::now();
        let mut monitors = vec![photo_tick("A", 3600), photo_tick("B", 0)];
        let mut s = RotationScheduler::new(7);
        s.plan(t0, &monitors, 7, false);

        // Wake is seen while A is still reported paused (Smart Pause lags 250 ms).
        monitors[0].renderable = false;
        assert!(s.plan(secs(t0, 5), &monitors, 8, false).is_empty());
        monitors[0].renderable = true;
        let plans = s.plan(secs(t0, 6), &monitors, 8, false);
        assert_eq!(
            summary(&plans),
            vec![(
                "A".into(),
                RotationTrigger::DisplayWake,
                RotationAction::NextPhoto
            )]
        );
        assert!(s.plan(secs(t0, 7), &monitors, 8, false).is_empty());
    }

    #[test]
    fn manual_rotation_and_source_changes_restart_timers() {
        let t0 = Instant::now();
        let p = playlist(PlaylistMode::Mixed, vec![ws("a"), ws("b"), ws("c")]);
        let mut monitors = vec![photo_tick("A", 60), playlist_tick("B", &p)];
        let mut s = RotationScheduler::new(0);
        s.plan(t0, &monitors, 0, false);
        assert_eq!(s.playlist_index("B"), Some(1));

        // photo_rotate_next without monitorId at t0+50 resets only photo monitors.
        s.note_manual(ManualNote {
            monitor_id: None,
            restart_playlist: false,
        });
        assert!(s.plan(secs(t0, 50), &monitors, 0, false).is_empty());
        let plans = s.plan(secs(t0, 60), &monitors, 0, false);
        assert_eq!(
            summary(&plans),
            vec![(
                "B".into(),
                RotationTrigger::Timer,
                RotationAction::ShowPlaylistItem {
                    playlist_id: "p1".into(),
                    index: 2,
                    item: ws("c")
                }
            )]
        );
        assert!(s.plan(secs(t0, 109), &monitors, 0, false).is_empty());

        // Re-applying the same playlist restarts its position.
        s.note_manual(ManualNote {
            monitor_id: Some("B".into()),
            restart_playlist: true,
        });
        s.plan(secs(t0, 110), &monitors, 0, false);
        assert_eq!(s.playlist_index("B"), Some(0));

        // Switching A to a single Workshop wallpaper: shown by the command, never rotated.
        monitors[0] = MonitorTick {
            monitor_id: "A".into(),
            source: WallpaperSource::Workshop {
                wallpaper_id: "w".into(),
            },
            interval_secs: 0,
            renderable: true,
            playlist: None,
        };
        assert!(s.plan(secs(t0, 111), &monitors, 0, false).is_empty());
        assert!(s
            .plan(secs(t0, 100_000), &monitors, 1, false)
            .iter()
            .all(|p| p.monitor_id != "A"));
    }

    #[test]
    fn single_item_and_hotplug_behaviour() {
        let t0 = Instant::now();
        let single = playlist(PlaylistMode::Mixed, vec![ws("only")]);
        let photo_single = Playlist {
            playlist_id: "p2".into(),
            ..playlist(PlaylistMode::Mixed, vec![PlaylistItem::PhotoApiDraw])
        };
        let mut monitors = vec![
            playlist_tick("A", &single),
            playlist_tick("B", &photo_single),
        ];
        let mut s = RotationScheduler::new(0);
        let launch = s.plan(t0, &monitors, 0, false);
        assert_eq!(launch.len(), 2, "launch always shows something");
        // Timer: the single Workshop item has nothing new; a single photo draw draws again.
        let plans = s.plan(secs(t0, 60), &monitors, 0, false);
        assert_eq!(
            summary(&plans),
            vec![(
                "B".into(),
                RotationTrigger::Timer,
                RotationAction::ShowPlaylistItem {
                    playlist_id: "p2".into(),
                    index: 0,
                    item: PlaylistItem::PhotoApiDraw
                }
            )]
        );

        // A monitor plugged in later is restored, not rotated.
        monitors.push(photo_tick("C", 60));
        let plans = s.plan(secs(t0, 61), &monitors, 0, false);
        assert_eq!(
            summary(&plans),
            vec![(
                "C".into(),
                RotationTrigger::AppLaunch,
                RotationAction::Restore
            )]
        );
    }

    #[test]
    fn clone_mode_restarts_every_photo_timer() {
        let t0 = Instant::now();
        let monitors = vec![photo_tick("A", 60), photo_tick("B", 60)];
        let mut s = RotationScheduler::new(0);
        s.plan(t0, &monitors, 0, true);
        s.note_manual(ManualNote {
            monitor_id: Some("B".into()),
            restart_playlist: false,
        });
        s.plan(secs(t0, 30), &monitors, 0, true);
        // A is due at 60; B (manually rotated at 30) is mirrored and restarts too.
        let plans = s.plan(secs(t0, 60), &monitors, 0, true);
        assert_eq!(
            summary(&plans),
            vec![(
                "A".into(),
                RotationTrigger::Timer,
                RotationAction::NextPhoto
            )]
        );
        assert!(s.plan(secs(t0, 90), &monitors, 0, true).is_empty());
        assert_eq!(s.plan(secs(t0, 120), &monitors, 0, true).len(), 2);
    }

    #[test]
    fn planned_rotations_need_the_same_persisted_source() {
        let pl = WallpaperSource::Playlist {
            playlist_id: "p1".into(),
        };
        let ws = WallpaperSource::Workshop {
            wallpaper_id: "w".into(),
        };
        assert!(planned_source_is_current(Some(&pl), &pl));
        assert!(planned_source_is_current(None, &WallpaperSource::PhotoApi));
        // The user applied a Workshop wallpaper while a playlist rotation was in flight.
        assert!(!planned_source_is_current(Some(&ws), &pl));
        assert!(!planned_source_is_current(None, &pl));
    }

    #[tokio::test]
    async fn multi_monitor_applies_lock_unique_ids_in_stable_order() {
        use crate::commands::monitor::lock_monitor_applies;
        let guards = lock_monitor_applies(["CLONE-B", "CLONE-A", "CLONE-B"]).await;
        assert_eq!(guards.len(), 2);
        let pending = tokio::spawn(async { lock_monitor_applies(["CLONE-A", "CLONE-B"]).await });
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        drop(guards);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), pending)
                .await
                .unwrap()
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn monitor_applies_are_serialized_per_monitor() {
        let a = lock_monitor_apply("SERIAL-A").await;
        // Another monitor is independent.
        let b =
            tokio::time::timeout(Duration::from_millis(200), lock_monitor_apply("SERIAL-B")).await;
        assert!(b.is_ok());
        // The same monitor waits until the first apply finished.
        let blocked =
            tokio::time::timeout(Duration::from_millis(50), lock_monitor_apply("SERIAL-A")).await;
        assert!(blocked.is_err());
        drop(a);
        let again =
            tokio::time::timeout(Duration::from_millis(200), lock_monitor_apply("SERIAL-A")).await;
        assert!(again.is_ok());
    }

    #[test]
    fn manual_notes_are_bounded() {
        for _ in 0..(MAX_PENDING_MANUAL_NOTES + 10) {
            note_manual_rotation(Some("X"), false);
        }
        assert!(take_manual_notes().len() <= MAX_PENDING_MANUAL_NOTES);
    }

    #[test]
    fn requested_next_advances_a_playlist_even_when_paused_or_manual_only() {
        let p = playlist(PlaylistMode::Mixed, vec![ws("a"), ws("b"), ws("c")]);
        let mut tick = playlist_tick("A", &p);
        let t0 = Instant::now();
        let mut s = RotationScheduler::new(0);
        // The launch tick rotates to item 1.
        s.plan(t0, std::slice::from_ref(&tick), 0, false);
        tick.interval_secs = 0;
        tick.renderable = false;
        assert!(s
            .plan(secs(t0, 5), std::slice::from_ref(&tick), 0, false)
            .is_empty());
        s.note_force_next("A");
        let plans = s.plan(secs(t0, 6), std::slice::from_ref(&tick), 0, false);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].trigger, RotationTrigger::Manual);
        assert!(matches!(
            plans[0].action,
            RotationAction::ShowPlaylistItem { index: 2, .. }
        ));
        assert!(s
            .plan(secs(t0, 7), std::slice::from_ref(&tick), 0, false)
            .is_empty());
    }

    #[test]
    fn forced_next_requests_are_bounded() {
        for _ in 0..(MAX_PENDING_MANUAL_NOTES + 10) {
            request_next("X");
        }
        assert!(take_forced_next().len() <= MAX_PENDING_MANUAL_NOTES);
    }

    #[test]
    fn displays_on_one_playlist_rotate_independently() {
        let p = playlist(PlaylistMode::Mixed, vec![ws("a"), ws("b"), ws("c")]);
        let t0 = Instant::now();
        let mut sch = RotationScheduler::new(0);
        let ticks = [playlist_tick("A", &p), playlist_tick("B", &p)];
        sch.plan(t0, &ticks, 0, false); // launch rotation moves both to item 1
        assert_eq!(sch.playlist_index("A"), Some(1));
        assert_eq!(sch.playlist_index("B"), Some(1));
        // Only A is advanced by the user.
        sch.note_force_next("A");
        sch.plan(secs(t0, 5), &ticks, 0, false);
        assert_eq!(sch.playlist_index("A"), Some(2));
        assert_eq!(sch.playlist_index("B"), Some(1));
        // B's own timer (from t0) fires at 60s; A's was restarted at 5s so it is not due yet.
        let plans = sch.plan(secs(t0, 61), &ticks, 0, false);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].monitor_id, "B");
        assert_eq!(sch.playlist_index("A"), Some(2));
        assert_eq!(sch.playlist_index("B"), Some(2));
        assert_eq!(sch.next_wake(), Some(secs(t0, 65)));
    }

    #[test]
    fn positions_are_reported_and_restored_after_restart() {
        let p = playlist(PlaylistMode::Mixed, vec![ws("a"), ws("b"), ws("c")]);
        let t0 = Instant::now();
        let ticks = [playlist_tick("A", &p)];
        let mut sch = RotationScheduler::new(0);
        sch.plan(t0, &ticks, 0, false);
        sch.note_force_next("A");
        sch.plan(secs(t0, 1), &ticks, 0, false);
        let changes = sch.take_position_changes();
        assert_eq!(changes.last().unwrap(), &("A".into(), "p1".into(), 2));
        assert!(sch.take_position_changes().is_empty());

        // New process: the saved index seeds the monitor and launch continues from it.
        let mut again = RotationScheduler::new(0);
        again.restore_positions(HashMap::from([("A".to_string(), ("p1".to_string(), 1))]));
        let plans = again.plan(t0, &ticks, 0, false);
        assert_eq!(plans.len(), 1);
        assert!(matches!(
            plans[0].action,
            RotationAction::ShowPlaylistItem { index: 2, .. }
        ));
        // A position saved for another playlist is ignored.
        let mut other = RotationScheduler::new(0);
        other.restore_positions(HashMap::from([("A".to_string(), ("zzz".to_string(), 2))]));
        other.plan(t0, &ticks, 0, false);
        assert_eq!(other.playlist_index("A"), Some(1));
    }

    #[test]
    fn next_wake_is_the_earliest_due_time() {
        let t0 = Instant::now();
        let mut sch = RotationScheduler::new(0);
        sch.plan(t0, &[photo_tick("A", 30), photo_tick("B", 90)], 0, false);
        assert_eq!(sch.next_wake(), Some(secs(t0, 30)));
        let mut never = RotationScheduler::new(0);
        never.plan(t0, &[photo_tick("A", 0)], 0, false);
        assert_eq!(never.next_wake(), None);
    }
}
