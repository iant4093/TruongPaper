//! Windows `Application` wallpaper host.
//! Spec: `docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` section 10.
//!
//! `"type": "application"` Workshop items are native Windows executables. On macOS the UI flags
//! them `windowsOnly: true` and shows a "Windows Only" badge (see
//! [`application_wallpapers_supported`]); on Windows they are launched with window-bounds
//! arguments, their window is re-parented behind the desktop icons, and the whole process tree
//! is terminated when switching wallpapers or shutting down.

/// `true` when `"type": "application"` wallpapers can run on the current OS (Windows only).
pub const fn application_wallpapers_supported() -> bool {
    cfg!(target_os = "windows")
}

/// `true` when a Wallpaper Engine `project.json` `type` can be played on this OS.
/// Only `application` is OS-restricted; every other type is cross-platform.
pub fn is_wallpaper_type_supported_on_this_os(wallpaper_type: &str) -> bool {
    if wallpaper_type.eq_ignore_ascii_case("application") {
        application_wallpapers_supported()
    } else {
        true
    }
}

/// Command-line arguments passed to Unity-style application wallpapers.
pub fn application_launch_args(width: i32, height: i32) -> Vec<String> {
    vec![
        "-popupwindow".into(),
        "-screen-fullscreen".into(),
        "0".into(),
        "-screen-width".into(),
        width.to_string(),
        "-screen-height".into(),
        height.to_string(),
    ]
}

#[cfg(target_os = "windows")]
pub use imp::{ApplicationWallpaperProcess, MonitorRect, WINDOW_SEARCH_TIMEOUT};

#[cfg(target_os = "windows")]
mod imp {
    //! Process lifetime: every application wallpaper runs in its own Job Object with
    //! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. The process is created suspended, assigned to the
    //! job and only then resumed, so no child it spawns (e.g. `UnityCrashHandler64.exe`) can
    //! escape. Closing the job handle (stop, drop, or TruongPaper crashing) kills the tree.
    //!
    //! Window: a helper thread polls `EnumWindows` for the visible top-level window of any
    //! process in the job, strips its frame / taskbar styles and attaches it behind the desktop
    //! icons like the Web / Video wallpaper windows. All cross-process window calls run on
    //! helper threads: they send messages synchronously and a hung wallpaper must not stall
    //! the caller.
    //!
    //! Pause: `NtSuspendProcess` / `NtResumeProcess` (ntdll) on every process of the job.

