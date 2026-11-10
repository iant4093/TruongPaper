//! `playlist_list` / `playlist_save` (docs/04 §5.1).

use crate::db;
use crate::models::Playlist;
use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub async fn playlist_list(state: State<'_, AppState>) -> Result<Vec<Playlist>, String> {
    let conn = state.db.lock();
    db::list_playlists(&conn)
}

#[tauri::command]
pub async fn playlist_save(
    state: State<'_, AppState>,
    playlist: Playlist,
) -> Result<Playlist, String> {
    let conn = state.db.lock();
    db::save_playlist(&conn, playlist)
}
