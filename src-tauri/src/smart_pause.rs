//! Smart Pause loop (docs/01 §6): every 250 ms (`POLICY_POLL_INTERVAL`) evaluate each monitor's
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

/// Monitor list and battery state are refreshed every N ticks (2 s) instead of every 250 ms:
/// both require OS round-trips (main-thread dispatch for the monitor list).
const SLOW_REFRESH_TICKS: u32 = 8;

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
        self.monitors = crate::commands::monitor::live_monitors(app)
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
        self.monitors = crate::commands::monitor::live_monitors(app)
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
            let mut ticks_until_refresh: u32 = 0;
            loop {
                if ticks_until_refresh == 0 {
                    probe.refresh_monitors(&app);
                    on_battery = truongpaper_platform::is_on_battery_or_low_power();
                    ticks_until_refresh = SLOW_REFRESH_TICKS;
                }
                ticks_until_refresh -= 1;

                let (prefs, battery) = prefs_from(&settings.lock());
                let system = SystemPowerState {
                    on_battery_or_low_power: on_battery,
                    display_asleep: crate::power::rendering_suspended(),
                };
                let occlusion = probe.evaluate(prefs);
                for change in evaluate_tick(&mut pm, &occlusion, system, battery) {
                    if let Some(policy) = pm.get(&change.monitor_id) {
                        host.set_policy(&change.monitor_id, policy);
                    }
                    let _ = app.emit(EVENT_POLICY_STATE_CHANGED, change);
                }
                std::thread::sleep(POLICY_POLL_INTERVAL);
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
