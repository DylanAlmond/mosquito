use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{AddFileError, RemoveError};
use crate::listing_pages::{NodePage, PageRow};

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
#[derive(Debug, Clone, Serialize)]
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

/// The store's record of an imported folder: where the folder is and a
/// file count. Nothing beneath the root is registered on import — its
/// members become shared files only as they are materialized.
#[derive(Debug, Clone, Serialize)]
pub struct RootRecord {
    pub id: RootId,
    /// Absolute path of the folder the user imported.
    pub path: PathBuf,
    /// The live count (ADR-0008 / GLOSSARY): committed at 0 when the
    /// folder is dropped, updated by the import walk's progress
    /// checkpoints. Display only on every surface — a climbing count is
    /// live-subtree drift by design, and nothing materializes because
    /// of it.
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
    /// Subfolders seen during the scan: lazily-known rows, no ids.
    pub lazy_subfolders: Vec<String>,
}

/// What removing a root record took with it.
#[derive(Debug, Serialize)]
pub struct RootRemoval {
    pub record: RootRecord,
    /// Every materialized member swept away with the record.
    pub files: Vec<SharedFile>,
}

/// What removing one node address took with it.
#[derive(Debug, Serialize)]
pub struct NodeRemoval {
    /// The materialized members swept from the subtree.
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

    /// Record an imported folder's drop (ADR-0008): the record commits
    /// synchronously at drop time, before the count walk starts, with the
    /// count going live from here — the walk moves it at its progress
    /// checkpoints. Nothing beneath the root is registered here; ids are
    /// never reused, even after removal.
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

