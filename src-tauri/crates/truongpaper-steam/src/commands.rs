//! `DepotDownloader` v3.4.0 argument builders (pure functions, no process spawning).

use crate::workshop_id::parse_workshop_id;
use crate::{WALLPAPER_ENGINE_APP_ID, WALLPAPER_ENGINE_CORE_DEPOT_ID};
use anyhow::{bail, Result};
use std::path::Path;

/// Default `-max-downloads` for Workshop item downloads.
pub const DEFAULT_MAX_DOWNLOADS: u32 = 8;

/// File list consumed by `-filelist` for the first-run core assets bootstrap.
pub const CORE_ASSETS_FILELIST_NAME: &str = "assets_only.txt";
/// Contents of [`CORE_ASSETS_FILELIST_NAME`] (DepotDownloader `regex:` filelist syntax).
pub const CORE_ASSETS_FILELIST_CONTENT: &str =
    "regex:^assets/(shaders|materials|effects|models)/.*";

fn s(v: impl Into<String>) -> String {
    v.into()
}

fn validate_username(user: &str) -> Result<&str> {
    let user = user.trim();
    if user.is_empty() || user.starts_with('-') || user.chars().any(char::is_whitespace) {
        bail!("Invalid Steam account name");
    }
    Ok(user)
}

fn path_arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `-app 431960 -qr -remember-password -manifest-only`
pub fn qr_login_args() -> Vec<String> {
    vec![
        s("-app"),
        s(WALLPAPER_ENGINE_APP_ID.to_string()),
        s("-qr"),
        s("-remember-password"),
        s("-manifest-only"),
    ]
}

/// `-app 431960 -username <USER> -password <PASS> -remember-password -no-mobile -manifest-only`
pub fn password_login_args(username: &str, password: &str) -> Result<Vec<String>> {
    let username = validate_username(username)?;
    if password.is_empty() {
        bail!("Steam password must not be empty");
    }
    Ok(vec![
        s("-app"),
        s(WALLPAPER_ENGINE_APP_ID.to_string()),
        s("-username"),
        s(username),
        s("-password"),
        s(password),
        s("-remember-password"),
        s("-no-mobile"),
        s("-manifest-only"),
    ])
}

/// `-app 431960 -pubfile <ID> -username <USER> -remember-password -dir <DIR> -max-downloads 8`
///
/// `workshop_id_or_url` may be any form accepted by [`parse_workshop_id`].
pub fn pubfile_download_args(
    workshop_id_or_url: &str,
    saved_username: &str,
    out_dir: &Path,
    max_downloads: u32,
) -> Result<Vec<String>> {
    let Some(id) = parse_workshop_id(workshop_id_or_url) else {
        bail!(
            "Invalid Steam Workshop URL or ID: {}",
            workshop_id_or_url.trim()
        );
    };
    let username = validate_username(saved_username)?;
    Ok(vec![
        s("-app"),
        s(WALLPAPER_ENGINE_APP_ID.to_string()),
        s("-pubfile"),
        id,
        s("-username"),
        s(username),
        s("-remember-password"),
        s("-dir"),
        path_arg(out_dir),
        s("-max-downloads"),
        s(max_downloads.max(1).to_string()),
    ])
}

/// First-run Wallpaper Engine core assets bootstrap:
/// `-app 431960 -depot 431961 -filelist <FILELIST> -username <USER> -remember-password -dir <DIR>`
pub fn core_assets_bootstrap_args(
    saved_username: &str,
    filelist_path: &Path,
    out_dir: &Path,
) -> Result<Vec<String>> {
    let username = validate_username(saved_username)?;
    Ok(vec![
        s("-app"),
        s(WALLPAPER_ENGINE_APP_ID.to_string()),
        s("-depot"),
        s(WALLPAPER_ENGINE_CORE_DEPOT_ID.to_string()),
        s("-filelist"),
        path_arg(filelist_path),
        s("-username"),
        s(username),
        s("-remember-password"),
        s("-dir"),
        path_arg(out_dir),
    ])
}

/// Writes `assets_only.txt` into `dir` (created if missing) and returns its path.
pub fn write_core_assets_filelist(dir: &Path) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(CORE_ASSETS_FILELIST_NAME);
    std::fs::write(&path, CORE_ASSETS_FILELIST_CONTENT)?;
    Ok(path)
}

/// True when a previous bootstrap already produced `<dir>/assets/shaders`.
pub fn core_assets_bootstrapped(dir: &Path) -> bool {
    dir.join("assets").join("shaders").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn qr_args() {
        assert_eq!(
            qr_login_args(),
            [
                "-app",
                "431960",
                "-qr",
                "-remember-password",
                "-manifest-only"
            ]
        );
    }

    #[test]
    fn password_args_and_validation() {
        let a = password_login_args("ian", "pw").unwrap();
        assert_eq!(
            a,
            [
                "-app",
                "431960",
                "-username",
                "ian",
                "-password",
                "pw",
                "-remember-password",
                "-no-mobile",
                "-manifest-only"
            ]
        );
        assert!(password_login_args("", "pw").is_err());
        assert!(password_login_args("-qr", "pw").is_err());
        assert!(password_login_args("a b", "pw").is_err());
        assert!(password_login_args("ian", "").is_err());
    }

    #[test]
    fn pubfile_args_accept_url_and_reject_garbage() {
        let dir = PathBuf::from("/data/wallpapers/workshop/2841928371");
        let a = pubfile_download_args(
            "https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371",
            "ian",
            &dir,
            DEFAULT_MAX_DOWNLOADS,
        )
        .unwrap();
        assert_eq!(
            a,
            [
                "-app",
                "431960",
                "-pubfile",
                "2841928371",
                "-username",
                "ian",
                "-remember-password",
                "-dir",
                "/data/wallpapers/workshop/2841928371",
                "-max-downloads",
                "8"
            ]
        );
        assert!(pubfile_download_args("nope", "ian", &dir, 8).is_err());
    }

    #[test]
    fn bootstrap_args_match_spec() {
        let a = core_assets_bootstrap_args(
            "ian",
            Path::new("/data/assets_only.txt"),
            Path::new("/data/we_core_assets"),
        )
        .unwrap();
        assert_eq!(
            a,
            [
                "-app",
                "431960",
                "-depot",
                "431961",
                "-filelist",
                "/data/assets_only.txt",
                "-username",
                "ian",
                "-remember-password",
                "-dir",
                "/data/we_core_assets"
            ]
        );
    }

    #[test]
    fn filelist_written_and_bootstrap_detected() {
        let dir =
            std::env::temp_dir().join(format!("truongpaper-steam-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!core_assets_bootstrapped(&dir));
        let p = write_core_assets_filelist(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "regex:^assets/(shaders|materials|effects|models)/.*"
        );
        std::fs::create_dir_all(dir.join("assets").join("shaders")).unwrap();
        assert!(core_assets_bootstrapped(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
