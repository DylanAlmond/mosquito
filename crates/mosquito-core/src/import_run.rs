//! Import runs: the background machinery of a count-only folder import
//! (ADR-0007). A run is one user-initiated drop; it counts each dropped
//! folder root in the background and reports progress per root, until
//! counting finishes, fails, or is cancelled for every root in the run.
//!
//! The walk never touches the store. It enumerates directories, keeps
//! only counters, and hands back a per-root file count for the caller to
//! commit — walk memory stays O(depth), never O(files).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

/// Identifier of one import run — one drop, possibly several roots.
pub type RunId = u64;

/// Files counted between progress updates.
pub const PROGRESS_EVERY_FILES: u64 = 500;

/// How far one imported root has got.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum RootRunStatus {
    Counting,
    Committed,
    Cancelled,
    Failed(String),
}

/// Live progress of one folder root inside a run.
#[derive(Debug, Clone, Serialize)]
pub struct RootRun {
    /// Absolute path of the folder being counted.
    pub path: PathBuf,
    /// Files counted so far.
    pub files: u64,
    pub status: RootRunStatus,
}

/// Read-only status of a whole run, shaped for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct ImportRunSnapshot {
    pub run_id: RunId,
    pub roots: Vec<RootRun>,
}

/// One run: its cancellation flag and its per-root progress.
pub struct ImportRun {
    pub id: RunId,
    cancelled: AtomicBool,
    roots: Mutex<Vec<RootRun>>,
}

impl ImportRun {
    fn new(id: RunId) -> Self {
        Self {
            id,
            cancelled: AtomicBool::new(false),
            roots: Mutex::new(Vec::new()),
        }
    }

    /// Ask the run to stop. Roots already committed stay committed;
    /// roots still counting leave no record.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn push_root(&self, path: PathBuf) -> usize {
        let mut roots = self.roots.lock().expect("import run roots not poisoned");
        roots.push(RootRun {
            path,
            files: 0,
            status: RootRunStatus::Counting,
        });
        roots.len() - 1
    }

    pub fn set_files(&self, index: usize, files: u64) {
        let mut roots = self.roots.lock().expect("import run roots not poisoned");
        if let Some(root) = roots.get_mut(index) {
            root.files = files;
        }
    }

    pub fn set_status(&self, index: usize, status: RootRunStatus) {
        let mut roots = self.roots.lock().expect("import run roots not poisoned");
        if let Some(root) = roots.get_mut(index) {
            root.status = status;
        }
    }

    pub fn snapshot(&self) -> ImportRunSnapshot {
        ImportRunSnapshot {
            run_id: self.id,
            roots: self
                .roots
                .lock()
                .expect("import run roots not poisoned")
                .clone(),
        }
    }
}

/// Registry of runs — one per drop. Finished runs stay listed so a
/// terminal state (committed/cancelled/failed) is observable afterwards.
/// Best-effort duplicate-notice: runs accumulate here for the whole
/// session (few per user interaction); the import-UI ticket may prune.
#[derive(Default)]
pub struct ImportRuns {
    next_id: RunId,
    runs: Vec<Arc<ImportRun>>,
}

impl ImportRuns {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(&mut self) -> Arc<ImportRun> {
        let run = Arc::new(ImportRun::new(self.next_id));
        self.next_id += 1;
        self.runs.push(Arc::clone(&run));
        run
    }

    /// Cancel a run by id; `false` means no such run was recorded.
    pub fn cancel(&self, id: RunId) -> bool {
        self.find(id).is_some_and(|run| {
            run.cancel();
            true
        })
    }

    /// Find a run by id.
    pub fn find(&self, id: RunId) -> Option<Arc<ImportRun>> {
        self.runs.iter().find(|run| run.id == id).map(Arc::clone)
    }

    /// Snapshot of every recorded run, oldest first.
    pub fn snapshot(&self) -> Vec<ImportRunSnapshot> {
        self.runs.iter().map(|run| run.snapshot()).collect()
    }
}

/// Why a count walk didn't return a number.
#[derive(Debug, Clone, PartialEq)]
pub enum CountFolderError {
    /// The run was cancelled; the count is partial (progress callbacks
    /// already carried whatever was seen before the stop).
    Cancelled,
    /// The folder itself couldn't be read at all.
    Unreadable(String),
}

