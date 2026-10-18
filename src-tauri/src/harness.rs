//! Debug-only IPC surface for the agent UI harness (issue #7).
//!
//! The tauri-pilot plugin listens on a local Named Pipe in debug builds
//! and lets an agent drive the real app (`harness/desktop/README.md`).
//! Synthetic click packets can't answer the native pick dialogs, so the
//! harness needs another way to get files into the sharer: this module
//! creates a deterministic fixture tree (loose files plus a folder) in
//! the temp directory and routes it through the production share flow —
//! the same [`PendingShareFiles`] buffer and `share-files-requested`
//! event the OS-level context menu uses — so the UI observes a launch
//! with staged files and runs its own add/server/refresh code.
//!
//! Everything here is compiled out of release builds.
//!
//! ## Fixture layout
//!
//! ```text
//! mosquito-harness/desktop/
//!   beach-notes.txt          (loose)
//!   release-plan.md          (loose)
//!   Photographs/             (folder root → group "Photographs")
//!     sunrise.jpg
//!     travel/
//!       beach.jpg
//! ```
//!
//! The directory is wiped on each call, so seeding always yields the
//! same four rows in the same order.

use std::path::PathBuf;

use serde::Serialize;
use tauri::Emitter;

use crate::PendingShareFiles;

/// Deterministic fixture root under the OS temp directory.
fn fixture_root() -> PathBuf {
    std::env::temp_dir()
        .join("mosquito-harness")
        .join("desktop")
}

/// Recreate the fixture tree from scratch, returning the paths to
/// pseudo-drop into the app: loose files first so the desktop list leads
/// with them, then the folder root (members carry provenance, ADR-0001).
fn write_fixtures() -> std::io::Result<(PathBuf, Vec<String>)> {
    let root = fixture_root();
    let folder = root.join("Photographs");

    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(folder.join("travel"))?;

    std::fs::write(
        root.join("beach-notes.txt"),
        "beach day notes for the UI harness\n",
    )?;
    std::fs::write(
        root.join("release-plan.md"),
        "# Release plan\n\n- [ ] ship the UI harness\n",
    )?;
    std::fs::write(
        folder.join("sunrise.jpg"),
        b"\xFF\xD8\xFF\xE0 stand-in JPEG header bytes for the harness fixture\0",
    )?;
    std::fs::write(
        folder.join("travel").join("beach.jpg"),
        b"\xFF\xD8\xFF\xE0 another stand-in JPEG for the harness fixture\0",
    )?;

    let paths = vec![
        root.join("beach-notes.txt"),
        root.join("release-plan.md"),
        folder,
    ]
    .into_iter()
    .map(|path| path.to_string_lossy().into_owned())
    .collect();

    Ok((root, paths))
}

/// What the harness CLI gets back from a seed call.
#[derive(Debug, Serialize)]
pub struct SeedOutcome {
    fixture_root: String,
    /// Paths staged exactly as the pending-share buffer received them.
    shared_paths: Vec<String>,
}

/// Create fixtures, stage them in the pending-share buffer, and wake the
/// UI: the frontend drains the buffer through `add_paths`, which adds the
/// files (folder included) and starts the server — the full production
/// path, not a debug shortcut.
///
/// Best-effort staging: a write failure surfaces as an error string to
/// the CLI; a failed emit just leaves paths pending for the next drain
/// (the mounted app drains once more at startup).
#[tauri::command]
pub async fn harness_seed(
    app: tauri::AppHandle,
    pending: tauri::State<'_, PendingShareFiles>,
) -> Result<SeedOutcome, String> {
    let (root, paths) =
        write_fixtures().map_err(|error| format!("writing UI harness fixtures failed: {error}"))?;

    pending.append(paths.clone());
    let _ = app.emit("share-files-requested", ());

    Ok(SeedOutcome {
        fixture_root: root.to_string_lossy().into_owned(),
        shared_paths: paths,
    })
}

/// Sanity check for the fixture content: names are harness-authored
/// constants the scenarios and README rely on.
#[cfg(test)]
mod tests {
    use super::{fixture_root, write_fixtures};

    #[test]
    fn fixtures_are_wiped_before_each_seed() {
        let (root, _) = write_fixtures().expect("first seed");
        std::fs::write(root.join("stale-artifact.txt"), "left over").unwrap();

        let (root_after, paths) = write_fixtures().expect("second seed");

        assert_eq!(root_after, fixture_root());
        assert!(!root_after.join("stale-artifact.txt").exists());
        assert_eq!(
            paths,
            [
                root.join("beach-notes.txt"),
                root.join("release-plan.md"),
                root.join("Photographs"),
            ]
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
        );
        assert!(root_after
            .join("Photographs")
            .join("travel")
            .join("beach.jpg")
            .exists());
    }
}
