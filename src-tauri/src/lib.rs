// src-tauri/src/lib.rs
pub mod audio_bridge;
pub mod commands;
pub mod db;
pub mod desktop_host;
pub mod host;
pub mod models;
pub mod power;
pub mod protocol;
pub mod scene_runtime;
pub mod scheduler;
pub mod secrets;
pub mod smart_pause;
pub mod state;
pub mod tray;
pub mod wallpaper_webview;

pub use state::AppState;

use commands::photo::PhotoServiceState;
use tauri::Manager;

/// SQLite file inside `app_data_dir`.
pub const DB_FILE_NAME: &str = "truongpaper.db";

fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let app_data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&app_data_dir)?;
    let conn = truongpaper_core::open_and_initialize_db(&app_data_dir.join(DB_FILE_NAME))?;
    let http = reqwest::Client::builder()
        .user_agent(concat!("TruongPaper/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let state = AppState::new(conn, app_data_dir.clone(), http.clone());
    let photo =
        PhotoServiceState::new(state.db.clone(), app_data_dir, http, state.settings.clone());
    let (settings, host) = (state.settings.clone(), state.host.clone());
    app.manage(state);
    app.manage(photo);

    tray::setup_system_tray(app.handle())?;
    power::start(app.handle());
    smart_pause::spawn(app.handle().clone(), settings.clone(), host.clone());
    // Live runtimes: Web / Video webview windows + their bridge, then the desktop render thread
    // (photos and scenes). Both stop on `AppState::shutdown_now`.
    wallpaper_webview::start(app.handle().clone(), host.clone());
    desktop_host::start(app.handle().clone(), host, settings);
    scheduler::spawn(app.handle().clone());
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_studio(app);
        }))
        .register_asynchronous_uri_scheme_protocol(
            protocol::WPE_SCHEME,
            |ctx, request, responder| {
                let host = ctx
                    .app_handle()
                    .try_state::<AppState>()
                    .map(|s| s.host.clone());
                let label = ctx.webview_label().to_string();
                // File reads (videos can be hundreds of MB) stay off the webview thread.
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(protocol::serve(host.as_deref(), &label, request));
                });
            },
        )
        .setup(setup_app)
        .invoke_handler(tauri::generate_handler![
            // --- Photo API & Cache ---
            commands::photo::photo_get_status,
            commands::photo::photo_fetch_public_catalog,
            commands::photo::photo_rotate_next,
            commands::photo::photo_trigger_refill,
            commands::photo::photo_add_album,
            commands::photo::photo_toggle_album,
            commands::photo::photo_remove_album,
            commands::photo::photo_set_filter,
            commands::photo::photo_set_favorite,
            commands::photo::photo_blacklist_and_skip,
            commands::photo::photo_set_cache_limit,
            commands::photo::photo_clear_cache,
            commands::photo::photo_reset_history,
            // --- Steam Workshop & Wallpaper Engine ---
            commands::steam::steam_browse_workshop,
            commands::steam::steam_inspect_url,
            commands::steam::steam_download_workshop_item,
            commands::steam::steam_login_qr,
            commands::steam::steam_login_credentials,
            commands::steam::steam_submit_2fa_code,
            commands::steam::steam_logout,
            commands::steam::steam_sync_local_folders,
            commands::library::library_list_wallpapers,
            commands::library::library_delete_wallpaper,
            // --- Monitor, Playback & Property Customization ---
            commands::monitor::monitors_list,
            commands::monitor::monitor_apply_wallpaper,
            commands::lock_screen::wallpaper_set_lock_screen,
            commands::monitor::monitor_update_property,
            commands::monitor::monitor_update_adjustments,
            commands::library::wallpaper_save_preset,
            commands::library::wallpaper_apply_preset,
            commands::monitor::playback_set_paused,
            // --- Playlists, Auto-Updater & Global Settings ---
            commands::playlist::playlist_list,
            commands::playlist::playlist_save,
            commands::settings::settings_get,
            commands::settings::settings_update,
            commands::updater::update_feed_status,
            commands::updater::runtime_shutdown_for_update,
            commands::runtime::runtime_get_memory_usage,
        ])
        .build(tauri::generate_context!())
        .expect("error while building TruongPaper Tauri application");

    app.run(|handle, event| match event {
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => tray::show_studio(handle),
        tauri::RunEvent::Exit => {
            if let Some(state) = handle.try_state::<AppState>() {
                state.shutdown_now();
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    /// Command names registered in `generate_handler!` above.
    fn registered_commands() -> BTreeSet<String> {
        let src = include_str!("lib.rs");
        let start = src.find("generate_handler![").expect("handler list") + 18;
        let end = start + src[start..].find(']').expect("handler list end");
        src[start..end]
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//"))
            .map(|l| {
                l.trim_end_matches(',')
                    .rsplit("::")
                    .next()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    /// Command names listed in docs/04 §5.1 (`invoke<...>('name'` / `invoke('name'`).
    fn documented_commands() -> BTreeSet<String> {
        let doc = include_str!("../../docs/04-UI-UX-LIQUID-GLASS-SPEC.md");
        let start = doc
            .find("### 5.1 Complete Tauri v2 IPC Commands")
            .expect("§5.1 heading");
        let end = start + doc[start..].find("#### Rust").expect("§5.1 events heading");
        doc[start..end]
            .split("invoke")
            .skip(1)
            .filter_map(|chunk| {
                let q = chunk.find('\'')?;
                let rest = &chunk[q + 1..];
                Some(rest[..rest.find('\'')?].to_string())
            })
            .collect()
    }

    #[test]
    fn registered_commands_match_docs_04_section_5_1() {
        let documented = documented_commands();
        assert_eq!(documented.len(), 38, "doc list changed: {documented:?}");
        assert_eq!(registered_commands(), documented);
    }

    #[test]
    fn event_names_match_docs_04_section_5_1() {
        let doc = include_str!("../../docs/04-UI-UX-LIQUID-GLASS-SPEC.md");
        for name in [
            crate::models::EVENT_WALLPAPER_CHANGED,
            crate::models::EVENT_PHOTO_QUEUE_UPDATED,
            crate::models::EVENT_STEAM_DOWNLOAD_PROGRESS,
            crate::models::EVENT_STEAM_QR_MATRIX,
            crate::models::EVENT_STEAM_NEED_2FA,
            crate::models::EVENT_POLICY_STATE_CHANGED,
            crate::models::EVENT_SYSTEM_POWER,
        ] {
            assert!(
                doc.contains(&format!("- `{name}`:")),
                "{name} not in docs/04"
            );
        }
    }

    #[test]
    fn tauri_config_has_required_windows_and_csp() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = conf["app"]["security"]["csp"].as_str().unwrap();
        let img_src = csp
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("img-src"))
            .unwrap();
        assert!(img_src.contains("https://*.cloudfront.net"));
        assert!(img_src.contains("https://*.steamusercontent.com"));
        let labels: Vec<&str> = conf["app"]["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, ["main", "tray-popover"]);

        let caps: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        assert_eq!(caps["windows"], serde_json::json!(["main", "tray-popover"]));
    }

    /// Command names in the `APP_COMMANDS` app ACL manifest of `build.rs`.
    fn manifest_commands() -> BTreeSet<String> {
        let src = include_str!("../build.rs");
        let start = src
            .find("APP_COMMANDS: &[&str] = &[")
            .expect("APP_COMMANDS");
        let end = start + src[start..].find("];").expect("APP_COMMANDS end");
        src[start..end]
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn app_acl_manifest_covers_every_registered_command() {
        // With an app manifest Tauri ACL-checks every app command, so a command missing here
        // would be unreachable from the Studio, and one missing from the handler list would
        // grant a permission for nothing.
        assert_eq!(manifest_commands(), registered_commands());

        let caps: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        let granted: BTreeSet<String> = caps["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p.as_str())
            .filter(|p| p.starts_with("allow-"))
            .map(str::to_string)
            .collect();
        let expected: BTreeSet<String> = registered_commands()
            .iter()
            .map(|c| format!("allow-{}", c.replace('_', "-")))
            .collect();
        assert_eq!(granted, expected);
    }

    #[test]
    fn no_capability_reaches_wallpaper_webviews() {
        // `wallpaper-*` windows run untrusted Workshop JavaScript: no capability file may name
        // them, directly or through a glob pattern.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
        let mut files = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let caps: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
            files += 1;
            for key in ["windows", "webviews"] {
                for w in caps[key].as_array().into_iter().flatten() {
                    let w = w.as_str().unwrap_or_default();
                    assert!(
                        !w.contains(['*', '?', '[']) && !w.starts_with("wallpaper"),
                        "{} grants {key} '{w}'",
                        path.display()
                    );
                }
            }
            assert!(caps.get("remote").is_none(), "no remote capabilities");
        }
        assert!(files >= 1);
        let label = crate::wallpaper_webview::wallpaper_window_label("DISPLAY-1");
        assert!(!["main", "tray-popover"].contains(&label.as_str()));
    }
}
