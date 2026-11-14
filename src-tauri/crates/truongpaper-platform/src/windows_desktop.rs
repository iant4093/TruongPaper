//! Windows desktop integration (classic Win10/11 23H2 `WorkerW` AND Windows 11 24H2+ raised
//! layered desktop). Spec: `docs/01-SYSTEM-ARCHITECTURE.md` section 2.1 (pitfall #1).
//!
//! Compiled as `crate::win_desktop` (Windows only).
//!
//! Windows 11 24H2 (build 26100+):
//! 1. `Progman` has `WS_EX_NOREDIRECTIONBITMAP`.
//! 2. `SHELLDLL_DefView` is a `WS_EX_LAYERED` direct child of `Progman`.
//! 3. `WorkerW` is ALSO a direct child of `Progman`.
//! 4. The render `HWND` must be `WS_CHILD` + `WS_EX_LAYERED` (alpha 255), parented to `Progman`,
//!    Z-ordered directly below `SHELLDLL_DefView` and above `WorkerW`.
//! 5. Never call `SystemParametersInfoW(SPI_SETDESKWALLPAPER, ...)`: it destroys the child `WorkerW`.

use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MapWindowPoints, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Undocumented `Progman` message that spawns the wallpaper `WorkerW` behind the desktop icons.
const WM_SPAWN_WORKERW: u32 = 0x052C;

#[derive(Debug, Clone, Copy)]
pub struct DesktopHandles {
    pub progman: HWND,
    pub shell_dll_defview: HWND,
    pub worker_w: HWND,
    /// True on Windows 11 24H2+ where Progman has WS_EX_NOREDIRECTIONBITMAP
    pub is_raised_layered_desktop: bool,
}

#[derive(Debug, Clone)]
pub struct Win32MonitorDesc {
    pub device_name: String,
    pub monitor_rect: RECT,
    pub work_rect: RECT,
    pub is_primary: bool,
}

#[derive(Debug, Clone)]
pub enum DesktopLifecycleEvent {
    /// explorer.exe restarted; re-run setup_desktop_layer() and re-parent/recreate wallpaper HWNDs
    ExplorerRestarted,
    /// Monitor plugged/unplugged, resolution changed, or work area updated
    DisplayTopologyChanged(Vec<Win32MonitorDesc>),
}

/// Lists all monitors (virtual-screen rects and work areas).
///
/// # Safety
/// Calls raw Win32 APIs.
pub unsafe fn enumerate_monitors_win32() -> Vec<Win32MonitorDesc> {
    let mut list: Vec<Win32MonitorDesc> = Vec::new();

    unsafe extern "system" fn monitor_enum_proc(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _lprect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let out = &mut *(lparam.0 as *mut Vec<Win32MonitorDesc>);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmonitor, &mut mi.monitorInfo as *mut _ as *mut _).as_bool() {
            let len = mi
                .szDevice
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(mi.szDevice.len());
            let device_name = String::from_utf16_lossy(&mi.szDevice[..len]);
            let is_primary = (mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0;
            out.push(Win32MonitorDesc {
                device_name,
                monitor_rect: mi.monitorInfo.rcMonitor,
                work_rect: mi.monitorInfo.rcWork,
                is_primary,
            });
        }
        BOOL(1)
    }

    let _ = EnumDisplayMonitors(
        None,
        None,
        Some(monitor_enum_proc),
        LPARAM(&mut list as *mut Vec<Win32MonitorDesc> as isize),
    );
    list
}

/// Finds `Progman` / `SHELLDLL_DefView` / `WorkerW` and detects the 24H2 raised layered desktop.
///
/// # Safety
/// Calls raw Win32 APIs; re-run after `ExplorerRestarted` since all handles become invalid.
pub unsafe fn setup_desktop_layer() -> Option<DesktopHandles> {
    let progman = FindWindowW(w!("Progman"), PCWSTR::null()).ok()?;
    if progman.0.is_null() {
        return None;
    }

    let ex_style = GetWindowLongPtrW(progman, GWL_EXSTYLE) as u32;
    let is_raised_layered_desktop = (ex_style & WS_EX_NOREDIRECTIONBITMAP.0) != 0;

    // Spawn WorkerW behind desktop icons
    let mut result: usize = 0;
    let _ = SendMessageTimeoutW(
        progman,
        WM_SPAWN_WORKERW,
        WPARAM(0xD),
        LPARAM(0x1),
        SMTO_NORMAL,
        1000,
        Some(&mut result as *mut usize),
    );

    struct FindCtx {
        shell_view: HWND,
        worker_w: HWND,
    }
    let mut ctx = FindCtx {
        shell_view: HWND::default(),
        worker_w: HWND::default(),
    };

    unsafe extern "system" fn enum_proc(top_hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut FindCtx);
        if let Ok(def_view) =
            FindWindowExW(Some(top_hwnd), None, w!("SHELLDLL_DefView"), PCWSTR::null())
        {
            if !def_view.0.is_null() {
                ctx.shell_view = def_view;
                if let Ok(next_worker) =
                    FindWindowExW(None, Some(top_hwnd), w!("WorkerW"), PCWSTR::null())
                {
                    ctx.worker_w = next_worker;
                }
            }
        }
        BOOL(1)
    }

    let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut FindCtx as isize));
    let mut shell_dll_defview = ctx.shell_view;
    let mut worker_w = ctx.worker_w;

    // Windows 11 24H2 override: WorkerW is a direct child of Progman below SHELLDLL_DefView
    if is_raised_layered_desktop {
        if let Ok(child_worker) = FindWindowExW(Some(progman), None, w!("WorkerW"), PCWSTR::null())
        {
            if !child_worker.0.is_null() {
                worker_w = child_worker;
            }
        }
        // On 24H2 SHELLDLL_DefView is also a direct child of Progman.
        if shell_dll_defview.0.is_null() {
            if let Ok(def_view) =
                FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), PCWSTR::null())
            {
                shell_dll_defview = def_view;
            }
        }
    }

    if worker_w.0.is_null() {
        return None;
    }

    Some(DesktopHandles {
        progman,
        shell_dll_defview,
        worker_w,
        is_raised_layered_desktop,
    })
}

