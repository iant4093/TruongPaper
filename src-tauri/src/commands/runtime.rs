//! Lightweight native runtime measurements for the Studio's RAM meter.
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMemoryUsage {
    pub resident_bytes: u64,
}

#[tauri::command]
pub fn runtime_get_memory_usage() -> Result<RuntimeMemoryUsage, String> {
    truongpaper_platform::memory::resident_memory_bytes()
        .map(|resident_bytes| RuntimeMemoryUsage { resident_bytes })
        .map_err(|err| err.to_string())
}
