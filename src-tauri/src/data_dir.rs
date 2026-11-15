//! Moving TruongPaper's data folder (Windows: Roaming -> Local AppData).
//!
//! Everything in the old folder is renamed into the new one. Renames on one drive are instant;
//! if any fails, the ones already done are moved back and the old folder stays in use, so a
//! half-moved library never happens. Absolute paths stored in the database are then rewritten.

use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

/// Picks the data folder: `new_dir` when it already has the database or nothing exists yet,
/// otherwise moves `old_dir`'s contents into `new_dir`. Returns `(folder, Some(old))` when a
/// move happened.
pub fn choose_and_migrate(
    old_dir: &Path,
    new_dir: &Path,
    db_name: &str,
) -> (PathBuf, Option<PathBuf>) {
    if new_dir.join(db_name).is_file() || !old_dir.join(db_name).is_file() {
        return (new_dir.to_path_buf(), None);
    }
    match move_contents(old_dir, new_dir, db_name) {
        Ok(()) => (new_dir.to_path_buf(), Some(old_dir.to_path_buf())),
        Err(err) => {
            tracing::warn!(
                "could not move app data from {} to {}: {err}; using the old folder",
                old_dir.display(),
                new_dir.display()
            );
            (old_dir.to_path_buf(), None)
        }
    }
}

/// Renames every entry of `old_dir` into `new_dir`, database files last (so it is the final
/// step that commits the move). Entries that already exist in `new_dir` (Tauri's own `EBWebView`
/// lives there) are left where they are. Rolls back on failure.
fn move_contents(old_dir: &Path, new_dir: &Path, db_name: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(new_dir)?;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(old_dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    let is_db = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(db_name))
    };
    entries.sort_by_key(|p| is_db(p));
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    for from in entries {
        let Some(name) = from.file_name() else {
            continue;
        };
        let to = new_dir.join(name);
        if to.exists() {
            if is_db(&from) {
                // Cannot happen (checked by the caller), but never overwrite a database.
                rollback(&done);
                return Err(std::io::Error::other(format!(
                    "{} already exists",
                    to.display()
                )));
            }
            continue;
        }
        if let Err(err) = std::fs::rename(&from, &to) {
            rollback(&done);
            return Err(std::io::Error::new(
                err.kind(),
                format!("{}: {err}", from.display()),
            ));
        }
        done.push((from, to));
    }
    Ok(())
}

fn rollback(done: &[(PathBuf, PathBuf)]) {
    for (from, to) in done.iter().rev() {
        let _ = std::fs::rename(to, from);
    }
}

/// Points stored absolute paths that start with `old_dir` at `new_dir` (case-insensitive, as
/// Windows paths are). Returns the number of rows changed.
pub fn rewrite_moved_paths(
    conn: &Connection,
    old_dir: &Path,
    new_dir: &Path,
) -> rusqlite::Result<usize> {
    let old = old_dir
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_string();
    let new = new_dir
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_string();
    let mut changed = 0;
    for column in ["install_dir", "preview_path"] {
        changed += conn.execute(
            &format!(
                "UPDATE wallpaper_configs
                 SET {column} = ?2 || substr({column}, length(?1) + 1)
                 WHERE lower(substr({column}, 1, length(?1))) = lower(?1)
                   AND (length({column}) = length(?1)
                        OR substr({column}, length(?1) + 1, 1) IN ('\\', '/'))"
            ),
            params![old, new],
        )?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tp-datadir-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn moves_everything_once_and_keeps_existing_entries() {
        let root = temp("move");
        let (old, new) = (root.join("Roaming"), root.join("Local"));
        std::fs::create_dir_all(old.join("wallpapers/workshop/1")).unwrap();
        std::fs::write(old.join("wallpapers/workshop/1/project.json"), "{}").unwrap();
        std::fs::write(old.join("truongpaper.db"), "db").unwrap();
        std::fs::write(old.join("truongpaper.db-wal"), "wal").unwrap();
        std::fs::create_dir_all(new.join("EBWebView")).unwrap();

        let (dir, moved) = choose_and_migrate(&old, &new, "truongpaper.db");
        assert_eq!(dir, new);
        assert_eq!(moved.as_deref(), Some(old.as_path()));
        assert!(new.join("wallpapers/workshop/1/project.json").is_file());
        assert!(new.join("truongpaper.db").is_file());
        assert!(new.join("truongpaper.db-wal").is_file());
        assert!(new.join("EBWebView").is_dir());
        assert!(!old.join("truongpaper.db").exists());

        // Second launch: nothing to move.
        assert_eq!(
            choose_and_migrate(&old, &new, "truongpaper.db"),
            (new.clone(), None)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fresh_install_uses_the_new_folder() {
        let root = temp("fresh");
        let (old, new) = (root.join("Roaming"), root.join("Local"));
        assert_eq!(
            choose_and_migrate(&old, &new, "truongpaper.db"),
            (new, None)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stored_paths_follow_the_move() {
        let conn = crate::state::test_db();
        let old = r"C:\Users\me\AppData\Roaming\com.iantruong.truongpaper";
        let new = r"C:\Users\me\AppData\Local\com.iantruong.truongpaper";
        for (id, dir) in [
            ("a", format!(r"{old}\wallpapers\workshop\1")),
            (
                "b",
                r"D:\SteamLibrary\steamapps\workshop\content\431960\2".to_string(),
            ),
            ("c", format!(r"{old}-other\x")),
        ] {
            // Case differs from `old` on purpose: Windows paths ignore case.
            let dir = dir.replace("Roaming", "roaming");
            conn.execute(
                "INSERT INTO wallpaper_configs
                     (wallpaper_id, title, wallpaper_type, install_dir, preview_path, updated_at)
                 VALUES (?1, 't', 'scene', ?2, ?2 || '\\preview.gif', 0)",
                params![id, dir],
            )
            .unwrap();
        }
        let n = rewrite_moved_paths(&conn, Path::new(old), Path::new(new)).unwrap();
        // install_dir and preview_path of row "a".
        assert_eq!(n, 2);
        let get = |id: &str| -> (String, String) {
            conn.query_row(
                "SELECT install_dir, preview_path FROM wallpaper_configs WHERE wallpaper_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        assert!(get("a").0.starts_with(new), "{:?}", get("a"));
        assert!(get("a").1.ends_with(r"\preview.gif"));
        assert!(get("b").0.starts_with(r"D:\"));
        // A sibling folder that only shares the prefix is untouched.
        assert!(get("c").0.contains("-other"), "{:?}", get("c"));
    }
}