    use super::application_launch_args;
    use parking_lot::Mutex;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Child, Command};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, OnceLock};
    use std::time::{Duration, Instant};
    use windows::core::{s, w, BOOL, PCSTR, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, NTSTATUS, RECT};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicProcessIdList,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        TerminateJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, CREATE_SUSPENDED, PROCESS_SUSPEND_RESUME,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;

    /// How long after launch the wallpaper's window is searched for.
    pub const WINDOW_SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
    const WINDOW_POLL: Duration = Duration::from_millis(100);
    /// `stop()` never waits longer than this for the process to exit.
    const STOP_WAIT: Duration = Duration::from_secs(2);
    /// Process ids read from the job at most (Unity apps run 2-3 processes).
    const MAX_JOB_PIDS: usize = 256;

    /// A monitor rectangle in physical virtual-screen pixels.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MonitorRect {
        pub x: i32,
        pub y: i32,
        pub width: i32,
        pub height: i32,
    }

    impl MonitorRect {
        fn to_rect(self) -> RECT {
            RECT {
                left: self.x,
                top: self.y,
                right: self.x + self.width,
                bottom: self.y + self.height,
            }
        }
    }

    /// Owned Job Object handle.
    struct Job(HANDLE);
    // SAFETY: kernel handles are process-wide; the job APIs used here are thread-safe.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        fn new_kill_on_close() -> windows::core::Result<Self> {
            // SAFETY: plain Win32 calls; the handle is owned by `Job` from here on.
            unsafe {
                let job = Job(CreateJobObjectW(None, PCWSTR::null())?);
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )?;
                Ok(job)
            }
        }

        /// Ids of the processes currently in the job.
        fn process_ids(&self) -> Vec<u32> {
            // Header (two u32) + the id array; `usize` keeps the array aligned.
            let mut buf = vec![0usize; MAX_JOB_PIDS + 2];
            let bytes = (buf.len() * std::mem::size_of::<usize>()) as u32;
            let list = buf.as_mut_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
            // SAFETY: `buf` is large enough for the header plus `MAX_JOB_PIDS` ids and is
            // suitably aligned; the kernel writes at most `bytes`.
            unsafe {
                if QueryInformationJobObject(
                    Some(self.0),
                    JobObjectBasicProcessIdList,
                    list.cast(),
                    bytes,
                    None,
                )
                .is_err()
                {
                    return Vec::new();
                }
                let n = ((*list).NumberOfProcessIdsInList as usize).min(MAX_JOB_PIDS);
                let ids = std::ptr::addr_of!((*list).ProcessIdList).cast::<usize>();
                (0..n).map(|i| *ids.add(i) as u32).collect()
            }
        }

        fn terminate(&self) -> bool {
            // SAFETY: valid job handle.
            unsafe { TerminateJobObject(self.0, 1).is_ok() }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is owned; closing it kills what is left in the job.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    type NtProcessFn = unsafe extern "system" fn(HANDLE) -> NTSTATUS;

    fn ntdll_fn(name: PCSTR) -> Option<NtProcessFn> {
        // SAFETY: ntdll is always loaded; both exports have the `NTSTATUS (HANDLE)` signature.
        unsafe {
            let module = GetModuleHandleW(w!("ntdll.dll")).ok()?;
            let f = GetProcAddress(module, name)?;
            Some(std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                NtProcessFn,
            >(f))
        }
    }

    fn nt_suspend_process() -> Option<NtProcessFn> {
        static F: OnceLock<Option<NtProcessFn>> = OnceLock::new();
        *F.get_or_init(|| ntdll_fn(s!("NtSuspendProcess")))
    }

    fn nt_resume_process() -> Option<NtProcessFn> {
        static F: OnceLock<Option<NtProcessFn>> = OnceLock::new();
        *F.get_or_init(|| ntdll_fn(s!("NtResumeProcess")))
    }

    /// Suspends / resumes every thread of `process` (a handle with `PROCESS_SUSPEND_RESUME`).
    fn suspend_handle(process: HANDLE, suspend: bool) -> bool {
        let f = if suspend {
            nt_suspend_process()
        } else {
            nt_resume_process()
        };
        // SAFETY: `process` is a valid process handle.
        f.is_some_and(|f| unsafe { f(process) }.is_ok())
    }

    fn suspend_pid(pid: u32, suspend: bool) -> bool {
        // SAFETY: the opened handle is closed before returning.
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_SUSPEND_RESUME, false, pid) else {
                return false;
            };
            let ok = suspend_handle(process, suspend);
            let _ = CloseHandle(process);
            ok
        }
    }

    /// The largest visible, unowned top-level window of any process in `pids`.
    fn find_window_of(pids: &[u32]) -> Option<HWND> {
        struct Ctx<'a> {
            pids: &'a [u32],
            best: Option<(HWND, i64)>,
        }
        unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let ctx = &mut *(lparam.0 as *mut Ctx);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if !ctx.pids.contains(&pid) || !IsWindowVisible(hwnd).as_bool() {
                return BOOL(1);
            }
            if GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.0.is_null()) {
                return BOOL(1);
            }
            let mut class = [0u16; 64];
            let len = GetClassNameW(hwnd, &mut class).max(0) as usize;
            if String::from_utf16_lossy(&class[..len]) == "ConsoleWindowClass" {
                return BOOL(1);
            }
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            let area = i64::from(r.right - r.left) * i64::from(r.bottom - r.top);
            if ctx.best.is_none_or(|(_, a)| area > a) {
                ctx.best = Some((hwnd, area));
            }
            BOOL(1)
        }
        let mut ctx = Ctx { pids, best: None };
        // SAFETY: `ctx` outlives the synchronous enumeration.
        unsafe {
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut Ctx as isize));
        }
        ctx.best.map(|(h, _)| h)
    }

    /// Strips the frame and taskbar presence of a foreign top-level window.
    ///
    /// # Safety
    /// `hwnd` must be a window handle (it may belong to another process).
    unsafe fn strip_window_chrome(hwnd: HWND) {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let strip =
            WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0;
        SetWindowLongPtrW(hwnd, GWL_STYLE, (style & !strip) as isize);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let ex = (ex & !WS_EX_APPWINDOW.0) | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
    }

    /// State shared with the window attach helper threads.
    struct Shared {
        job: Job,
        child_pid: u32,
        hwnd: Mutex<Option<isize>>,
        rect: Mutex<MonitorRect>,
        stopped: AtomicBool,
        suspended: AtomicBool,
        /// A re-attach was requested while suspended (or Explorer was not ready); done on resume.
        reattach_pending: AtomicBool,
        /// Serializes attach passes.
        attach_lock: Mutex<()>,
    }

    impl Shared {
        fn pids(&self) -> Vec<u32> {
            let mut pids = self.job.process_ids();
            if !pids.contains(&self.child_pid) {
                pids.push(self.child_pid);
            }
            pids
        }

        /// The stored window if it still exists, else a fresh search.
        fn current_window(&self) -> Option<HWND> {
            let stored = *self.hwnd.lock();
            if let Some(h) = stored {
                let hwnd = HWND(h as *mut _);
                // SAFETY: `IsWindow` accepts any value.
                if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                    return Some(hwnd);
                }
            }
            let found = find_window_of(&self.pids());
            *self.hwnd.lock() = found.map(|h| h.0 as isize);
            found
        }

        /// Puts the wallpaper window behind the desktop icons on its monitor. `false` when there
        /// is no window yet, the desktop layer is not ready, or the process is suspended.
        fn attach(&self) -> bool {
            let _guard = self.attach_lock.lock();
            if self.stopped.load(Ordering::Acquire) {
                return false;
            }
            if self.suspended.load(Ordering::Acquire) {
                // Window calls on a suspended process would block until it resumes.
                self.reattach_pending.store(true, Ordering::Release);
                return false;
            }
            let Some(hwnd) = self.current_window() else {
                return false;
            };
            let rect = *self.rect.lock();
            // SAFETY: Win32 window calls on an existing window; may block while the target is
            // busy, which is why this runs on a helper thread.
            let attached = unsafe {
                let Some(desktop) = crate::win_desktop::setup_desktop_layer() else {
                    self.reattach_pending.store(true, Ordering::Release);
                    return false;
                };
                // Hidden while the styles change so the taskbar drops its button.
                let _ = ShowWindow(hwnd, SW_HIDE);
                strip_window_chrome(hwnd);
                let ok = crate::win_desktop::attach_hwnd_to_desktop(hwnd, &desktop, rect.to_rect());
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
                let _ = ShowWindow(hwnd, SW_SHOWNA);
                ok
            };
            self.reattach_pending.store(!attached, Ordering::Release);
            if attached {
                tracing::info!(
                    "application wallpaper window {:?} attached at {rect:?}",
                    hwnd.0
                );
            }
            attached
        }
    }

    /// Runs `f` on a short-lived helper thread.
    fn on_helper_thread(shared: &Arc<Shared>, f: impl FnOnce(&Shared) + Send + 'static) {
        let shared = Arc::clone(shared);
        let spawned = std::thread::Builder::new()
            .name("truongpaper-app-window".into())
            .spawn(move || f(&shared));
        if let Err(e) = spawned {
            tracing::warn!("could not start the application window thread: {e}");
        }
    }

    /// Owns a running application wallpaper process tree; kills it on [`stop`](Self::stop) or
    /// drop.
    pub struct ApplicationWallpaperProcess {
        child: Option<Child>,
        shared: Arc<Shared>,
    }

    impl ApplicationWallpaperProcess {
        /// Launches the Application wallpaper `exe_path` sized for `monitor` and attaches its
        /// window behind the desktop icons once it appears.
        pub fn spawn(exe_path: &Path, monitor: MonitorRect) -> anyhow::Result<Self> {
            let mut cmd = Command::new(exe_path);
            cmd.current_dir(exe_path.parent().unwrap_or_else(|| Path::new(".")))
                .args(application_launch_args(monitor.width, monitor.height));
            Self::spawn_command(cmd, monitor)
        }

        /// Spawns `cmd` inside a kill-on-close Job Object and starts the window search.
        pub fn spawn_command(mut cmd: Command, monitor: MonitorRect) -> anyhow::Result<Self> {
            let job = Job::new_kill_on_close()
                .map_err(|e| anyhow::anyhow!("could not create a job object: {e}"))?;
            let mut child = cmd.creation_flags(CREATE_SUSPENDED.0).spawn()?;
            let process = HANDLE(child.as_raw_handle());
            // SAFETY: `process` is the live (suspended) child handle owned by `child`.
            let assigned = unsafe { AssignProcessToJobObject(job.0, process) };
            if let Err(e) = assigned {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("could not add the process to its job object: {e}");
            }
            if !suspend_handle(process, false) {
                drop(job); // kills the suspended child
                let _ = child.wait();
                anyhow::bail!("could not resume the process");
            }
            let shared = Arc::new(Shared {
                job,
                child_pid: child.id(),
                hwnd: Mutex::new(None),
                rect: Mutex::new(monitor),
                stopped: AtomicBool::new(false),
                suspended: AtomicBool::new(false),
                reattach_pending: AtomicBool::new(false),
                attach_lock: Mutex::new(()),
            });
            on_helper_thread(&shared, |s| {
                let deadline = Instant::now() + WINDOW_SEARCH_TIMEOUT;
                while !s.stopped.load(Ordering::Acquire) && Instant::now() < deadline {
                    if s.suspended.load(Ordering::Acquire) {
                        s.reattach_pending.store(true, Ordering::Release);
                        return;
                    }
                    if find_window_of(&s.pids()).is_some() {
                        s.attach();
                        return;
                    }
                    std::thread::sleep(WINDOW_POLL);
                }
                if !s.stopped.load(Ordering::Acquire) {
                    tracing::warn!(
                        "application wallpaper (pid {}) showed no window within {:?}",
                        s.child_pid,
                        WINDOW_SEARCH_TIMEOUT
                    );
                    s.reattach_pending.store(true, Ordering::Release);
                }
            });
            Ok(Self {
                child: Some(child),
                shared,
            })
        }

        pub fn id(&self) -> Option<u32> {
            self.child.as_ref().map(|c| c.id())
        }

        /// Ids of every process in the job (the wallpaper and anything it spawned).
        pub fn process_ids(&self) -> Vec<u32> {
            self.shared.job.process_ids()
        }

        /// The attached wallpaper window (raw `HWND`), once found.
        pub fn hwnd(&self) -> Option<isize> {
            *self.shared.hwnd.lock()
        }

        /// Blocks until the wallpaper window was found, or `timeout`.
        pub fn wait_for_window(&self, timeout: Duration) -> Option<isize> {
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(h) = self.hwnd() {
                    return Some(h);
                }
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(WINDOW_POLL);
            }
        }

        /// Attaches the window again (Explorer restart, display change), at `monitor` when
        /// given. Runs on a helper thread; deferred to the resume while suspended.
        pub fn reattach(&self, monitor: Option<MonitorRect>) {
            if let Some(rect) = monitor {
                *self.shared.rect.lock() = rect;
            }
            if self.shared.suspended.load(Ordering::Acquire) {
                self.shared.reattach_pending.store(true, Ordering::Release);
                return;
            }
            on_helper_thread(&self.shared, |s| {
                s.attach();
            });
        }

        pub fn is_suspended(&self) -> bool {
            self.shared.suspended.load(Ordering::Acquire)
        }

        /// Suspends (`true`) or resumes every process of the job. Repeated calls with the same
        /// state are no-ops (suspend counts would stack otherwise).
        pub fn set_suspended(&self, suspend: bool) {
            let s = &self.shared;
            if s.stopped.load(Ordering::Acquire)
                || s.suspended.swap(suspend, Ordering::AcqRel) == suspend
            {
                return;
            }
            // Let a running attach pass finish first (its window calls would block on the
            // suspended process), but never stall the caller on a hung wallpaper window.
            let guard = s.attach_lock.try_lock_for(Duration::from_millis(200));
            let pids = s.pids();
            let failed = pids
                .iter()
                .filter(|&&pid| !suspend_pid(pid, suspend))
                .count();
            drop(guard);
            tracing::debug!(
                "application wallpaper {} {} process(es) ({failed} failed)",
                if suspend { "suspended" } else { "resumed" },
                pids.len()
            );
            if !suspend && s.reattach_pending.load(Ordering::Acquire) {
                on_helper_thread(s, |s| {
                    s.attach();
                });
            }
        }

        /// Terminates the whole process tree and waits (bounded) for the main process to exit.
        pub fn stop(&mut self) {
            self.shared.stopped.store(true, Ordering::Release);
            let Some(mut child) = self.child.take() else {
                return;
            };
            if !self.shared.job.terminate() {
                tracing::warn!("TerminateJobObject failed; killing the main process only");
                let _ = child.kill();
            }
            // SAFETY: the child handle stays valid until `child` is dropped.
            let waited = unsafe {
                WaitForSingleObject(HANDLE(child.as_raw_handle()), STOP_WAIT.as_millis() as u32)
            };
            if waited != windows::Win32::Foundation::WAIT_OBJECT_0 {
                tracing::warn!(
                    "application wallpaper (pid {}) did not exit within {STOP_WAIT:?}",
                    child.id()
                );
            } else {
                let _ = child.try_wait();
            }
        }
    }

    impl Drop for ApplicationWallpaperProcess {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_support_flag_matches_os() {
        assert_eq!(
            application_wallpapers_supported(),
            cfg!(target_os = "windows")
        );
        assert_eq!(
            is_wallpaper_type_supported_on_this_os("application"),
            cfg!(target_os = "windows")
        );
        assert_eq!(
            is_wallpaper_type_supported_on_this_os("Application"),
            cfg!(target_os = "windows")
        );
        for t in ["scene", "video", "web", "preset"] {
            assert!(is_wallpaper_type_supported_on_this_os(t), "{t}");
        }
    }

    #[test]
    fn launch_args_carry_bounds() {
        assert_eq!(
            application_launch_args(2560, 1440),
            [
                "-popupwindow",
                "-screen-fullscreen",
                "0",
                "-screen-width",
                "2560",
                "-screen-height",
                "1440"
            ]
        );
    }

    /// The job owns the whole process tree: a grandchild is in it and dies with it.
    #[cfg(target_os = "windows")]
    #[test]
    fn job_kills_the_process_tree_on_stop() {
        let mut cmd = std::process::Command::new("cmd.exe");
        cmd.args(["/c", "ping -n 30 127.0.0.1 >NUL"]);
        let rect = MonitorRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let mut p = ApplicationWallpaperProcess::spawn_command(cmd, rect).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while p.process_ids().len() < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let pids = p.process_ids();
        assert!(pids.len() >= 2, "cmd + ping expected in the job: {pids:?}");
        p.set_suspended(true);
        assert!(p.is_suspended());
        p.set_suspended(false);
        let started = std::time::Instant::now();
        p.stop();
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        assert!(p.process_ids().is_empty());
    }

    /// Functional check on a real desktop: a WinForms window opened by a grandchild
    /// (`cmd` -> `powershell`) is found, loses its frame and becomes a child of the desktop
    /// layer; stopping the job destroys it. Briefly shows a 480x270 window on the desktop.
    /// Run with `cargo test -p truongpaper-platform -- --ignored application_window`.
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "opens a window on the interactive desktop"]
    fn application_window_is_attached_behind_the_desktop_icons() {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::*;

        let script = "Add-Type -AssemblyName System.Windows.Forms; \
                      $f = New-Object System.Windows.Forms.Form; $f.Text = 'tp-app-test'; \
                      [System.Windows.Forms.Application]::Run($f)";
        let mut cmd = std::process::Command::new("cmd.exe");
        cmd.args(["/c", "powershell", "-NoProfile", "-Command", script]);
        let rect = MonitorRect {
            x: 0,
            y: 0,
            width: 480,
            height: 270,
        };
        let mut p = ApplicationWallpaperProcess::spawn_command(cmd, rect).unwrap();
        let raw = p
            .wait_for_window(WINDOW_SEARCH_TIMEOUT + std::time::Duration::from_secs(2))
            .expect("window found");
        // Give the attach pass time to finish.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let hwnd = HWND(raw as *mut _);
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let parent = GetParent(hwnd).unwrap_or_default();
            let mut class = [0u16; 64];
            let len = GetClassNameW(parent, &mut class).max(0) as usize;
            let parent_class = String::from_utf16_lossy(&class[..len]);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            eprintln!(
                "hwnd={raw:#x} pid={pid} job={:?} style={style:#x} ex={ex:#x} parent={parent_class}",
                p.process_ids()
            );
            assert_ne!(Some(pid), p.id(), "the window belongs to the grandchild");
            assert!(p.process_ids().contains(&pid));
            assert_ne!(style & WS_CHILD.0, 0);
            assert_eq!(style & WS_CAPTION.0, 0);
            assert_eq!(style & WS_THICKFRAME.0, 0);
            assert_ne!(ex & WS_EX_TOOLWINDOW.0, 0);
            assert_ne!(ex & WS_EX_NOACTIVATE.0, 0);
            assert_eq!(ex & WS_EX_APPWINDOW.0, 0);
            assert!(parent_class == "Progman" || parent_class == "WorkerW");

            p.set_suspended(true);
            p.set_suspended(false);
            p.stop();
            std::thread::sleep(std::time::Duration::from_millis(200));
            assert!(
                !IsWindow(Some(hwnd)).as_bool(),
                "window destroyed with the job"
            );
        }
    }
}
