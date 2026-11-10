//! OS Sleep, Display Off, and Session Lock observer (`system-power` event).
//! Spec: `docs/01-SYSTEM-ARCHITECTURE.md` §6.2.
//!
//! Besides emitting `system-power`, every event updates process-wide flags that the Smart Pause
//! loop (`smart_pause.rs`) reads as `SystemPowerState::display_asleep`.

use crate::models::EVENT_SYSTEM_POWER;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter};

static APP: OnceLock<AppHandle> = OnceLock::new();

static SYSTEM_ASLEEP: AtomicBool = AtomicBool::new(false);
static DISPLAY_ASLEEP: AtomicBool = AtomicBool::new(false);
static SESSION_LOCKED: AtomicBool = AtomicBool::new(false);
static WAKE_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PowerEvent {
    pub kind: &'static str,   // "pause" | "resume"
    pub reason: &'static str, // "sleep" | "display" | "lock"
}

/// Flag affected by a `reason`, or `None` for unknown reasons.
fn flag_for(reason: &str) -> Option<&'static AtomicBool> {
    match reason {
        "sleep" => Some(&SYSTEM_ASLEEP),
        "display" => Some(&DISPLAY_ASLEEP),
        "lock" => Some(&SESSION_LOCKED),
        _ => None,
    }
}

fn record(kind: &str, reason: &str) {
    if let Some(flag) = flag_for(reason) {
        let was_suspended = rendering_suspended();
        flag.store(kind == "pause", Ordering::Relaxed);
        if kind == "resume" && was_suspended && !rendering_suspended() {
            WAKE_GENERATION.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Bumped every time the desktop becomes visible again (wake from sleep, display wake or
/// unlock, once no other suspend reason remains). The rotation scheduler polls it in-process to
/// implement "Rotate on Display Wake / Unlock".
pub fn wake_generation() -> u64 {
    WAKE_GENERATION.load(Ordering::Relaxed)
}

/// `true` while the system is asleep, the display is off, or the session is locked: nothing on
/// the desktop is visible, so every monitor reports `paused_display_sleep`.
pub fn rendering_suspended() -> bool {
    SYSTEM_ASLEEP.load(Ordering::Relaxed)
        || DISPLAY_ASLEEP.load(Ordering::Relaxed)
        || SESSION_LOCKED.load(Ordering::Relaxed)
}

fn emit(kind: &'static str, reason: &'static str) {
    record(kind, reason);
    if let Some(app) = APP.get() {
        let _ = app.emit(EVENT_SYSTEM_POWER, PowerEvent { kind, reason });
    }
}

pub fn start(app: &AppHandle) {
    if APP.set(app.clone()).is_ok() {
        platform::start();
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::emit;
    use block2::RcBlock;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;

    /// # Safety
    /// `center` must be a valid `NSNotificationCenter` (or subclass) instance.
    unsafe fn observe(
        center: *mut AnyObject,
        name: &str,
        kind: &'static str,
        reason: &'static str,
    ) {
        let name = NSString::from_str(name);
        let block = RcBlock::new(move |_note: *mut AnyObject| emit(kind, reason));
        let queue: *mut AnyObject = msg_send![class!(NSOperationQueue), mainQueue];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let token: *mut AnyObject = msg_send![
            center,
            addObserverForName: &*name,
            object: nil,
            queue: queue,
            usingBlock: &*block
        ];
        // Observers live for the rest of the process.
        let _: *mut AnyObject = msg_send![token, retain];
    }

    pub fn start() {
        // SAFETY: plain AppKit/Foundation class messages with correctly typed arguments; the
        // notification centers copy the blocks.
        unsafe {
            let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            let center: *mut AnyObject = msg_send![workspace, notificationCenter];
            observe(center, "NSWorkspaceWillSleepNotification", "pause", "sleep");
            observe(center, "NSWorkspaceDidWakeNotification", "resume", "sleep");
            observe(
                center,
                "NSWorkspaceScreensDidSleepNotification",
                "pause",
                "display",
            );
            observe(
                center,
                "NSWorkspaceScreensDidWakeNotification",
                "resume",
                "display",
            );

            let distributed: *mut AnyObject =
                msg_send![class!(NSDistributedNotificationCenter), defaultCenter];
            observe(distributed, "com.apple.screenIsLocked", "pause", "lock");
            observe(distributed, "com.apple.screenIsUnlocked", "resume", "lock");
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::emit;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Power::{RegisterPowerSettingNotification, POWERBROADCAST_SETTING};
    use windows::Win32::System::RemoteDesktop::{
        WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
    };
    use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
    use windows::Win32::UI::WindowsAndMessaging::*;

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_WTSSESSION_CHANGE if wparam.0 as u32 == WTS_SESSION_LOCK => emit("pause", "lock"),
            WM_WTSSESSION_CHANGE if wparam.0 as u32 == WTS_SESSION_UNLOCK => emit("resume", "lock"),
            WM_POWERBROADCAST if wparam.0 as u32 == PBT_APMSUSPEND => emit("pause", "sleep"),
            WM_POWERBROADCAST if wparam.0 as u32 == PBT_APMRESUMEAUTOMATIC => {
                emit("resume", "sleep")
            }
            WM_POWERBROADCAST if wparam.0 as u32 == PBT_POWERSETTINGCHANGE && lparam.0 != 0 => {
                let setting = &*(lparam.0 as *const POWERBROADCAST_SETTING);
                if setting.PowerSetting == GUID_CONSOLE_DISPLAY_STATE {
                    // Data[0]: 0 = Display Off, 1 = Display On, 2 = Display Dimmed
                    match setting.Data[0] {
                        0 => emit("pause", "display"),
                        1 => emit("resume", "display"),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    pub fn start() {
        std::thread::spawn(|| unsafe {
            let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
            let class_name = w!("TruongPaperPowerWatcher");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            if let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class_name,
                w!(""),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinstance.into()),
                None,
            ) {
                let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
                let _ = RegisterPowerSettingNotification(
                    HANDLE(hwnd.0),
                    &GUID_CONSOLE_DISPLAY_STATE,
                    REGISTER_NOTIFICATION_FLAGS(0), // DEVICE_NOTIFY_WINDOW_HANDLE == 0
                );
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = DispatchMessageW(&msg);
                }
            }
        });
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    pub fn start() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_flags_track_pause_and_resume() {
        // Single test so the process-wide flags are not raced by another test.
        assert!(!rendering_suspended());
        let wake0 = wake_generation();
        record("pause", "display");
        assert!(rendering_suspended());
        record("pause", "lock");
        record("resume", "display");
        assert!(rendering_suspended());
        assert_eq!(wake_generation(), wake0, "still locked: not awake yet");
        record("resume", "lock");
        assert!(!rendering_suspended());
        assert_eq!(
            wake_generation(),
            wake0 + 1,
            "unlock makes the desktop visible"
        );
        record("resume", "lock");
        assert_eq!(
            wake_generation(),
            wake0 + 1,
            "duplicate resume is not a wake"
        );
        record("pause", "bogus");
        assert!(!rendering_suspended());
    }

    #[test]
    fn power_event_payload_shape() {
        assert_eq!(
            serde_json::to_value(PowerEvent {
                kind: "pause",
                reason: "sleep"
            })
            .unwrap(),
            serde_json::json!({"kind": "pause", "reason": "sleep"})
        );
    }
}
