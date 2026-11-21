//! Where the clips live on disk.
//!
//! The default location is the Tauri app-data directory, but people with a small
//! system drive want the history elsewhere. The chosen location has to be known
//! *before* the database is opened, so it cannot live in the database itself: a
//! small pointer file in the default directory records it instead.
//!
//! Moving is a copy-then-restart operation. The live connection is used to produce
//! a consistent `VACUUM INTO` snapshot, and media files are copied alongside it.
//! A legacy `key.bin` is carried over if present so an unmigrated history can
//! still be decrypted from the new location; the old files are removed on the next
//! start — by then this process is gone and no handle is left open on them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::store::Store;

const POINTER_FILE: &str = "storage.json";
const DATABASE_FILE: &str = "copypaste.db";
const KEY_FILE: &str = "key.bin";
/// Files that belong to a Kept data directory, and nothing else.
const OWNED_FILES: [&str; 4] = [
    DATABASE_FILE,
    "copypaste.db-wal",
    "copypaste.db-shm",
    KEY_FILE,
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Pointer {
    /// `None` means the default directory.
    data_dir: Option<String>,
    /// A directory whose contents were copied out and can now be removed.
    previous_dir: Option<String>,
}

/// What the settings page shows about the current location.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    pub data_dir: String,
    pub default_dir: String,
    pub custom: bool,
    /// The sync service that owns this folder, when one does. A live SQLite
    /// database inside Dropbox or Google Drive is asking for trouble.
    pub cloud: Option<String>,
}

fn pointer_path(default_dir: &Path) -> PathBuf {
    default_dir.join(POINTER_FILE)
}

fn read_pointer(default_dir: &Path) -> Pointer {
    let path = pointer_path(default_dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Pointer::default();
    };
    serde_json::from_str(&raw).unwrap_or_else(|e| {
        log::warn!("ignoring unreadable {}: {e}", path.display());
        Pointer::default()
    })
}

fn write_pointer(default_dir: &Path, pointer: &Pointer) -> Result<()> {
    std::fs::create_dir_all(default_dir)?;
    let path = pointer_path(default_dir);
    std::fs::write(&path, serde_json::to_vec_pretty(pointer)?)?;
    Ok(())
}

/// The directory the store should open. A pointer to a directory that is no
/// longer there (an unplugged drive, a deleted folder) falls back to the default
/// rather than leaving the app unable to start.
pub fn active_dir(default_dir: &Path) -> PathBuf {
    let Some(custom) = read_pointer(default_dir).data_dir else {
        return default_dir.to_path_buf();
    };
    let candidate = PathBuf::from(custom);
    if candidate.join(DATABASE_FILE).is_file() {
        candidate
    } else {
        log::warn!(
            "data folder {} is unavailable; falling back to {}",
            candidate.display(),
            default_dir.display()
        );
        default_dir.to_path_buf()
    }
}

pub fn info(default_dir: &Path, active: &Path) -> StorageInfo {
    StorageInfo {
        data_dir: active.to_string_lossy().to_string(),
        default_dir: default_dir.to_string_lossy().to_string(),
        custom: active != default_dir,
        cloud: cloud_service(active).map(str::to_string),
    }
}

/// The sync service a folder belongs to, when one of them owns it.
///
/// Kept keeps a live SQLite database plus its media files in the data folder.
/// Sync clients copy and rewrite files behind the app's back — mid-transaction,
/// which is exactly when a database must not be touched — so a folder inside one
/// of these trees is worth a warning before anything is moved into it.
///
/// Detection is by the locations the clients actually use: Dropbox writes its
/// sync roots into `info.json`, and Google Drive appears either as a folder named
/// `Google Drive` or as the mounted `My Drive` of Drive for desktop.
pub fn cloud_service(path: &Path) -> Option<&'static str> {
    service_for(path, &dropbox_roots())
}

fn service_for(path: &Path, dropbox_roots: &[PathBuf]) -> Option<&'static str> {
    if dropbox_roots.iter().any(|root| path.starts_with(root)) {
        return Some("Dropbox");
    }
    // A `My Drive` mount belongs to Drive for desktop; the older client synced a
    // folder literally called `Google Drive`.
    let named = path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name.eq_ignore_ascii_case("Google Drive") || name.eq_ignore_ascii_case("My Drive")
    });
    if named {
        return Some("Google Drive");
    }
    None
}

