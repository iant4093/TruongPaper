//! `truongpaper-steam`: Steam Workshop URL/ID parser, `ISteamRemoteStorage` metadata client,
//! `DepotDownloader` v3.4.0 sidecar controller, and local Steam `431960` folder sync.
//! Complete specification: `docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` (Section 1).

pub mod browse;
pub use browse::{browse_workshop, WorkshopBrowsePage, WorkshopBrowseQuery};

pub mod commands;
pub mod guest_download;
pub mod local_sync;
pub mod metadata;
pub mod qr;
pub mod sidecar;
pub mod workshop_id;

pub const WALLPAPER_ENGINE_APP_ID: u32 = 431960;
pub const WALLPAPER_ENGINE_CORE_DEPOT_ID: u32 = 431961;

pub use commands::{
    core_assets_bootstrap_args, core_assets_bootstrapped, password_login_args,
    pubfile_download_args, qr_login_args, write_core_assets_filelist, DEFAULT_MAX_DOWNLOADS,
};
pub use local_sync::{
    discover_all_steam_libraries, find_default_steam_roots, find_local_we_core_assets_dir,
    parse_library_folders_vdf, scan_workshop_directories, spawn_local_workshop_watcher,
    spawn_workshop_watcher, DiscoveredWorkshopWallpaper,
};
pub use metadata::{fetch_workshop_preview, parse_published_file_details, WorkshopPreview};
pub use qr::parse_ascii_qr_to_matrix;
pub use sidecar::{
    detect_2fa_prompt, run_depot_downloader_sidecar, SidecarEvent, StderrPromptScanner,
    StdoutParser, TwoFaPrompt,
};
pub use workshop_id::{parse_workshop_id, parse_workshop_url_or_id};
