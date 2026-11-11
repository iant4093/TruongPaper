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

#[cfg(target_os = "windows")]
pub fn resident_memory_bytes() -> anyhow::Result<u64> {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size,
        ..Default::default()
    };
    // SAFETY: pseudo-handle refers to this process; the output pointer/size are valid.
    unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size)?;
    }
    Ok(counters.WorkingSetSize as u64)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn resident_memory_bytes() -> anyhow::Result<u64> {
    anyhow::bail!("RAM readings are supported on macOS and Windows")
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