/// Every folder Dropbox syncs, from the per-user `info.json` both clients write.
///
/// A missing or unreadable file is normal: it only exists on machines that run
/// Dropbox.
fn dropbox_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for base in [std::env::var_os("LOCALAPPDATA"), std::env::var_os("APPDATA")] {
        let Some(base) = base else { continue };
        let info = PathBuf::from(base).join("Dropbox").join("info.json");
        let Ok(raw) = std::fs::read_to_string(&info) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        for key in ["personal", "business"] {
            if let Some(root) = json.get(key).and_then(|entry| entry.get("path"))
                && let Some(root) = root.as_str()
                && !root.trim().is_empty()
            {
                roots.push(PathBuf::from(root));
            }
        }
    }
    // The client's default folder, for a machine whose info.json has gone.
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let fallback = PathBuf::from(home).join("Dropbox");
        if fallback.is_dir() && !roots.iter().any(|root| *root == fallback) {
            roots.push(fallback);
        }
    }
    roots
}

/// Removes the files left behind by a completed move. Runs at startup, after the
/// store at the new location is open, so the old files are no longer in use.
pub fn cleanup_previous(default_dir: &Path, active: &Path) {
    let pointer = read_pointer(default_dir);
    let Some(previous) = pointer.previous_dir else {
        return;
    };
    let previous = PathBuf::from(previous);

    if previous != active {
        for name in OWNED_FILES {
            let _ = std::fs::remove_file(previous.join(name));
        }
        let media = previous.join("media");
        if let Ok(entries) = std::fs::read_dir(&media) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        let _ = std::fs::remove_dir(&media);
        log::info!("removed the previous copy of the data in {}", previous.display());
    }

    // Keep the location, drop the marker so the sweep runs once.
    let updated = Pointer {
        data_dir: pointer.data_dir,
        previous_dir: None,
    };
    if let Err(e) = write_pointer(default_dir, &updated) {
        log::warn!("could not update the storage pointer: {e}");
    }
}

/// Copies the live data to `target` and points the app at it on the next start.
pub fn relocate(store: &Store, default_dir: &Path, target: &Path) -> Result<()> {
    let current = store.data_dir().to_path_buf();
    let target = target.to_path_buf();

    if target == current {
        return Err(AppError::msg("Kept already stores its clips there"));
    }
    if target.starts_with(&current) {
        return Err(AppError::msg(
            "choose a folder outside the current data folder",
        ));
    }
    if target.join(DATABASE_FILE).exists() {
        return Err(AppError::msg(format!(
            "{} already contains a Kept database",
            target.display()
        )));
    }

    std::fs::create_dir_all(target.join("media"))?;

    let outcome = copy_everything(store, &current, &target);
    if let Err(e) = outcome {
        rollback(&target);
        return Err(e);
    }

    let pointer = Pointer {
        data_dir: if target == default_dir {
            None
        } else {
            Some(target.to_string_lossy().to_string())
        },
        previous_dir: Some(current.to_string_lossy().to_string()),
    };
    if let Err(e) = write_pointer(default_dir, &pointer) {
        rollback(&target);
        return Err(e);
    }
    Ok(())
}

fn copy_everything(store: &Store, current: &Path, target: &Path) -> Result<()> {
    // A consistent snapshot even while the clipboard watcher is mid-write; the
    // caller stops the watcher before this runs.
    store.vacuum_into(&target.join(DATABASE_FILE))?;
    let key = current.join(KEY_FILE);
    if key.is_file() {
        std::fs::copy(&key, target.join(KEY_FILE))?;
    }
    copy_media(&current.join("media"), &target.join("media"))?;
    Ok(())
}

fn copy_media(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    let Ok(entries) = std::fs::read_dir(source) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let name = match path.file_name() {
                Some(name) => name,
                None => continue,
            };
            std::fs::copy(&path, target.join(name))?;
        }
    }
    Ok(())
}

/// Best-effort removal of a half-finished copy so the target can be retried.
fn rollback(target: &Path) {
    for name in OWNED_FILES {
        let _ = std::fs::remove_file(target.join(name));
    }
    let media = target.join("media");
    if let Ok(entries) = std::fs::read_dir(&media) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let _ = std::fs::remove_dir(&media);
}

/// Directory that belongs to one archive collection.
///
/// The name is reduced to characters that are legal in a Windows path, so a
/// collection called `Q3 / invoices` still gets a folder that Explorer can open.
pub fn archive_dir(data_dir: &Path, name: &str) -> PathBuf {
    let mut slug: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    slug = slug.trim().trim_matches('-').to_string();
    if slug.is_empty() {
        slug = "archive".into();
    }
    data_dir.join("archives").join(slug)
}

