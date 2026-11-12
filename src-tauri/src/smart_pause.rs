//! Smart Pause loop (docs/01 §6): every 250 ms (`POLICY_POLL_INTERVAL`) while the policy is
//! changing, backing off to once a second after a stable stretch, evaluate each monitor's
//! playback policy (fullscreen / maximized occlusion, battery, display sleep) and emit
//! `policy://state-changed` on transitions tracked by `truongpaper_platform::PolicyMonitor`.

use crate::host::WallpaperHost;
use crate::models::{PolicyStatePayload, EVENT_POLICY_STATE_CHANGED};
use parking_lot::Mutex;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use truongpaper_core::AppSettings;
use truongpaper_platform::{
    apply_system_policy, BatteryPolicy, MonitorPlaybackPolicy, OcclusionPrefs, PolicyMonitor,
    SystemPowerState, POLICY_POLL_INTERVAL,
};

/// Monitor list and battery state are refreshed every 2 s instead of every tick: both require
/// OS round-trips (the monitor list comes from the shared cache in `desktop_host`).
const SLOW_REFRESH: std::time::Duration = std::time::Duration::from_secs(2);
/// Ticks without any policy change after which polling backs off.
const STABLE_TICKS_BEFORE_BACKOFF: u32 = 8;
/// Poll interval once the policy has been stable (window list queries are the main cost).
const STABLE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Poll interval for the next tick: fast while things are changing, 1 Hz when stable.
pub fn next_poll_interval(stable_ticks: u32) -> std::time::Duration {
    if stable_ticks >= STABLE_TICKS_BEFORE_BACKOFF {
        STABLE_POLL_INTERVAL
    } else {
        POLICY_POLL_INTERVAL
    }
}

/// One evaluated tick: combines per-monitor occlusion with system state and returns the
/// transitions to publish (pure; unit tested).
pub fn evaluate_tick(
    pm: &mut PolicyMonitor,
    occlusion: &[(String, MonitorPlaybackPolicy)],
    system: SystemPowerState,
    battery: BatteryPolicy,
) -> Vec<PolicyStatePayload> {
    let mut changes = Vec::new();
    for (id, occ) in occlusion {
        let decision = apply_system_policy(*occ, system, battery);
        if let Some(changed) = pm.update(id, decision) {
            changes.push(PolicyStatePayload {
                monitor_id: id.clone(),
                state: changed.as_str(),
            });
        }
    }
    pm.retain_monitors(occlusion.iter().map(|(id, _)| id.as_str()));
    changes
}

fn prefs_from(settings: &AppSettings) -> (OcclusionPrefs, BatteryPolicy) {
    (
        OcclusionPrefs {
            pause_on_fullscreen: settings.pause_on_fullscreen,
            pause_on_maximized: settings.pause_on_maximized,
        },
        BatteryPolicy::from_setting(&settings.battery_policy),
    )
}

#[cfg(target_os = "macos")]
struct OcclusionProbe {
    monitors: Vec<(String, truongpaper_platform::ScreenRect)>,
}

#[cfg(target_os = "macos")]
impl OcclusionProbe {
    fn new() -> Self {
        Self {
            monitors: Vec::new(),
        }
    }

    fn refresh_monitors(&mut self, app: &AppHandle) {
        self.monitors = crate::desktop_host::live_monitors_cached(app)
            .into_iter()
            .map(|m| {
                let (x, y, w, h) = m.logical_rect();
                (m.id, truongpaper_platform::ScreenRect::new(x, y, w, h))
            })
            .collect();
    }

    fn evaluate(&mut self, prefs: OcclusionPrefs) -> Vec<(String, MonitorPlaybackPolicy)> {
        let windows = if prefs.pause_on_fullscreen || prefs.pause_on_maximized {
            truongpaper_platform::cursor::query_normal_window_bounds_macos()
        } else {
            Vec::new()
        };
        self.monitors
            .iter()
            .map(|(id, rect)| {
                (
                    id.clone(),
                    truongpaper_platform::decide_occlusion_by_coverage(*rect, &windows, prefs),
                )
            })
            .collect()
    }
}

#[cfg(target_os = "windows")]
struct OcclusionProbe {
    desktop: Option<truongpaper_platform::win_desktop::DesktopHandles>,
    monitors: Vec<truongpaper_platform::win_desktop::Win32MonitorDesc>,
    setup_attempts: u32,
}

#[cfg(target_os = "windows")]
impl OcclusionProbe {
    fn new() -> Self {
        Self {
            desktop: None,
            monitors: Vec::new(),
            setup_attempts: 0,
        }
    }

    fn refresh_monitors(&mut self, _app: &AppHandle) {
        // SAFETY: plain Win32 enumeration / window lookups on this dedicated thread.
        unsafe {
            if self.desktop.is_none() && self.setup_attempts < 5 {
                self.setup_attempts += 1;
                self.desktop = truongpaper_platform::win_desktop::setup_desktop_layer();
            }
            self.monitors = truongpaper_platform::win_desktop::enumerate_monitors_win32();
        }
    }