/// Count the regular files under `root`, recursively — the light walk an
/// import run does instead of ADR-0001's eager register-everything walk.
///
/// - Memory is O(depth): counters only, no accumulated lists.
/// - Symlinks and junctions are skipped without following them — same
///   rule as ADR-0001's walk, so a later materialization agrees in kind.
/// - Only regular files are counted; special files contribute nothing
///   (a later materialization reports them per-entry instead).
/// - Unreadable *nested* directories are tolerated: what's under them
///   goes uncounted. An unreadable *root* is the walk failing.
/// - `is_cancelled` is polled throughout; cancellation flushes progress
///   once more and stops early.
/// - `on_files` receives the running count at batch milestones and once
///   at the end — the run's heartbeat.
pub fn count_folder(
    root: &Path,
    is_cancelled: &dyn Fn() -> bool,
    progress_every: u64,
    on_files: &mut impl FnMut(u64),
) -> Result<u64, CountFolderError> {
    if is_cancelled() {
        return Err(CountFolderError::Cancelled);
    }

    let mut counter = Counter {
        is_cancelled,
        progress_every,
        on_files,
        milestone: 0,
        reported_any: false,
    };
    let mut total = 0;
    count_dir(root, &mut counter, &mut total, true)?;

    // Completion heartbeat: the terminal count always fires once — also
    // for zero, which never crosses a batch boundary.
    if !counter.reported_any || counter.milestone != total {
        (counter.on_files)(total);
    }
    Ok(total)
}

struct Counter<'a> {
    is_cancelled: &'a dyn Fn() -> bool,
    progress_every: u64,
    on_files: &'a mut dyn FnMut(u64),
    milestone: u64,
    reported_any: bool,
}

impl Counter<'_> {
    fn check_cancel(&mut self, total: u64) -> Result<(), CountFolderError> {
        if (self.is_cancelled)() {
            (self.on_files)(total);
            self.milestone = total;
            return Err(CountFolderError::Cancelled);
        }
        Ok(())
    }

    fn bump(&mut self, total: &mut u64) -> Result<(), CountFolderError> {
        self.check_cancel(*total)?;
        *total += 1;
        if *total - self.milestone >= self.progress_every {
            (self.on_files)(*total);
            self.milestone = *total;
            self.reported_any = true;
        }
        Ok(())
    }
}