/// Creates the archive's folder and returns it.
pub fn ensure_archive_dir(data_dir: &Path, name: &str) -> Result<PathBuf> {
    let dir = archive_dir(data_dir, name);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Removes the archive's folder when nothing is left in it.
pub fn remove_archive_dir(data_dir: &Path, name: &str) {
    let dir = archive_dir(data_dir, name);
    let empty = std::fs::read_dir(&dir)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false);
    if empty {
        let _ = std::fs::remove_dir(&dir);
    }
    let _ = std::fs::remove_dir(data_dir.join("archives"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ClipContent, ClipFilter, NewClip};
    use crate::store::InsertOutcome;

    fn text_clip(body: &str) -> NewClip {
        NewClip {
            content: ClipContent::Text(body.to_string()),
            content_hash: crate::hashing::of_str(body),
            source_app: None,
            search_text: crate::search::normalize(body),
            preview: body.chars().take(80).collect(),
            html: None,
            rtf: None,
        }
    }

    /// Cloud-folder detection is driven by Windows paths; the Linux one is not
    /// written yet.
    #[cfg(windows)]
    #[test]
    fn a_sync_folder_is_named_and_a_plain_one_is_not() {
        let roots = [PathBuf::from(r"C:\Users\me\Dropbox")];
        assert_eq!(
            service_for(Path::new(r"C:\Users\me\Dropbox\Kept"), &roots),
            Some("Dropbox")
        );
        assert_eq!(
            service_for(Path::new(r"G:\My Drive\Kept"), &roots),
            Some("Google Drive")
        );
        assert_eq!(
            service_for(Path::new(r"C:\Users\me\Google Drive\Kept"), &roots),
            Some("Google Drive")
        );
        assert_eq!(
            service_for(Path::new(r"C:\Users\me\AppData\Local\Kept"), &roots),
            None,
            "an ordinary folder is not a sync folder"
        );
    }

    #[test]
    fn a_folder_that_merely_mentions_a_service_is_left_alone() {
        let roots = [PathBuf::from(r"C:\Users\me\Dropbox")];
        assert_eq!(
            service_for(Path::new(r"C:\Users\me\Dropbox-notes\Kept"), &roots),
            None,
            "a sibling with a similar name is not inside Dropbox"
        );
        assert_eq!(
            service_for(Path::new(r"C:\My Drive notes"), &roots),
            None
        );
    }

    #[test]
    fn relocate_copies_clips_and_points_the_app_at_them() {
        let (store, _dir) = crate::store::test_store();
        let id = match store.insert_clip(text_clip("relocate me")).unwrap() {
            InsertOutcome::Inserted(id) => id,
            _ => unreachable!(),
        };

        let default_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let new_home = target.path().join("KeptData");

        relocate(&store, default_dir.path(), &new_home).unwrap();

        let pointer = read_pointer(default_dir.path());
        assert_eq!(
            pointer.data_dir.as_deref(),
            Some(new_home.to_string_lossy().as_ref())
        );
        assert_eq!(active_dir(default_dir.path()), new_home);

        let moved = Store::open(&new_home).unwrap();
        let page = moved.list(&ClipFilter::default(), "", 10, 0);
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].id, id);
    }

    #[test]
    fn cleanup_removes_the_old_copy_and_the_marker() {
        let (store, _dir) = crate::store::test_store();
        store.insert_clip(text_clip("move me")).unwrap();
        let previous = store.data_dir().to_path_buf();

        let default_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let new_home = target.path().join("data");
        relocate(&store, default_dir.path(), &new_home).unwrap();

        // The real flow restarts first; the fixture has to let go of the files.
        drop(store);
        cleanup_previous(default_dir.path(), &new_home);

        assert!(!previous.join("copypaste.db").exists());
        assert!(!previous.join("key.bin").exists());
        let pointer = read_pointer(default_dir.path());
        assert!(pointer.previous_dir.is_none(), "the sweep runs once");
        assert_eq!(
            pointer.data_dir.as_deref(),
            Some(new_home.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn moving_back_to_the_default_location_clears_the_pointer() {
        let (store, _dir) = crate::store::test_store();
        store.insert_clip(text_clip("come home")).unwrap();

        let default_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let custom = target.path().join("elsewhere");
        relocate(&store, default_dir.path(), &custom).unwrap();
        drop(store);

        let moved = Store::open(&custom).unwrap();
        relocate(&moved, default_dir.path(), default_dir.path()).unwrap();

        let pointer = read_pointer(default_dir.path());
        assert!(
            pointer.data_dir.is_none(),
            "the default location needs no pointer"
        );
        assert_eq!(active_dir(default_dir.path()), default_dir.path());
    }

    #[test]
    fn refuses_a_folder_that_already_holds_a_database() {
        let (store, _dir) = crate::store::test_store();
        let default_dir = tempfile::tempdir().unwrap();
        let taken = tempfile::tempdir().unwrap();
        std::fs::write(taken.path().join("copypaste.db"), b"not ours").unwrap();

        assert!(relocate(&store, default_dir.path(), taken.path()).is_err());
    }

    #[test]
    fn missing_custom_folder_falls_back_to_the_default() {
        let default_dir = tempfile::tempdir().unwrap();
        write_pointer(
            default_dir.path(),
            &Pointer {
                data_dir: Some("Z:\\definitely\\not\\here".into()),
                previous_dir: None,
            },
        )
        .unwrap();
        assert_eq!(active_dir(default_dir.path()), default_dir.path());
    }
}
