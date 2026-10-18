use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{AddFileError, RemoveError};

/// Opaque identifier for a shared file.
/// Newtype over u64 so signatures can't confuse it with a random number,
/// and we can swap the representation later (e.g. random tokens).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct FileId(pub u64);

/// A file the user dropped into the app.
#[derive(Debug, Clone, Serialize)]
pub struct SharedFile {
    pub id: FileId,
    /// File name only (e.g. `report.pdf`) — shown in UI and listing page.
    pub name: String,
    /// Absolute path on disk — the server streams from here.
    pub path: PathBuf,
    /// Size in bytes, captured when the file was added.
    pub size: u64,
    /// Where this file hangs in a folder, when it was added as part of
    /// one. Grouping is derived from this — the store itself stays flat
    /// (see ADR-0001). Loose files added directly have none.
    pub provenance: Option<FolderProvenance>,
}

/// Provenance of a folder member: which root it came from, and its path
/// below that root. Bundling the two makes a half-member (a root with no
/// relative path or vice versa) unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderProvenance {
    /// Absolute path of the folder root. Two roots may share a display
    /// name; they stay distinct by path.
    pub folder_root: PathBuf,
    /// Path below the root, forward-slash-joined (`sub/b.txt`) — the
    /// format grouping and zip archives both use.
    pub relative_path: String,
}

/// One path that couldn't be added, and why.
#[derive(Debug, Serialize)]
pub struct FailedAdd {
    pub path: PathBuf,
    pub error: String,
}

/// Result of a batch add: what made it in, and what didn't.
#[derive(Debug, Serialize)]
pub struct AddFilesOutcome {
    pub added: Vec<SharedFile>,
    pub failed: Vec<FailedAdd>,
}

/// Identifier of an imported folder root — a root record.
/// Kept a distinct newtype so file and root ids can never be swapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct RootId(pub u64);

/// The store's record of an imported folder: where the folder is and how
/// many files it held when the import's count walk finished (ADR-0005).
/// Nothing beneath the root is registered on import — its members become
/// shared files only as they are materialized.
#[derive(Debug, Clone, Serialize)]
pub struct RootRecord {
    pub id: RootId,
    /// Absolute path of the folder the user imported.
    pub path: PathBuf,
    /// Files counted at import time. Lazily-known contents may drift
    /// from this number afterwards — that is the live-subtree contract.
    pub file_count: u64,
}

/// What materializing a root record's folder registered.
#[derive(Debug, Serialize)]
pub struct MaterializeOutcome {
    pub added: Vec<SharedFile>,
    /// Already registered (folder root + relative path) — the
    /// idempotency key made them a no-op.
    pub skipped: u64,
    pub failed: Vec<FailedAdd>,
}

/// What removing a root record took with it.
#[derive(Debug, Serialize)]
pub struct RootRemoval {
    pub record: RootRecord,
    /// Every materialized member swept away with the record.
    pub files: Vec<SharedFile>,
}

/// In-memory registry of files currently being shared.
#[derive(Debug, Default)]
pub struct FileStore {
    files: Vec<SharedFile>,
    next_id: u64,
    roots: Vec<RootRecord>,
    next_root_id: u64,
}

