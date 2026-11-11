// src-tauri/build.rs

/// Every app command registered in `generate_handler!` (docs/04 §5.1). Declaring them in an app
/// ACL manifest makes Tauri ACL-check custom commands: only windows granted the generated
/// `allow-<command>` permissions in `capabilities/` (the Studio and tray windows) can call them,
/// so the `wallpaper-*` webviews that run Workshop JavaScript get no IPC access at all.
/// `src/lib.rs` tests keep this list equal to the registered handlers.
const APP_COMMANDS: &[&str] = &[
    "photo_get_status",
    "photo_fetch_public_catalog",
    "photo_rotate_next",
    "photo_trigger_refill",
    "photo_add_album",
    "photo_toggle_album",
    "photo_remove_album",
    "photo_set_filter",
    "photo_set_favorite",
    "photo_blacklist_and_skip",
    "photo_set_cache_limit",
    "photo_clear_cache",
    "photo_reset_history",
    "steam_browse_workshop",
    "steam_inspect_url",
    "steam_download_workshop_item",
    "steam_login_qr",
    "steam_login_credentials",
    "steam_submit_2fa_code",
    "steam_logout",
    "steam_sync_local_folders",
    "library_list_wallpapers",
    "library_delete_wallpaper",
    "monitors_list",
    "monitor_apply_wallpaper",
    "monitor_update_property",
    "monitor_update_adjustments",
    "wallpaper_save_preset",
    "wallpaper_apply_preset",
    "playback_set_paused",
    "playlist_list",
    "playlist_save",
    "settings_get",
    "settings_update",
    "update_feed_status",
    "runtime_shutdown_for_update",
    "runtime_get_memory_usage",
];

fn attributes() -> tauri_build::Attributes {
    tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(APP_COMMANDS))
}

fn main() {
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");

    if windows_msvc {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());

        let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        tauri_build::try_build(attributes().windows_attributes(windows))
            .expect("failed to run tauri-build");
    } else {
        tauri_build::try_build(attributes()).expect("failed to run tauri-build");
    }
}