/// Attaches a per-monitor wallpaper HWND behind the desktop icons for `monitor_bounds` (in virtual screen coords).
///
/// # Safety
/// `hwnd` must be a window owned by this process; `desktop` must come from a current
/// [`setup_desktop_layer`] call.
pub unsafe fn attach_hwnd_to_desktop(
    hwnd: HWND,
    desktop: &DesktopHandles,
    monitor_bounds: RECT,
) -> bool {
    if !IsWindow(Some(hwnd)).as_bool() {
        return false;
    }

    let width = monitor_bounds.right - monitor_bounds.left;
    let height = monitor_bounds.bottom - monitor_bounds.top;

    // 1. Move window to target monitor coordinates in virtual screen space first
    let _ = SetWindowPos(
        hwnd,
        Some(HWND_BOTTOM),
        monitor_bounds.left,
        monitor_bounds.top,
        width,
        height,
        SWP_NOACTIVATE,
    );

    // 2. Map monitor rectangle into target parent client coordinates using [POINT; 2]
    let target_parent = if desktop.is_raised_layered_desktop {
        desktop.progman
    } else {
        desktop.worker_w
    };
    let mut mapped_rect = RECT::default();
    let _ = GetWindowRect(hwnd, &mut mapped_rect);
    let mut pts = [
        POINT {
            x: mapped_rect.left,
            y: mapped_rect.top,
        },
        POINT {
            x: mapped_rect.right,
            y: mapped_rect.bottom,
        },
    ];
    // `None` = screen coordinates.
    let _ = MapWindowPoints(None, Some(target_parent), &mut pts);

    // 3. Apply parent and Z-order depending on Win11 24H2 vs classic WorkerW
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    let new_style = (style & !WS_POPUP.0) | WS_CHILD.0 | WS_VISIBLE.0;
    SetWindowLongPtrW(hwnd, GWL_STYLE, new_style as isize);

    if desktop.is_raised_layered_desktop {
        // MUST add WS_EX_LAYERED with alpha=255 BEFORE SetParent on 24H2
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex_style | WS_EX_LAYERED.0) as isize);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);

        let _ = SetParent(hwnd, Some(desktop.progman));

        // Order hwnd directly beneath SHELLDLL_DefView
        let _ = SetWindowPos(
            hwnd,
            Some(desktop.shell_dll_defview),
            pts[0].x,
            pts[0].y,
            width,
            height,
            SWP_NOACTIVATE,
        );

        // Ensure WorkerW stays at the very bottom of Progman's children
        let _ = SetWindowPos(
            desktop.worker_w,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        // NOTE: never SPI_SETDESKWALLPAPER here; on 24H2 it destroys the child WorkerW.
    } else {
        let _ = SetParent(hwnd, Some(desktop.worker_w));
        let _ = SetWindowPos(
            hwnd,
            None,
            pts[0].x,
            pts[0].y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );

        // Refresh desktop only on classic non-24H2 (on 24H2 this would destroy WorkerW!)
        let _ = SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, None, SPIF_UPDATEINIFILE);
    }

    true
}

static TASKBAR_CREATED_MSG: OnceLock<u32> = OnceLock::new();
static LIFECYCLE_TX: OnceLock<Sender<DesktopLifecycleEvent>> = OnceLock::new();

unsafe extern "system" fn lifecycle_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let taskbar_msg = *TASKBAR_CREATED_MSG.get().unwrap_or(&0);
    if taskbar_msg != 0 && msg == taskbar_msg {
        if let Some(tx) = LIFECYCLE_TX.get() {
            let _ = tx.send(DesktopLifecycleEvent::ExplorerRestarted);
        }
    } else if msg == WM_DISPLAYCHANGE
        || (msg == WM_SETTINGCHANGE && wparam.0 == SPI_SETWORKAREA.0 as usize)
    {
        if let Some(tx) = LIFECYCLE_TX.get() {
            let _ = tx.send(DesktopLifecycleEvent::DisplayTopologyChanged(
                enumerate_monitors_win32(),
            ));
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Spawns a dedicated Win32 message listener that catches `TaskbarCreated` (explorer.exe crash/restart)
/// and `WM_DISPLAYCHANGE` / `SPI_SETWORKAREA` (monitor hotplug, resolution, or DPI scaling changes).
///
/// Only the first call installs the sender; later calls are ignored.
pub fn spawn_desktop_lifecycle_watcher(tx: Sender<DesktopLifecycleEvent>) -> std::io::Result<()> {
    if LIFECYCLE_TX.set(tx).is_err() {
        return Ok(());
    }
    std::thread::Builder::new()
        .name("truongpaper-lifecycle".into())
        .spawn(|| unsafe {
            let msg_id = RegisterWindowMessageW(w!("TaskbarCreated"));
            let _ = TASKBAR_CREATED_MSG.set(msg_id);

            let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
            let class_name = w!("TruongPaperLifecycleWatcher");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(lifecycle_wndproc),
                hInstance: hinstance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassExW(&wc);

            // Must be a top-level WS_POPUP window (NOT HWND_MESSAGE) to receive TaskbarCreated and WM_DISPLAYCHANGE broadcasts
            if let Ok(_hwnd) = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                }
            }
        })?;
    Ok(())
}