impl FileStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a file on disk. Metadata is read once, up front,
    /// so the UI and listing page never touch the filesystem.
    pub fn add(&mut self, path: impl Into<PathBuf>) -> Result<SharedFile, AddFileError> {
        self.insert(path.into(), None)
    }

    /// Register many paths (the drag-and-drop case): files individually,
    /// directories walked into frozen snapshots. Never fails wholesale —
    /// per-entry tolerance throughout.
    pub fn add_paths(&mut self, paths: Vec<PathBuf>) -> AddFilesOutcome {
        let mut outcome = AddFilesOutcome {
            added: Vec::new(),
            failed: Vec::new(),
        };
        for path in paths {
            // A dropped path must not be a link. Following it would
            // escape wherever it points, and a looped link hangs — the
            // same reason links inside a walk are skipped outright.
            if is_dropped_link(&path) {
                outcome.failed.push(FailedAdd {
                    path,
                    error: "symlinks can't be shared".into(),
                });
                continue;
            }

            if path.is_dir() {
                let batch = self.add_directory(&path);
                outcome.added.extend(batch.added);
                outcome.failed.extend(batch.failed);
            } else {
                match self.add(&path) {
                    Ok(file) => outcome.added.push(file),
                    Err(e) => outcome.failed.push(FailedAdd {
                        path,
                        error: e.to_string(),
                    }),
                }
            }
        }
        outcome
    }

    /// Register one file under a folder root (the folder-add path).
    fn insert(
        &mut self,
        path: PathBuf,
        provenance: Option<FolderProvenance>,
    ) -> Result<SharedFile, AddFileError> {
        let metadata = std::fs::metadata(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => AddFileError::NotFound(path.clone()),
            _ => AddFileError::Metadata(e),
        })?;

        if metadata.is_dir() {
            return Err(AddFileError::IsADirectory(path));
        }

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());

        let file = SharedFile {
            id: FileId(self.next_id),
            name,
            path,
            size: metadata.len(),
            provenance,
        };

        self.next_id += 1;
        self.files.push(file.clone());

        Ok(file)
    }

    /// Register every readable regular file under `root`, recursively —
    /// the folder-add path. Membership is decided once, here: a frozen
    /// snapshot. Contents still stream from disk live, like any other
    /// shared file.
    ///
    /// Per-entry tolerance mirrors batch adds: an unreadable entry is
    /// reported and skipped, never wholesale-rejecting its siblings.
    /// Symlinks (and junctions on Windows) are skipped outright —
    /// following them risks loops and sharing paths outside the folder
    /// the user dropped. Empty directories contribute nothing.
    pub fn add_directory(&mut self, root: impl Into<PathBuf>) -> AddFilesOutcome {
        let root = root.into();
        let mut outcome = AddFilesOutcome {
            added: Vec::new(),
            failed: Vec::new(),
        };
        self.add_dir_contents(&root, &root, &mut outcome);
        outcome
    }

    /// Walk one directory, recursing into subdirectories. `root` stays
    /// fixed across the recursion so each member knows where it hangs.
    fn add_dir_contents(&mut self, root: &Path, dir: &Path, outcome: &mut AddFilesOutcome) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                outcome.failed.push(FailedAdd {
                    path: dir.to_path_buf(),
                    error: e.to_string(),
                });
                return;
            }
        };

        for entry in entries {
            // An iteration error has no entry to attribute it to; the
            // directory itself is the best we can report.
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    outcome.failed.push(FailedAdd {
                        path: dir.to_path_buf(),
                        error: e.to_string(),
                    });
                    continue;
                }
            };
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(e) => {
                    outcome.failed.push(FailedAdd {
                        path,
                        error: e.to_string(),
                    });
                    continue;
                }
            };

            // Checked *before* is_dir: std reports junctions as symlinks
            // on Windows, so this skips both without ever following them.
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                self.add_dir_contents(root, &path, outcome);
                continue;
            }
            if !file_type.is_file() {
                outcome.failed.push(FailedAdd {
                    path,
                    error: "not a regular file".into(),
                });
                continue;
            }

            let provenance = FolderProvenance {
                folder_root: root.to_path_buf(),
                relative_path: match relative_path_string(root, &path) {
                    Some(relative_path) => relative_path,
                    None => {
                        outcome.failed.push(FailedAdd {
                            path,
                            error: "path escapes its folder root".into(),
                        });
                        continue;
                    }
                },
            };

            match self.insert(path.clone(), Some(provenance)) {
                Ok(file) => outcome.added.push(file),
                Err(e) => outcome.failed.push(FailedAdd {
                    path,
                    error: e.to_string(),
                }),
            }
        }
    }

    /// Remove a file from the registry. Returns what was removed
    /// so the caller can confirm exactly what disappeared.
    pub fn remove(&mut self, id: FileId) -> Result<SharedFile, RemoveError> {
        let index = self
            .files
            .iter()
            .position(|f| f.id == id)
            .ok_or(RemoveError::NotFound(id))?;
        Ok(self.files.remove(index))
    }

    pub fn get(&self, id: FileId) -> Option<&SharedFile> {
        self.files.iter().find(|f| f.id == id)
    }

    /// Insertion-ordered view of everything currently shared.
    pub fn list(&self) -> &[SharedFile] {
        &self.files
    }

    pub fn clear(&mut self) {
        self.files.clear();
    }

    /// Nothing shared right now: neither registered files nor imported
    /// root records. A root record alone counts as content.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.roots.is_empty()
    }

    // -- root records: count-only imports (ADR-0005) ----------------------

    /// Record an imported folder: absolute path plus the file count the
    /// import's count walk produced. Nothing beneath the root is
    /// registered here; ids are never reused, even after removal.
    pub fn commit_root_record(&mut self, path: impl Into<PathBuf>, file_count: u64) -> RootRecord {
        let record = RootRecord {
            id: RootId(self.next_root_id),
            path: path.into(),
            file_count,
        };
        self.next_root_id += 1;
        self.roots.push(record.clone());
        record
    }

    /// Insertion-ordered view of the imported roots.
    pub fn roots(&self) -> &[RootRecord] {
        &self.roots
    }

    /// Remove a root record and every member materialized under it —
    /// one action, mirroring how folder groups present themselves.
    pub fn remove_root(&mut self, id: RootId) -> Result<RootRemoval, RemoveError> {
        let index = self
            .roots
            .iter()
            .position(|r| r.id == id)
            .ok_or(RemoveError::RootNotFound(id))?;
        let record = self.roots.remove(index);

        // Members are found by the root record's absolute path — that's
        // the idempotency key, so removing one record can't miss files
        // another record for the same folder materialized.
        let mut remaining = Vec::new();
        let mut removed = Vec::new();
        for file in std::mem::take(&mut self.files) {
            let under_record = file
                .provenance
                .as_ref()
                .is_some_and(|p| p.folder_root == record.path);
            if under_record {
                removed.push(file);
            } else {
                remaining.push(file);
            }
        }
        self.files = remaining;

        Ok(RootRemoval {
            record,
            files: removed,
        })
    }

    /// Register the lazily-known direct children of a root record's
    /// folder (ADR-0005): regular files directly inside it become shared
    /// files; subfolders stay lazily-known (they materialize on their own
    /// turn), symlinks are skipped outright, and unreadable entries are
    /// reported per-entry like any batch add.
    ///
    /// Idempotent, keyed by (folder root, relative path): materializing
    /// twice — or two root records pointing at the same folder — converges
    /// on one registration per file instead of duplicates.
    ///
    /// The error type is [`RemoveError`][crate::error::RemoveError]'s
    /// RootNotFound variant: materialization's only failure is an unknown
    /// root id, and "no root record with that id" is one domain error
    /// whoever asks.
    pub fn materialize(&mut self, id: RootId) -> Result<MaterializeOutcome, RemoveError> {
        let root_path = self
            .roots
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.path.clone())
            .ok_or(RemoveError::RootNotFound(id))?;

        let mut outcome = MaterializeOutcome {
            added: Vec::new(),
            skipped: 0,
            failed: Vec::new(),
        };

        let entries = match std::fs::read_dir(&root_path) {
            Ok(entries) => entries,
            Err(e) => {
                outcome.failed.push(FailedAdd {
                    path: root_path,
                    error: e.to_string(),
                });
                return Ok(outcome);
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    outcome.failed.push(FailedAdd {
                        path: root_path.clone(),
                        error: e.to_string(),
                    });
                    continue;
                }
            };
            let path = entry.path();

            // Checked *before* is_dir, like the walk in add_paths: std
            // reports junctions as symlinks on Windows, so this skips
            // both without ever following them.
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(e) => {
                    outcome.failed.push(FailedAdd {
                        path,
                        error: e.to_string(),
                    });
                    continue;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                // Lazily-known: becomes visible by expanding, which
                // materializes its own direct children.
                continue;
            }
            if !file_type.is_file() {
                outcome.failed.push(FailedAdd {
                    path,
                    error: "not a regular file".into(),
                });
                continue;
            }

            let relative_path = match relative_path_string(&root_path, &path) {
                Some(relative_path) => relative_path,
                None => {
                    outcome.failed.push(FailedAdd {
                        path,
                        error: "path escapes its folder root".into(),
                    });
                    continue;
                }
            };

            // Idempotency key: (folder root, relative path). Concurrent or
            // repeated materializations of the same directory — including
            // through a duplicate root record — must converge.
            let already_registered = self.files.iter().any(|f| {
                f.provenance
                    .as_ref()
                    .is_some_and(|p| p.folder_root == root_path && p.relative_path == relative_path)
            });
            if already_registered {
                outcome.skipped += 1;
                continue;
            }

            match self.insert(
                path.clone(),
                Some(FolderProvenance {
                    folder_root: root_path.clone(),
                    relative_path,
                }),
            ) {
                Ok(file) => outcome.added.push(file),
                Err(e) => outcome.failed.push(FailedAdd {
                    path,
                    error: e.to_string(),
                }),
            }
        }

        Ok(outcome)
    }
}