    /// Update the root record's live count (ADR-0008): the count walk's
    /// checkpoint heartbeat, one brief call per checkpoint. The count is
    /// display only — no registration and no materialization follows
    /// from it. An unknown id is a no-op: the record was removed while
    /// its walk still ran (cancel, failure, or the user's own removal),
    /// and removal wins over any late checkpoint.
    pub fn update_root_count(&mut self, id: RootId, count: u64) {
        if let Some(record) = self.roots.iter_mut().find(|record| record.id == id) {
            record.file_count = count;
        }
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

    /// Remove every materialized member under one node address (the
    /// sub path relative to the root record, node-directory rules as in
    /// any listing request). The root record itself survives — with its
    /// live count reduced by the members that left — because the share
    /// still covers the record's whole tree view; parity with removing
    /// a member file, at subtree grain.
    ///
    /// Live-subtree tolerance (ADR-0005): removal takes only what the
    /// store registered at the time — a later view of that node
    /// re-materializes per the view-trigger rule, the same way a removed
    /// member returns if a node page re-registers it. The definitive
    /// "nothing of this folder in my share" action is the record's own
    /// removal, not a node's.
    ///
    /// An empty sub path addresses the record itself and delegates to
    /// `remove_root` — the whole sweep, record included.
    pub fn remove_node(&mut self, id: RootId, sub_path: &str) -> Result<NodeRemoval, RemoveError> {
        if sub_path.is_empty() {
            return self.remove_root(id).map(|removal| NodeRemoval {
                files: removal.files,
            });
        }

        let index = self
            .roots
            .iter()
            .position(|record| record.id == id)
            .ok_or(RemoveError::RootNotFound(id))?;
        if node_directory(&self.roots[index].path, sub_path).is_none() {
            return Err(RemoveError::SubPathEscapesRoot(sub_path.to_string()));
        }
        let root_path = self.roots[index].path.clone();

        let mut removed = Vec::new();
        let mut remaining = Vec::new();
        for file in std::mem::take(&mut self.files) {
            let under_subtree = file.provenance.as_ref().is_some_and(|provenance| {
                provenance.folder_root == root_path
                    && relative_in_subtree(&provenance.relative_path, sub_path)
            });
            if under_subtree {
                removed.push(file);
            } else {
                remaining.push(file);
            }
        }
        self.files = remaining;

        // The count is display only (ADR-0008): reducing by the members
        // the store knew about is the honest update available — the
        // subtree's unregistered files were part of the walk number, so
        // the update can miss deeper members. Saturating because a
        // materialization that happened after the walk may exceed what
        // the displayed count ever accounted for.
        let record = &mut self.roots[index];
        record.file_count = record.file_count.saturating_sub(removed.len() as u64);

        Ok(NodeRemoval { files: removed })
    }

    /// Register the lazily-known direct children of `root`'s folder, or
    /// of a subfolder `sub_path` below it (ADR-0005): regular files
    /// directly inside become shared files; subfolders stay lazily-known
    /// (they list on their own turn), symlinks are skipped outright, and
    /// unreadable entries are reported per-entry like any batch add.
    /// The lazily-known subfolder names ride the outcome so a listing
    /// can derive its folder rows from the same single scan.
    ///
    /// Idempotent, keyed by (folder root, relative path): materializing
    /// twice — or two root records pointing at the same folder — converges
    /// on one registration per file instead of duplicates.
    pub fn materialize_at(&mut self, root: &Path, sub_path: &str) -> MaterializeOutcome {
        let mut outcome = MaterializeOutcome {
            added: Vec::new(),
            skipped: 0,
            failed: Vec::new(),
            lazy_subfolders: Vec::new(),
        };

        let node = node_directory(root, sub_path)
            .expect("materialize_at is called with an already-validated sub path");

        let entries = match std::fs::read_dir(&node) {
            Ok(entries) => entries,
            Err(e) => {
                outcome.failed.push(FailedAdd {
                    path: node,
                    error: e.to_string(),
                });
                return outcome;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    outcome.failed.push(FailedAdd {
                        path: node.clone(),
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
                outcome
                    .lazy_subfolders
                    .push(entry.file_name().to_string_lossy().into_owned());
                continue;
            }
            if !file_type.is_file() {
                outcome.failed.push(FailedAdd {
                    path,
                    error: "not a regular file".into(),
                });
                continue;
            }

            let relative_path = match relative_path_string(root, &path) {
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
                    .is_some_and(|p| p.folder_root == *root && p.relative_path == relative_path)
            });
            if already_registered {
                outcome.skipped += 1;
                continue;
            }

            match self.insert(
                path.clone(),
                Some(FolderProvenance {
                    folder_root: root.to_path_buf(),
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

        outcome
    }

    /// Register the lazily-known direct children of a root record's
    /// folder — the (folder root, relative path) of the record itself.
    /// The only failure is an unknown root id: materialization strokes
    /// per-entry problems as failures inside the outcome, not errors.
    pub fn materialize(&mut self, id: RootId) -> Result<MaterializeOutcome, RemoveError> {
        let record = self.node_root(id, "")?;
        Ok(self.materialize_at(&record, ""))
    }

    /// One paged window of `sub_path`'s rows below the root record
    /// `id` — the store-side listing derivation ADR-0006 and issue #24
    /// call for. The request itself materializes the node's direct
    /// children ([`FileStore::materialize_at`]); the page then derives
    /// from the registered membership plus the scan's lazily-known
    /// subfolder rows.
    ///
    /// Page slices hold files first (name-sorted), then lazily-known
    /// subfolders (name-sorted). Order guarantees hold within a page as
    /// fetched; a live subtree may present differently on the next page
    /// request — accepted by the contract.
    pub fn list_page(
        &mut self,
        id: RootId,
        sub_path: &str,
        offset: usize,
        limit: usize,
    ) -> Result<NodePage, RemoveError> {
        let record = self.node_root(id, sub_path)?;

        // The listing showing the folder IS the materialization
        // trigger: one scan registers direct children and hands back
        // the lazily-known subfolder names plus per-entry failures.
        let scan = self.materialize_at(&record, sub_path);

        // File rows come from the registered membership, not from the
        // scan — a member materialized earlier keeps its row even when
        // it vanishes from disk (frozen snapshot; its download fails
        // later, as for any shared file). One level deep only: this
        // node's direct children.
        let mut file_rows: Vec<SharedFile> = self
            .files
            .iter()
            .filter(|file| {
                file.provenance.as_ref().is_some_and(|provenance| {
                    provenance.folder_root == record
                        && relative_in_node(&provenance.relative_path, sub_path)
                })
            })
            .cloned()
            .collect();
        file_rows.sort_by_cached_key(|file| file.name.to_lowercase());

        let mut folders = scan.lazy_subfolders;
        folders.sort_by_cached_key(|name| name.to_lowercase());

        // A page always shows the node's rows in one order: files
        // first, then the lazily-known subfolders, each name-sorted.
        let total = file_rows.len() + folders.len();
        let mut rows: Vec<PageRow> = Vec::with_capacity(total);
        rows.extend(file_rows.into_iter().map(|file| PageRow::File { file }));
        rows.extend(folders.into_iter().map(|name| PageRow::LazyFolder { name }));

        let window: Vec<PageRow> = rows.into_iter().skip(offset).take(limit).collect();
        let has_more = total > offset + window.len();

        Ok(NodePage {
            root_id: id,
            sub_path: sub_path.to_string(),
            offset,
            rows: window,
            failures: scan.failed,
            total,
            has_more,
        })
    }

    /// The root record behind a node address — an id plus the relative
    /// sub path the node sits at — validated so the caller can address
    /// the node's subtree itself ([`crate::archive_subtree`]). Unknown
    /// ids and escaping sub paths are the caller errors
    /// ([`RemoveError`]), answered with not-found by the HTTP layer.
    pub(crate) fn node_root(&self, id: RootId, sub_path: &str) -> Result<PathBuf, RemoveError> {
        let record = self
            .roots
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.path.clone())
            .ok_or(RemoveError::RootNotFound(id))?;

        if node_directory(&record, sub_path).is_none() {
            return Err(RemoveError::SubPathEscapesRoot(sub_path.to_string()));
        }
        Ok(record)
    }

    /// Every registered member under one node address, any depth down:
    /// the node-keyed archive's selection. Members materialized *after*
    /// this call are deliberately absent — the caller pays the walk
    /// once, then plans from the collected snapshot (ADR-0005).
    pub(crate) fn subtree_members(&self, root: &Path, sub_path: &str) -> Vec<SharedFile> {
        let mut members: Vec<SharedFile> = self
            .files
            .iter()
            .filter(|file| {
                file.provenance.as_ref().is_some_and(|provenance| {
                    provenance.folder_root == root
                        && relative_in_subtree(&provenance.relative_path, sub_path)
                })
            })
            .cloned()
            .collect();

        // Deterministic write order, independent of materialization
        // history: member paths below the node, case-insensitively
        // sorted — the same reading the listing pages sort rows with.
        members.sort_by_cached_key(|file| {
            file.provenance
                .as_ref()
                .map(|provenance| provenance.relative_path.to_lowercase())
                .unwrap_or_default()
        });
        members
    }
}

/// Whether `relative_path` names anything under `node_sub_path`, any
/// depth down: the subtree filter the node-keyed archive selection
/// uses. Files carry `relative_path`s only, so the node's own path
/// never matches a member.
pub(crate) fn relative_in_subtree(relative_path: &str, sub_path: &str) -> bool {
    if sub_path.is_empty() {
        true
    } else {
        match relative_path.strip_prefix(sub_path) {
            Some(rest) => rest.starts_with('/'),
            None => false,
        }
    }
}

/// Whether `relative_path` names a direct child of `node_sub_path`:
/// directly-inside rows carry the node path as nothing (empty sub) or
/// `node/` prefix plus one more segment. This yields, for each row, the
/// level directly below the node — members one level down only.
fn relative_in_node(relative_path: &str, node_sub_path: &str) -> bool {
    if node_sub_path.is_empty() {
        !relative_path.contains('/')
    } else {
        match relative_path
            .strip_prefix(node_sub_path)
            .and_then(|rest| rest.strip_prefix('/'))
        {
            Some(rest) => !rest.contains('/'),
            None => false,
        }
    }
}

/// The absolute directory of a node: `root` itself for an empty sub
/// path, else `root` joined with the sub path's segments. Traversal is
/// rejected outright — every segment must be a plain name (no `..`,
/// no empty segments, no separators hidden inside a segment), so a
/// listing can never name something outside the root's subtree.
fn node_directory(root: &Path, sub_path: &str) -> Option<PathBuf> {
    if sub_path.is_empty() {
        return Some(root.to_path_buf());
    }
    let mut node = root.to_path_buf();
    for segment in sub_path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." || segment.contains('\\') {
            return None;
        }
        node.push(segment);
    }
    Some(node)
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
    fn update_root_count_moves_the_live_count() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("big");
        std::fs::create_dir(&root).unwrap();

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 0);

        // The walk's checkpoints move the number; nothing but the
        // record's count changes.
        store.update_root_count(record.id, 500);
        assert_eq!(store.roots()[0].file_count, 500);
        assert_eq!(store.list_top().roots[0].file_count, 500);
        // The count updates never register: the tree stays lazily known.
        assert!(store.list().is_empty());
        store.update_root_count(record.id, 1000);
        assert_eq!(store.roots()[0].file_count, 1000);
    }

    #[test]
    fn replayed_or_late_checkpoints_converge_to_whatever_they_carry() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("big");
        std::fs::create_dir(&root).unwrap();

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 0);

