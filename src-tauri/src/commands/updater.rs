//! Auto-updater diagnostic & clean shutdown commands (docs/06 §5.1).

#[tauri::command]
pub async fn update_feed_status(app: tauri::AppHandle) -> String {
    let url = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("endpoints"))
        .and_then(|e| e.get(0))
        .and_then(|e| e.as_str())
        .unwrap_or_default()
        .to_owned();
    if url.is_empty() {
        return "no update feed is configured".into();
    }
    let client = match reqwest::Client::builder()
        .user_agent("tauri-updater")
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => return e.to_string(),
    };
    match client.get(&url).send().await {
        Ok(res) if res.status().is_success() => "ok".into(),
        Ok(res) if res.status() == reqwest::StatusCode::NOT_FOUND => "feed_not_found".into(),
        Ok(res) => format!("GitHub answered HTTP {}", res.status()),
        Err(e) if e.is_timeout() => "GitHub didn’t answer within 15 seconds".into(),
        Err(e) if e.is_connect() => "couldn’t connect to GitHub (offline, VPN or firewall?)".into(),
        Err(e) => e.to_string(),
    }
}

#[tauri::command]
pub async fn runtime_shutdown_for_update(
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    // 1. Terminate any active Windows Application (.exe) JobObjects or DepotDownloader sidecars
    // 2. Detach any desktop child HWNDs from Progman/WorkerW so NSIS can overwrite TruongPaper.exe cleanly
    state.shutdown_all_wallpaper_hosts().await;
    Ok(())
}