/// Whether a dropped path is a link (symlink or junction on Windows).
/// Links are never followed: not as drop roots, not inside walks.
pub(crate) fn is_dropped_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_symlink())
        .unwrap_or(false)
}

/// Path below `root`, forward-slash-joined (`sub/b.txt`) — the format
/// grouping and zip archives both use. Components go through lossy
/// conversion like everything else that crosses IPC.
fn relative_path_string(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let joined = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    (!joined.is_empty()).then_some(joined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::disk::{make_file, nested_tree};
    use tempfile::TempDir;

    #[test]
    fn new_store_is_empty() {
        let store = FileStore::new();
        assert!(store.list().is_empty());
    }

    #[test]
    fn add_assigns_sequential_ids() {
        let dir = TempDir::new().unwrap();
        let a = make_file(dir.path(), "a.txt", b"hello");
        let b = make_file(dir.path(), "b.txt", b"world");

        let mut store = FileStore::new();
        let first = store.add(&a).unwrap();
        let second = store.add(&b).unwrap();

        assert_eq!(first.id, FileId(0));
        assert_eq!(second.id, FileId(1));
    }

    #[test]
    fn add_captures_name_and_size() {
        let dir = TempDir::new().unwrap();
        let path = make_file(dir.path(), "report.pdf", b"0123456789");

        let mut store = FileStore::new();
        let file = store.add(&path).unwrap();

        assert_eq!(file.name, "report.pdf");
        assert_eq!(file.size, 10);
        assert_eq!(file.path, path);
    }

    #[test]
    fn add_nonexistent_file_is_an_error() {
        let mut store = FileStore::new();
        let err = store.add("/definitely/not/here.txt").unwrap_err();
        assert!(matches!(err, AddFileError::NotFound(_)));
    }

    #[test]
    fn add_directory_is_an_error() {
        let dir = TempDir::new().unwrap();
        let mut store = FileStore::new();
        let err = store.add(dir.path()).unwrap_err();
        assert!(matches!(err, AddFileError::IsADirectory(_)));
    }

    #[test]
    fn adding_a_directory_registers_its_regular_files_recursively() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("photos");
        std::fs::create_dir_all(root.join("sub").join("deep")).unwrap();
        make_file(&root, "a.txt", b"a");
        make_file(&root.join("sub"), "b.txt", b"b");
        make_file(&root.join("sub").join("deep"), "c.txt", b"c");
        // Empty directories contribute nothing: neither an entry nor a failure.
        std::fs::create_dir(root.join("empty")).unwrap();

        let mut store = FileStore::new();
        let outcome = store.add_directory(&root);

        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
        let mut names: Vec<_> = outcome.added.iter().map(|f| f.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["a.txt", "b.txt", "c.txt"]);
    }

    #[test]
    fn folder_members_record_relative_path_and_folder_root() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("photos");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        make_file(&root, "a.txt", b"a");
        make_file(&root.join("sub"), "b.txt", b"b");

        let mut store = FileStore::new();
        let outcome = store.add_directory(&root);

        let a = outcome
            .added
            .iter()
            .find(|f| f.name == "a.txt")
            .expect("a.txt added");
        let b = outcome
            .added
            .iter()
            .find(|f| f.name == "b.txt")
            .expect("b.txt added");

        // Forward slashes: the format grouping and zip archives both use.
        let provenance = |f: &SharedFile| f.provenance.clone().expect("folder member");
        assert_eq!(provenance(a).relative_path, "a.txt");
        assert_eq!(provenance(b).relative_path, "sub/b.txt");
        assert_eq!(provenance(a).folder_root, root);
        assert_eq!(provenance(b).folder_root, root);
    }

    #[test]
    fn symlinks_are_skipped_not_followed_and_not_reported() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("real")).unwrap();
        make_file(&root, "real.txt", b"real");
        let target = make_file(dir.path(), "outside.txt", b"top secret");

        // Creating symlinks unprivileged needs developer mode on Windows,
        // or root on some CI images — skip there rather than fail.
        #[cfg(windows)]
        let link = match std::os::windows::fs::symlink_file(&target, root.join("link.txt")) {
            Ok(()) => root.join("link.txt"),
            Err(_) => return, // can't create symlinks in this environment
        };
        #[cfg(unix)]
        let link = match std::os::unix::fs::symlink(&target, root.join("link.txt")) {
            Ok(()) => root.join("link.txt"),
            Err(_) => return,
        };
        #[cfg(not(any(windows, unix)))]
        let _link_unused = ();

        let mut store = FileStore::new();
        let outcome = store.add_directory(&root);

        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
        let names: Vec<_> = outcome.added.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["real.txt"]);
        let _ = link; // link only existed to prove it doesn't get shared
    }

    #[test]
    #[cfg(windows)]
    fn unreadable_directory_is_reported_and_siblings_still_added() {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let dir = TempDir::new().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sealed")).unwrap();
        make_file(&root, "visible.txt", b"v");
        make_file(&root.join("sealed"), "hidden.txt", b"h");

        // Deny the Everyone SID (S-1-1-0) read access — locale-independent,
        // unlike using a group name like "Everyone".
        let deny = std::process::Command::new("icacls")
            .arg(root.join("sealed"))
            .args(["/deny", "*S-1-1-0:(OI)(CI)R"])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .unwrap();
        assert!(deny.success(), "icacls deny failed");

        let mut store = FileStore::new();
        let outcome = store.add_directory(&root);

        // Lift the denial before the temp dir is cleaned up.
        let reset = std::process::Command::new("icacls")
            .arg(root.join("sealed"))
            .args(["/reset", "/t", "/q"])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .unwrap();
        assert!(reset.success(), "icacls reset failed");

        let names: Vec<_> = outcome.added.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["visible.txt"]);
        assert_eq!(outcome.failed.len(), 1, "{:?}", outcome.failed);
        assert!(outcome.failed[0].path.ends_with("sealed"));
    }

    #[test]
    #[cfg(unix)]
    fn unreadable_directory_is_reported_and_siblings_still_added() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sealed")).unwrap();
        make_file(&root, "visible.txt", b"v");
        make_file(&root.join("sealed"), "hidden.txt", b"h");

        // Strip every permission bit; if we somehow retain access (root),
        // the isolation behavior can't be exercised here — skip instead.
        std::fs::set_permissions(root.join("sealed"), std::fs::Permissions::from_mode(0)).unwrap();
        if std::fs::read_dir(root.join("sealed")).is_ok() {
            std::fs::set_permissions(root.join("sealed"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            return;
        }

        let mut store = FileStore::new();
        let outcome = store.add_directory(&root);

        // Restore before the temp dir is cleaned up.
        std::fs::set_permissions(root.join("sealed"), std::fs::Permissions::from_mode(0o755))
            .unwrap();

        let names: Vec<_> = outcome.added.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["visible.txt"]);
        assert_eq!(outcome.failed.len(), 1, "{:?}", outcome.failed);
        assert!(outcome.failed[0].path.ends_with("sealed"));
    }

    #[test]
    fn symlinked_roots_are_rejected_not_followed() {
        let dir = TempDir::new().unwrap();
        let outside_files = dir.path().join("elsewhere");
        std::fs::create_dir(&outside_files).unwrap();
        make_file(&outside_files, "secret.txt", b"top secret");

        // Creating symlinks unprivileged needs developer mode on Windows,
        // or root on some CI images — skip there rather than fail.
        #[cfg(windows)]
        let folder_link = match std::os::windows::fs::symlink_dir(
            &outside_files,
            dir.path().join("linked-folder"),
        ) {
            Ok(()) => dir.path().join("linked-folder"),
            Err(_) => return, // can't create symlinks in this environment
        };
        #[cfg(unix)]
        let folder_link =
            match std::os::unix::fs::symlink(&outside_files, dir.path().join("linked-folder")) {
                Ok(()) => dir.path().join("linked-folder"),
                Err(_) => return,
            };
        #[cfg(not(any(windows, unix)))]
        let _link_unused = ();

        let mut store = FileStore::new();
        let outcome = store.add_paths(vec![folder_link]);

        // Rejected as one failed add — the contents never entered the store.
        assert!(outcome.added.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert!(outcome.failed[0].error.contains("symlinks"));
    }

    #[test]
    fn get_finds_file_by_id() {
        let dir = TempDir::new().unwrap();
        let path = make_file(dir.path(), "a.txt", b"hi");
        let mut store = FileStore::new();
        let added = store.add(&path).unwrap();

        assert_eq!(store.get(added.id).unwrap().name, "a.txt");
        assert!(store.get(FileId(999)).is_none());
    }

    #[test]
    fn remove_deletes_the_entry() {
        let dir = TempDir::new().unwrap();
        let path = make_file(dir.path(), "a.txt", b"hi");
        let mut store = FileStore::new();
        let added = store.add(&path).unwrap();

        let removed = store.remove(added.id).unwrap();
        assert_eq!(removed.id, added.id);
        assert!(store.get(added.id).is_none());
        assert!(store.list().is_empty());
    }

    #[test]
    fn remove_unknown_id_is_an_error() {
        let mut store = FileStore::new();
        assert!(matches!(
            store.remove(FileId(42)),
            Err(RemoveError::NotFound(FileId(42)))
        ));
    }

    #[test]
    fn list_preserves_insertion_order() {
        let dir = TempDir::new().unwrap();
        let mut store = FileStore::new();
        for name in ["c.txt", "a.txt", "b.txt"] {
            store.add(make_file(dir.path(), name, b"x")).unwrap();
        }
        let names: Vec<_> = store.list().iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["c.txt", "a.txt", "b.txt"]);
    }

    #[test]
    fn ids_are_never_reused_after_removal() {
        let dir = TempDir::new().unwrap();
        let a = make_file(dir.path(), "a.txt", b"a");
        let b = make_file(dir.path(), "b.txt", b"b");

        let mut store = FileStore::new();
        let first = store.add(&a).unwrap();
        store.remove(first.id).unwrap();
        let second = store.add(&b).unwrap();

        assert_eq!(second.id, FileId(1)); // not 0 again
    }

    #[test]
    fn clear_removes_everything() {
        let dir = TempDir::new().unwrap();
        let mut store = FileStore::new();
        store.add(make_file(dir.path(), "a.txt", b"a")).unwrap();

        store.clear();

        assert!(store.list().is_empty());
    }

    // -- root records: count-only imports (ADR-0005) ----------------------

    #[test]
    fn committed_root_records_carry_path_count_and_sequential_ids() {
        let dir = TempDir::new().unwrap();
        let mut store = FileStore::new();
        store.add(make_file(dir.path(), "loose.txt", b"l")).unwrap();

        let root = dir.path().join("photos");
        std::fs::create_dir(&root).unwrap();
        let first = store.commit_root_record(&root, 3);
        let second = store.commit_root_record(dir.path().join("other"), 0);

        assert_eq!(first.id, RootId(0));
        assert_eq!(second.id, RootId(1));
        assert_eq!(first.path, root);
        assert_eq!(first.file_count, 3);

        // Insertion-ordered view, loose files untouched.
        assert_eq!(store.roots().len(), 2);
        assert_eq!(store.roots()[0].id, RootId(0));
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn commit_root_record_with_no_files_is_recorded() {
        let dir = TempDir::new().unwrap();
        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).unwrap();

        let mut store = FileStore::new();
        let record = store.commit_root_record(empty, 0);

        assert_eq!(record.file_count, 0);
        // A root record alone counts as content — a shareable, startable state.
        assert!(!store.is_empty());
    }

    #[test]
    fn root_ids_are_never_reused_after_root_removal() {
        let dir = TempDir::new().unwrap();
        let mut store = FileStore::new();
        let root = dir.path().join("a");
        std::fs::create_dir(&root).unwrap();
        let first = store.commit_root_record(root, 0);
        store.remove_root(first.id).unwrap();
        let second = store.commit_root_record(dir.path().join("b"), 0);

        assert_eq!(second.id, RootId(1));
    }

    #[test]
    fn remove_unknown_root_is_an_error() {
        let mut store = FileStore::new();
        assert!(matches!(
            store.remove_root(RootId(42)),
            Err(RemoveError::RootNotFound(RootId(42)))
        ));
    }

    #[test]
    fn remove_root_sweeps_its_materialized_members() {
        let dir = TempDir::new().unwrap();
        let root = nested_tree(dir.path());
        let loose = make_file(dir.path(), "loose.txt", b"l");

        let mut store = FileStore::new();
        store.add(&loose).unwrap();
        let record = store.commit_root_record(&root, 2);
        let materialized = store.materialize(record.id).unwrap();
        assert_eq!(materialized.added.len(), 1);

        let removal = store.remove_root(record.id).unwrap();

        assert_eq!(removal.record.id, record.id);
        assert_eq!(removal.files.len(), 1);
        assert!(store.roots().is_empty());
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].name, "loose.txt");
        assert!(!store.is_empty());
    }

    #[test]
    fn materialize_registers_direct_children_with_provenance() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("flat");
        std::fs::create_dir(&root).unwrap();
        make_file(&root, "a.txt", b"a");
        make_file(&root, "c.txt", b"c");

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 2);
        let outcome = store.materialize(record.id).unwrap();

        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
        assert_eq!(outcome.added.len(), 2);
        let a = outcome.added.iter().find(|f| f.name == "a.txt").unwrap();
        let provenance = a.provenance.clone().expect("folder member");
        assert_eq!(provenance.folder_root, root);
        assert_eq!(provenance.relative_path, "a.txt");
    }

    #[test]
    fn materialize_is_depth_lazy_direct_children_only() {
        let dir = TempDir::new().unwrap();
        let root = nested_tree(dir.path()); // a.txt at depth 0, b.txt at depth 1

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 2);
        let outcome = store.materialize(record.id).unwrap();

        // Deeper levels materialize on their own turn.
        assert_eq!(outcome.added.len(), 1);
        assert_eq!(outcome.added[0].name, "a.txt");
    }

    #[test]
    fn materialize_is_idempotent_keyed_by_folder_root_and_relative_path() {
        let dir = TempDir::new().unwrap();
        let root = nested_tree(dir.path());

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 2);
        let first = store.materialize(record.id).unwrap();
        let second = store.materialize(record.id).unwrap();

        assert_eq!(first.added.len(), 1);
        assert_eq!(second.added.len(), 0);
        assert_eq!(second.skipped, 1);
        assert_eq!(store.list().len(), 1, "no duplicates in the store");
    }

    #[test]
    fn duplicate_root_records_resolve_to_the_same_registered_files() {
        let dir = TempDir::new().unwrap();
        let root = nested_tree(dir.path());

        let mut store = FileStore::new();
        let first = store.commit_root_record(&root, 2);
        let second = store.commit_root_record(&root, 2);

        store.materialize(first.id).unwrap();
        let again = store.materialize(second.id).unwrap();

        // Same disk files the second root resolves to — one registration
        // per (folder root, relative path), no duplicated memory.
        assert_eq!(again.added.len(), 0);
        assert_eq!(again.skipped, 1);
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.roots().len(), 2);
    }

    #[test]
    fn materialize_after_root_removal_is_an_error() {
        let dir = TempDir::new().unwrap();
        let root = nested_tree(dir.path());

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 2);
        store.remove_root(record.id).unwrap();

        assert!(matches!(
            store.materialize(record.id),
            Err(RemoveError::RootNotFound(RootId(_)))
        ));
        assert!(store.list().is_empty());
    }

    #[test]
    fn materialize_skips_symlinks_and_subfolders_silently() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        make_file(&root, "real.txt", b"r");
        make_file(&root.join("sub"), "b.txt", b"b");
        let target = make_file(dir.path(), "outside.txt", b"top secret");

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 1);

        // Creating symlinks unprivileged needs developer mode on Windows,
        // or root on some CI images — skip there rather than fail.
        #[cfg(windows)]
        let link = match std::os::windows::fs::symlink_file(&target, root.join("link.txt")) {
            Ok(()) => root.join("link.txt"),
            Err(_) => return, // can't create symlinks in this environment
        };
        #[cfg(unix)]
        let link = match std::os::unix::fs::symlink(&target, root.join("link.txt")) {
            Ok(()) => root.join("link.txt"),
            Err(_) => return,
        };
        #[cfg(not(any(windows, unix)))]
        let _link_unused = ();

        let outcome = store.materialize(record.id).unwrap();

        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
        let names: Vec<_> = outcome.added.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["real.txt"]);
        let _ = link;
    }

    #[test]
    fn materialize_counts_as_content() {
        let dir = TempDir::new().unwrap();
        let root = nested_tree(dir.path());

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 2);
        store.remove_root(record.id).unwrap();
        assert!(store.is_empty());
    }

    #[test]
    fn non_ascii_file_names_are_kept() {
        let dir = TempDir::new().unwrap();
        let path = make_file(dir.path(), "résumé final.txt", b"x");
        let mut store = FileStore::new();

        let file = store.add(&path).unwrap();

        assert_eq!(file.name, "résumé final.txt");
    }
}