        // A re-applied earlier checkpoint (the idempotent convergence
        // ADR-0008's display-only count allows) just moves the number —
        // one registration-less write, no duplicated rows.
        store.update_root_count(record.id, 500);
        store.update_root_count(record.id, 1000);
        store.update_root_count(record.id, 500);
        assert_eq!(store.roots()[0].file_count, 500);
        assert_eq!(store.list_top().roots[0].file_count, 500);
        assert!(store.list().is_empty());
        assert_eq!(store.roots().len(), 1);
    }

    #[test]
    fn update_root_count_after_removal_is_a_no_op() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("big");
        std::fs::create_dir(&root).unwrap();

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 0);
        store.remove_root(record.id).unwrap();

        // The record was removed while its walk still ran (cancel,
        // failure, or the user's own removal): removal wins over any
        // late checkpoint, and nothing resurrects.
        store.update_root_count(record.id, 900);
        assert!(store.roots().is_empty());
        assert!(store.list().is_empty());
        assert!(store.is_empty());
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
    fn remove_node_sweeps_exactly_that_subtree_and_updates_the_live_count() {
        let dir = TempDir::new().unwrap();
        let root = nested_tree(dir.path()); // a.txt, sub/b.txt, sub/deep/c.txt, empty/

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 3);
        store.materialize(record.id).unwrap(); // direct children only: a.txt
        store.materialize_at(&root, "sub"); // b.txt on its own turn

        let removal = store.remove_node(record.id, "sub").unwrap();

        // Exactly the registered members under `sub`: the directly shared
        // member stays put, nothing deeper was ever registered, and the
        // record survives — with its live count reduced by what left.
        assert_eq!(removal.files.len(), 1);
        assert_eq!(
            removal.files[0].provenance.as_ref().unwrap().relative_path,
            "sub/b.txt"
        );
        assert_eq!(store.roots().len(), 1);
        assert_eq!(store.roots()[0].file_count, 2);
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].name, "a.txt");
    }

    #[test]
    fn remove_node_unknown_root_and_escaping_sub_are_errors() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("a");
        std::fs::create_dir(&root).unwrap();

        let mut store = FileStore::new();
        let record = store.commit_root_record(root, 0);

        assert!(matches!(
            store.remove_node(RootId(42), "sub"),
            Err(RemoveError::RootNotFound(RootId(42)))
        ));
        assert!(matches!(
            store.remove_node(record.id, "../escape"),
            Err(RemoveError::SubPathEscapesRoot(..))
        ));
    }

    #[test]
    fn remove_node_at_the_record_itself_is_the_whole_root_sweep() {
        let dir = TempDir::new().unwrap();
        let root = nested_tree(dir.path());

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 3);
        store.materialize(record.id).unwrap();

        let removal = store.remove_node(record.id, "").unwrap();

        // The node addressed is the record: the record plus every
        // materialized member, exactly what the group removal does.
        assert_eq!(removal.files.len(), 1);
        assert!(store.roots().is_empty());
        assert!(store.list().is_empty());
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