    fn evaluate(&mut self, prefs: OcclusionPrefs) -> Vec<(String, MonitorPlaybackPolicy)> {
        self.monitors
            .iter()
            .map(|m| {
                let decision = match &self.desktop {
                    // SAFETY: handles come from setup_desktop_layer on this thread.
                    Some(handles) => unsafe {
                        truongpaper_platform::policy::evaluate_monitor_occlusion_win32(
                            m.monitor_rect,
                            m.work_rect,
                            prefs.pause_on_fullscreen,
                            prefs.pause_on_maximized,
                            handles,
                        )
                    },
                    None => MonitorPlaybackPolicy::Playing,
                };
                (m.device_name.clone(), decision)
            })
            .collect()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
struct OcclusionProbe {
    monitors: Vec<String>,
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl OcclusionProbe {
    fn new() -> Self {
        Self {
            monitors: Vec::new(),
        }
    }

    fn refresh_monitors(&mut self, app: &AppHandle) {
        self.monitors = crate::desktop_host::live_monitors_cached(app)
            .into_iter()
            .map(|m| m.id)
            .collect();
    }

    fn evaluate(&mut self, _prefs: OcclusionPrefs) -> Vec<(String, MonitorPlaybackPolicy)> {
        self.monitors
            .iter()
            .map(|id| (id.clone(), MonitorPlaybackPolicy::Playing))
            .collect()
    }
}

/// Spawns the dedicated Smart Pause thread for the lifetime of the process.
pub fn spawn(app: AppHandle, settings: Arc<Mutex<AppSettings>>, host: Arc<WallpaperHost>) {
    let spawned = std::thread::Builder::new()
        .name("truongpaper-smart-pause".into())
        .spawn(move || {
            let mut pm = PolicyMonitor::new();
            let mut probe = OcclusionProbe::new();
            let mut on_battery = false;
            let mut refresh_at = std::time::Instant::now();
            let mut first = true;
            let mut stable_ticks: u32 = 0;
            loop {
                if first || refresh_at.elapsed() >= SLOW_REFRESH {
                    first = false;
                    probe.refresh_monitors(&app);
                    on_battery = truongpaper_platform::is_on_battery_or_low_power();
                    refresh_at = std::time::Instant::now();
                }

                let (prefs, battery) = prefs_from(&settings.lock());
                let system = SystemPowerState {
                    on_battery_or_low_power: on_battery,
                    display_asleep: crate::power::rendering_suspended(),
                };
                let occlusion = probe.evaluate(prefs);
                let changes = evaluate_tick(&mut pm, &occlusion, system, battery);
                stable_ticks = if changes.is_empty() {
                    stable_ticks.saturating_add(1)
                } else {
                    0
                };
                for change in changes {
                    if let Some(policy) = pm.get(&change.monitor_id) {
                        host.set_policy(&change.monitor_id, policy);
                    }
                    let _ = app.emit(EVENT_POLICY_STATE_CHANGED, change);
                }
                std::thread::sleep(next_poll_interval(stable_ticks));
            }
        });
    if let Err(err) = spawned {
        tracing::error!("failed to start Smart Pause thread: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_emits_only_transitions_and_applies_system_state() {
        let mut pm = PolicyMonitor::new();
        let occ = vec![
            ("A".to_string(), MonitorPlaybackPolicy::Playing),
            ("B".to_string(), MonitorPlaybackPolicy::PausedFullscreen),
        ];
        let idle = SystemPowerState::default();
        let first = evaluate_tick(&mut pm, &occ, idle, BatteryPolicy::PauseAnimations);
        assert_eq!(first.len(), 2);
        assert_eq!(first[1].state, "paused_fullscreen");
        assert!(evaluate_tick(&mut pm, &occ, idle, BatteryPolicy::PauseAnimations).is_empty());

        let battery = SystemPowerState {
            on_battery_or_low_power: true,
            display_asleep: false,
        };
        let changes = evaluate_tick(&mut pm, &occ, battery, BatteryPolicy::PauseAnimations);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].monitor_id, "A");
        assert_eq!(changes[0].state, "paused_battery");

        let asleep = SystemPowerState {
            on_battery_or_low_power: false,
            display_asleep: true,
        };
        let changes = evaluate_tick(&mut pm, &occ[..1], asleep, BatteryPolicy::Continue);
        assert_eq!(changes[0].state, "paused_display_sleep");
        // Unplugged monitor B is forgotten and re-reported when it comes back.
        assert_eq!(pm.get("B"), None);
    }

    #[test]
    fn polling_backs_off_when_stable() {
        assert_eq!(next_poll_interval(0), POLICY_POLL_INTERVAL);
        assert_eq!(next_poll_interval(7), POLICY_POLL_INTERVAL);
        assert_eq!(next_poll_interval(8), STABLE_POLL_INTERVAL);
    }

    #[test]
    fn prefs_follow_settings() {
        let s = AppSettings {
            pause_on_fullscreen: false,
            pause_on_maximized: true,
            battery_policy: "continue".into(),
            ..AppSettings::default()
        };
        let (prefs, battery) = prefs_from(&s);
        assert!(!prefs.pause_on_fullscreen);
        assert!(prefs.pause_on_maximized);
        assert_eq!(battery, BatteryPolicy::Continue);
    }
}
