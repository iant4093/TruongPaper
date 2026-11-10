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
//! visible again. Playlist positions live in memory only (docs do not specify persistence).

use crate::commands::monitor::{
    apply_source, first_playlist_index, live_monitors, lock_monitor_apply, set_photo_host_source,
    show_photo, show_playlist_item,
};
use crate::commands::photo::{rotate_photo_next, PhotoServiceState};
use crate::models::{Playlist, PlaylistItem, PlaylistMode, WallpaperSource};
use crate::state::{AppState, SETTING_MULTI_MONITOR_MODE};
use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// Scheduler poll period.
pub const TICK_INTERVAL: Duration = Duration::from_secs(1);
/// Delay before the first tick ("Rotate on App Launch") so the event loop and windows are up.
pub const LAUNCH_DELAY: Duration = Duration::from_secs(2);
/// The live monitor list needs a main-thread round trip; refresh it every N ticks.
const LIVE_MONITOR_REFRESH_TICKS: u32 = 5;
/// Bound for queued manual notes when no scheduler is draining them (tests, early startup).
const MAX_PENDING_MANUAL_NOTES: usize = 256;

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
/// a display nobody sees. They are restored (as new monitors) once they are connected again.
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
}

impl RotationScheduler {
    /// `wake_generation` is the current `power::wake_generation()`.
    pub fn new(wake_generation: u64) -> Self {
        Self {
            monitors: HashMap::new(),
            launched: false,
            wake_generation,
            manual: Vec::new(),
        }
    }

    /// Records a manual rotation; applied by the next [`Self::plan`].
    pub fn note_manual(&mut self, note: ManualNote) {
        self.manual.push(note);
    }

    /// Forgets a monitor whose wallpaper window was closed because it disconnected: the next
    /// [`Self::plan`] that sees it treats it as new and restores its source.
    pub fn forget(&mut self, monitor_id: &str) {
        self.monitors.remove(monitor_id);
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
        self.monitors
            .retain(|id, _| monitors.iter().any(|m| &m.monitor_id == id));

        let mut out = Vec::new();
        for m in monitors {
            let first_index = m
                .playlist
                .as_ref()
                .and_then(first_playlist_index)
                .unwrap_or(0);
            let is_new = !self.monitors.contains_key(&m.monitor_id);
            let s = self
                .monitors
                .entry(m.monitor_id.clone())
                .or_insert_with(|| MonitorSchedule {
                    source: m.source.clone(),
                    last_rotation: now,
                    playlist_index: first_index,
                    wake_pending: false,
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
                continue;
            }
            if !notes.is_empty() {
                s.last_rotation = now;
                s.wake_pending = false;
                if notes.iter().any(|n| n.restart_playlist) {
                    s.playlist_index = first_index;
                }
                continue;
            }
            if woke && m.interval_secs > 0 {
                s.wake_pending = true;
            }

            if launch || is_new {
                let rotate = launch && m.interval_secs > 0 && m.renderable;
                let action = rotate
                    .then(|| rotation_action(s, m, true))
                    .flatten()
                    .unwrap_or(RotationAction::Restore);
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
        out
    }
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
}

fn take_manual_notes() -> Vec<ManualNote> {
    std::mem::take(&mut *MANUAL_NOTES.lock())
}

static DISCONNECTED: parking_lot::Mutex<Vec<String>> = parking_lot::const_mutex(Vec::new());

/// Called when a monitor's wallpaper window was closed because the monitor disconnected, so the
/// scheduler restores it on reconnect (see [`RotationScheduler::forget`]).
pub fn note_monitor_disconnected(monitor_id: &str) {
    let mut ids = DISCONNECTED.lock();
    if ids.len() >= MAX_PENDING_MANUAL_NOTES {
        ids.remove(0);
    }
    ids.push(monitor_id.to_string());
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
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(LAUNCH_DELAY).await;
        let mut scheduler = RotationScheduler::new(crate::power::wake_generation());
        let mut live: Vec<String> = Vec::new();
        let mut refresh_in: u32 = 0;
        let mut interval = tokio::time::interval(TICK_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if refresh_in == 0 {
                live = live_monitors(&app).into_iter().map(|m| m.id).collect();
                refresh_in = LIVE_MONITOR_REFRESH_TICKS;
            }
            refresh_in -= 1;
            run_tick(&app, &mut scheduler, &live).await;
        }
    });
}

async fn run_tick(app: &AppHandle, scheduler: &mut RotationScheduler, live: &[String]) {
    let (Some(state), Some(photo)) = (
        app.try_state::<AppState>(),
        app.try_state::<PhotoServiceState>(),
    ) else {
        return;
    };
    let settings_interval = state.settings.lock().rotation_interval_seconds;
    let snapshot = {
        let conn = state.db.lock();
        crate::db::monitor_sources(&conn).and_then(|assigned| {
            let playlists = crate::db::list_playlists(&conn)?;
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
        scheduler.forget(&id);
    }
    let plans = scheduler.plan(
        Instant::now(),
        &monitors,
        crate::power::wake_generation(),
        clone_photos,
    );
    execute(app, &state, &photo, plans, clone_photos).await;
}

async fn execute(
    app: &AppHandle,
    state: &AppState,
    photo: &PhotoServiceState,
    plans: Vec<PlannedRotation>,
    clone_photos: bool,
) {
    let mut cloned_photo_draw = false;
    for p in plans {
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
                continue;
            }
            Err(err) => {
                tracing::warn!("rotation scheduler: cannot re-read the source of {id}: {err}");
                continue;
            }
        }
        let res = match &p.action {
            RotationAction::Restore => match &p.source {
                // Unassigned monitors stay unassigned (no DB write).
                WallpaperSource::PhotoApi => show_photo(app, state, photo, id, false).await,
                other => apply_source(app, state, photo, id, other).await,
            },
            RotationAction::NextPhoto if clone_photos && p.source == WallpaperSource::PhotoApi => {
                cloned_photo_draw = true;
                set_photo_host_source(state, id)
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
    }
    if cloned_photo_draw {
        if let Err(err) = rotate_photo_next(app, photo, None).await {
            tracing::warn!("rotation scheduler: cloned photo rotation failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        s.forget("EXT");
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
}
