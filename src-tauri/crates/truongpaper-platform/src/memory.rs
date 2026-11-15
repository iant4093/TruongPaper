//! Resident RAM for this process, not photo-cache bytes or total system memory.

#[cfg(target_os = "macos")]
pub fn resident_memory_bytes() -> anyhow::Result<u64> {
    let mut info = std::mem::MaybeUninit::<libc::mach_task_basic_info>::zeroed();
    let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
    // SAFETY: the output buffer and count match MACH_TASK_BASIC_INFO; the task is our own.
    // libc is moving Mach bindings to mach2; this OS API remains supported.
    #[allow(deprecated)]
    let result = unsafe {
        libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            info.as_mut_ptr().cast(),
            &mut count,
        )
    };
    anyhow::ensure!(
        result == libc::KERN_SUCCESS,
        "Cannot read app RAM (Mach error {result})"
    );
    // SAFETY: successful task_info initialized this fixed-layout buffer.
    Ok(unsafe { info.assume_init() }.resident_size)
}

/// Windows: this process plus every process it started, directly or not. That covers the
/// WebView2 browser, renderer and GPU processes behind Studio and Web/Video wallpapers, the
/// Steam downloader and Application wallpapers, which run as our child processes.
#[cfg(target_os = "windows")]
pub fn resident_memory_bytes() -> anyhow::Result<u64> {
    use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId};
    // SAFETY: pseudo-handle refers to this process.
    let own = unsafe { working_set(GetCurrentProcess()) }?;
    let me = unsafe { GetCurrentProcessId() };
    let children: u64 = descendant_pids(&process_parents(), me)
        .into_iter()
        .filter_map(|pid| {
            use windows::Win32::Foundation::CloseHandle;
            use windows::Win32::System::Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            };
            // SAFETY: the handle is closed below; a process that exited just fails to open.
            unsafe {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
                let bytes = working_set(h).ok();
                let _ = CloseHandle(h);
                bytes
            }
        })
        .sum();
    Ok(own + children)
}

#[cfg(target_os = "windows")]
unsafe fn working_set(h: windows::Win32::Foundation::HANDLE) -> anyhow::Result<u64> {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size,
        ..Default::default()
    };
    GetProcessMemoryInfo(h, &mut counters, size)?;
    Ok(counters.WorkingSetSize as u64)
}

/// `(pid, parent pid)` of every running process.
#[cfg(target_os = "windows")]
fn process_parents() -> Vec<(u32, u32)> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: snapshot handle closed below; the entry carries its own size.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                out.push((e.th32ProcessID, e.th32ParentProcessID));
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Every pid below `root` in the `(pid, parent)` tree. Windows reuses pids, so a parent that
/// exited could look like the parent of an unrelated new process; the walk stops at cycles.
pub fn descendant_pids(parents: &[(u32, u32)], root: u32) -> Vec<u32> {
    let mut found: Vec<u32> = Vec::new();
    let mut frontier = vec![root];
    while let Some(p) = frontier.pop() {
        for &(pid, parent) in parents {
            if parent == p && pid != root && pid != 0 && !found.contains(&pid) {
                found.push(pid);
                frontier.push(pid);
            }
        }
    }
    found
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn resident_memory_bytes() -> anyhow::Result<u64> {
    anyhow::bail!("RAM readings are supported on macOS and Windows")
}

#[cfg(test)]
mod tree_tests {
    #[test]
    fn descendants_include_grandchildren_and_survive_cycles() {
        // 10 -> 11 -> 12, 10 -> 13; 20 is unrelated; 30 <-> 31 is a cycle from reused pids.
        let tree = [
            (11, 10),
            (12, 11),
            (13, 10),
            (20, 1),
            (30, 31),
            (31, 30),
            (10, 30),
        ];
        let mut got = super::descendant_pids(&tree, 10);
        got.sort();
        assert_eq!(got, vec![11, 12, 13]);
        let mut cyc = super::descendant_pids(&tree, 30);
        cyc.sort();
        assert_eq!(cyc, vec![10, 11, 12, 13, 31]);
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod tests {
    #[test]
    fn native_process_reading_reports_real_resident_bytes() {
        let bytes = super::resident_memory_bytes().unwrap();
        assert!(
            bytes > 0 && bytes < (1u64 << 50),
            "implausible process RAM: {bytes}"
        );
    }
}
