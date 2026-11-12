//! `playlist_list` / `playlist_save` / `playlist_delete` (docs/04 §5.1).

use crate::commands::monitor::{apply_source, lock_monitor_apply};
use crate::commands::photo::PhotoServiceState;
use crate::db;
use crate::models::{Playlist, WallpaperSource};
use crate::state::AppState;
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn playlist_list(state: State<'_, AppState>) -> Result<Vec<Playlist>, String> {
    let conn = state.db.lock();
    db::list_playlists(&conn)
}

/// Creates (empty `playlistId`) or updates a playlist; several playlists may coexist.
#[tauri::command]
pub async fn playlist_save(
    state: State<'_, AppState>,
    playlist: Playlist,
) -> Result<Playlist, String> {
    let saved = {
        let conn = state.db.lock();
        db::save_playlist(&conn, playlist)?
    };
    crate::scheduler::invalidate_playlist_cache();
    Ok(saved)
}

/// Deletes a playlist; displays that used it fall back to the Photo API.
#[tauri::command]
pub async fn playlist_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    photo: State<'_, PhotoServiceState>,
    playlist_id: String,
) -> Result<(), String> {
    let affected = {
        let conn = state.db.lock();
        db::delete_playlist(&conn, &playlist_id)?
    };
    crate::scheduler::invalidate_playlist_cache();
    for monitor_id in affected {
        let res = {
            let _apply = lock_monitor_apply(&monitor_id).await;
            apply_source(
                &app,
                &state,
                &photo,
                &monitor_id,
                &WallpaperSource::PhotoApi,
            )
            .await
        };
        if let Err(err) = res {
            tracing::warn!("playlist_delete: cannot refresh {monitor_id}: {err}");
        }
        crate::scheduler::note_manual_rotation(Some(&monitor_id), true);
    }
    Ok(())
}