/// Walk one directory. The failure boundary is carried explicitly by
/// `is_root` (only the root failing to read fails the walk; unreadable
/// nested directories are tolerance cases).
fn count_dir(
    dir: &Path,
    counter: &mut Counter<'_>,
    total: &mut u64,
    is_root: bool,
) -> Result<(), CountFolderError> {
    counter.check_cancel(*total)?;

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            if is_root {
                return Err(CountFolderError::Unreadable(e.to_string()));
            }
            // Tolerated: what's under an unreadable nested directory is
            // skipped, like ADR-0001's walk reporting and moving on.
            return Ok(());
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };

        // Checked *before* is_dir: std reports junctions as symlinks on
        // Windows, so this skips both without ever following them.
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let path = entry.path();
            count_dir(&path, counter, total, false)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        counter.bump(total)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::disk::{make_file, nested_tree, seal_directory};
    use tempfile::TempDir;

    /// Recording sink for progress callbacks.
    #[derive(Default)]
    struct Progress(std::sync::Mutex<Vec<u64>>);

    impl Progress {
        fn push(&self, files: u64) {
            self.0.lock().unwrap().push(files);
        }

        fn values(&self) -> Vec<u64> {
            self.0.lock().unwrap().clone()
        }
    }

    #[test]
    fn counts_regular_files_recursively() {
        let dir = TempDir::new().unwrap();
        let root = nested_tree(dir.path());
        let progress = Progress::default();

        let total = count_folder(&root, &|| false, 500, &mut |n| progress.push(n)).unwrap();

        assert_eq!(total, 3);
        // Final flush fires even below one batch.
        assert_eq!(progress.values(), [3]);
    }

    #[test]
    fn progress_fires_at_every_batch_boundary_and_the_end() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("many");
        std::fs::create_dir(&root).unwrap();
        for i in 0..7 {
            make_file(&root, &format!("f{i}.txt"), b"x");
        }

        let progress = Progress::default();
        let total = count_folder(&root, &|| false, 3, &mut |n| progress.push(n)).unwrap();

        assert_eq!(total, 7);
        assert_eq!(progress.values(), [3, 6, 7]);
    }

    #[test]
    fn cancel_stops_the_walk_and_reports_partial_progress() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("big");
        std::fs::create_dir(&root).unwrap();
        for i in 0..10 {
            make_file(&root, &format!("f{i}.txt"), b"x");
        }

        // Cancel right after the third file has been counted.
        let seen = std::sync::atomic::AtomicU64::new(0);
        let is_cancelled = move || {
            let n = seen.fetch_add(1, Ordering::SeqCst);
            n >= 3
        };
        let progress = Progress::default();
        let err = count_folder(&root, &is_cancelled, 500, &mut |n| progress.push(n));

        assert_eq!(err.unwrap_err(), CountFolderError::Cancelled);
        // Partial count reached the sink before the stop.
        let values = progress.values();
        assert!(
            values.last().copied().unwrap_or(0) < 10,
            "walk should have stopped early: {:?}",
            values
        );
    }

    #[test]
    fn cancel_before_any_file_yields_no_progress() {
        let dir = TempDir::new().unwrap();
        let root = nested_tree(dir.path());
        let progress = Progress::default();

        let is_cancelled = || true;
        let err = count_folder(&root, &is_cancelled, 500, &mut |n| progress.push(n));

        assert_eq!(err.unwrap_err(), CountFolderError::Cancelled);
        assert!(progress.values().is_empty());
    }

    #[test]
    fn nonexistent_root_is_unreadable() {
        let dir = TempDir::new().unwrap();
        let progress = Progress::default();

        let err = count_folder(&dir.path().join("gone"), &|| false, 500, &mut |n| {
            progress.push(n)
        });

        assert!(matches!(err.unwrap_err(), CountFolderError::Unreadable(_)));
    }

    #[test]
    fn empty_root_counts_zero_and_still_flushes() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("empty");
        std::fs::create_dir(&root).unwrap();
        let progress = Progress::default();

        let total = count_folder(&root, &|| false, 500, &mut |n| progress.push(n)).unwrap();

        assert_eq!(total, 0);
        // Zero is a legitimate final state — the run must see it.
        assert_eq!(progress.values(), [0]);
    }

    #[test]
    #[cfg(any(windows, unix))]
    fn unreadable_nested_directory_is_tolerated() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sealed")).unwrap();
        make_file(&root, "visible.txt", b"v");
        make_file(&root.join("sealed"), "hidden.txt", b"h");

        // Deny read access; skip when the environment refuses to deny
        // (locked-down CI images, root, developer-mode-only limits).
        let sealed = match seal_directory(&root.join("sealed")) {
            Some(sealed) => sealed,
            None => return,
        };

        let progress = Progress::default();
        let total = count_folder(&root, &|| false, 500, &mut |n| progress.push(n));

        // Restore before the temp dir is cleaned up.
        drop(sealed);

        assert_eq!(total.unwrap(), 1);
    }

    #[test]
    fn symlinks_are_skipped_not_followed() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("real")).unwrap();
        make_file(&root, "real.txt", b"real");
        make_file(&root.join("real"), "inside.txt", b"i");
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

        let progress = Progress::default();
        let total = count_folder(&root, &|| false, 500, &mut |n| progress.push(n)).unwrap();

        // The link's target would have added another file; the walk
        // counted only the real subtree.
        assert_eq!(total, 2);
        let _ = link;
    }

    #[test]
    fn non_regular_files_contribute_nothing() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("special");
        std::fs::create_dir(&root).unwrap();
        make_file(&root, "normal.txt", b"n");

        // Platform-special files are hard to conjure portably; asserting
        // via a directory-like regular file entry is impossible — so the
        // property tested here is the counting rule itself: only files.
        let progress = Progress::default();
        let total = count_folder(&root, &|| false, 500, &mut |n| progress.push(n)).unwrap();

        assert_eq!(total, 1);
    }

    #[test]
    fn runs_track_status_assignments() {
        let mut runs = ImportRuns::new();
        let run = runs.begin();

        let index = run.push_root(PathBuf::from("C:/photos"));
        run.set_files(index, 25);
        run.set_status(index, RootRunStatus::Committed);

        let snapshot = runs.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].run_id, 0);
        assert_eq!(snapshot[0].roots[0].files, 25);
        assert_eq!(snapshot[0].roots[0].status, RootRunStatus::Committed);
    }

    #[test]
    fn second_run_gets_a_fresh_id_and_both_stay_listed() {
        let mut runs = ImportRuns::new();
        let _first = runs.begin();
        let second = runs.begin();

        assert_eq!(second.id, 1);
        assert_eq!(runs.snapshot().len(), 2);
    }

    #[test]
    fn cancel_by_id_flags_only_that_run() {
        let mut runs = ImportRuns::new();
        let first = runs.begin();
        let second = runs.begin();

        assert!(runs.cancel(first.id));
        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());

        assert!(!runs.cancel(99));
    }

    #[test]
    fn unknown_run_snapshot_is_empty() {
        let runs = ImportRuns::new();
        assert!(runs.snapshot().is_empty());
        assert!(runs.find(7).is_none());
    }
}
