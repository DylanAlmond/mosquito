//! The store-side listing derivation (ADR-0006): the single place that
//! turns the store's members and root records into listing rows. The
//! desktop fetches pages of these rows through a Tauri command, and the
//! receiver's fragment route will serve the identical derivation —
//! equal by construction, not by mirrored implementations.
//!
//! Two request shapes exist:
//!
//! - `list_top` — the top of the list: loose files plus one collapsed
//!   row per root record. Pure, no disk I/O, so showing it costs
//!   nothing beyond what the import already recorded.
//! - `list_page` — one request-scoped page (~200 rows) of one node's
//!   content, identified by a root record id plus a relative subfolder
//!   path. The request itself materializes the node's direct children
//!   (ADR-0005: the listing showing that folder is the trigger) and
//!   derives the rest — lazily-known subfolder rows — from the disk at
//!   view time, the live-subtree contract.

use std::collections::HashSet;
use std::path::Path;

use serde::Serialize;

use crate::file_store::{FileStore, RootId, SharedFile};

/// How many rows one listing page holds. ADR-0006's budget: the UI
/// fetches ahead while scrolling, so nothing holds more than this
/// plus what the fetch-ahead already pulled.
pub const LISTING_PAGE_ROWS: usize = 200;

/// One top-level row: a collapsed, lazily-known group.
#[derive(Debug, Clone, Serialize)]
pub struct RootRow {
    pub id: RootId,
    /// The root record path's last segment — the group's display name.
    pub name: String,
    /// Version-of-record disambiguation for same-named roots: the bare
    /// name first, then `name-2`, `name-3`, … probing for the next free
    /// key, exactly the web listing's rule. The desktop keys its
    /// expansion state by it; no absolute path is ever embedded.
    pub key: String,
    /// Files counted at import time — lazily-known contents may have
    /// drifted since (live subtree), which the listing deliberately
    /// accepts and display-only never re-walks.
    pub file_count: u64,
}

/// What the top of a listing looks like: loose files first, then the
/// imported groups.
#[derive(Debug, Clone, Serialize)]
pub struct TopListing {
    pub loose: Vec<SharedFile>,
    pub roots: Vec<RootRow>,
}

/// One derived row of a node's page.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageRow {
    /// A registered shared file — materialized, so it carries its id
    /// and everything selection, removal, and download need.
    File { file: SharedFile },
    /// A subfolder that is still lazily known: expandable, never
    /// clickable, carrying no id and no count.
    LazyFolder { name: String },
}

/// One request-scoped page (`Page` in the glossary) of a node's rows.
#[derive(Debug, Clone, Serialize)]
pub struct NodePage {
    pub root_id: RootId,
    /// Where the node hangs below the root record: empty for the root
    /// itself, `sub/inner` for nested listings.
    pub sub_path: String,
    /// The window's first row index — pages always restart from
    /// `list_page(root, sub, offset, …)`.
    pub offset: usize,
    /// Up to the requested limit of rows: files first (name-sorted),
    /// then the lazily-known subfolders (name-sorted).
    pub rows: Vec<PageRow>,
    /// Materialization failures of this node's folders: inline per-node
    /// markers, never import failures, never blocking the rest. Rides the
    /// failed-add wire shape (path + error); GLOSSARY keeps the two terms
    /// apart conceptually.
    pub failures: Vec<crate::file_store::FailedAdd>,
    /// How many rows the node derives for this request, across all
    /// pages. Pages may disagree between requests — a live subtree may
    /// present differently on the next fetch (ADR-0006).
    pub total: usize,
    /// Whether a later page holds more of this node's rows.
    pub has_more: bool,
}

/// The `impl` lives beside its module-own tests rather than inside
/// `file_store.rs`, which stays about membership; this module is about
/// presenting it.
impl FileStore {    /// Derive the listing's top: loose files lead, then one row per
    /// root record in import order. Pure — no disk I/O — so a
    /// root-record-only share displays with no additional registered
    /// cost (issue #24's first acceptance box).
    pub fn list_top(&self) -> TopListing {
        let loose = self
            .list()
            .iter()
            .filter(|file| file.provenance.is_none())
            .cloned()
            .collect();

        let mut roots = Vec::with_capacity(self.roots().len());
        let mut keys_used: HashSet<String> = HashSet::new();
        for record in self.roots() {
            let name = root_display_name(&record.path);
            // Same probing rule as the web listing's `root_keys`: keep
            // the bare name for the first same-named root, then probe
            // `-2`, `-3`, … for every later one.
            let mut ordinal = 2;
            let mut key = name.clone();
            while !keys_used.insert(key.clone()) {
                key = format!("{name}-{ordinal}");
                ordinal += 1;
            }
            roots.push(RootRow {
                id: record.id,
                name,
                key,
                file_count: record.file_count,
            });
        }

        TopListing { loose, roots }
    }
}

/// The root record's display name: its last path segment. Falls back to
/// the whole lossy string when the path has no final component (a bare
/// drive root, say) — the same last-segment reading the old derived
/// tree used.
fn root_display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::disk::make_file;
    use tempfile::TempDir;

    // -- list_top ----------------------------------------------------------

    #[test]
    fn top_lists_loose_files_and_one_row_per_root_record() {
        let dir = TempDir::new().unwrap();
        let photos = dir.path().join("Photos");
        std::fs::create_dir(&photos).unwrap();
        // Two same-named roots for suffixing; the second lives deeper.
        let backup_photos = dir.path().join("backup").join("Photos");
        std::fs::create_dir_all(&backup_photos).unwrap();

        let mut store = FileStore::new();
        store.add(make_file(dir.path(), "notes.txt", b"n")).unwrap();
        let first = store.commit_root_record(&photos, 2);
        let second = store.commit_root_record(&backup_photos, 5);

        let top = store.list_top();

        assert_eq!(top.loose.len(), 1);
        assert_eq!(top.loose[0].name, "notes.txt");

        // One row per root record, insertion order, each with the root's
        // own display name (last path segment) and its counted files.
        assert_eq!(top.roots.len(), 2);
        assert_eq!(top.roots[0].id, first.id);
        assert_eq!(top.roots[0].name, "Photos");
        assert_eq!(top.roots[0].file_count, 2);
        assert_eq!(top.roots[1].id, second.id);
        assert_eq!(top.roots[1].name, "Photos");
        assert_eq!(top.roots[1].file_count, 5);

        // Display without derivation side effects: only the loose file
        // the import itself registered is in the store — no folder
        // members were materialized to show counts.
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.roots().len(), 2);
    }

    #[test]
    fn top_suffixes_duplicate_root_names_like_the_listing_mirror() {
        let dir = TempDir::new().unwrap();
        let photos = dir.path().join("Photos");
        std::fs::create_dir(&photos).unwrap();
        let photos_2 = dir.path().join("Photos-2");
        std::fs::create_dir(&photos_2).unwrap();

        let mut store = FileStore::new();
        store.commit_root_record(&photos, 2);
        store.commit_root_record(&photos_2, 1);

        let top = store.list_top();

        // The literally-named `Photos-2` root keeps its own name as
        // key — it doesn't collide yet; probing only appends when the
        // candidate is taken (same rule as the web listing's root
        // keys).
        assert_eq!(top.roots[0].key, "Photos");
        assert_eq!(top.roots[1].key, "Photos-2");
        assert_eq!(top.roots[1].name, "Photos-2");
    }

    #[test]
    fn top_probes_for_fully_colliding_keys() {
        let dir = TempDir::new().unwrap();
        let photos = dir.path().join("Photos");
        std::fs::create_dir(&photos).unwrap();
        let photos_2 = dir.path().join("Photos-2");
        std::fs::create_dir(&photos_2).unwrap();
        let backup_photos = dir.path().join("backup").join("Photos");
        std::fs::create_dir_all(&backup_photos).unwrap();
        let photos_2_2 = dir.path().join("Photos-2-2");
        std::fs::create_dir(&photos_2_2).unwrap();

        let mut store = FileStore::new();
        store.commit_root_record(&photos, 1);
        store.commit_root_record(&photos_2, 1);
        store.commit_root_record(&backup_photos, 1);
        store.commit_root_record(&photos_2_2, 1);

        let top = store.list_top();

        // Two `Photos` roots and a root literally named `Photos-2`
        // already: the second `Photos` probes past the taken `Photos-2`
        // to `Photos-2-2`, and the taken suffix pushes the literal
        // one to `Photos-2-2-2…
        assert_eq!(top.roots[0].key, "Photos");
        assert_eq!(top.roots[1].key, "Photos-2");
        assert_eq!(top.roots[2].key, "Photos-3");
        assert_eq!(top.roots[3].key, "Photos-2-2");
    }

    #[test]
    fn top_keys_shift_when_an_earlier_same_named_root_is_removed() {
        let dir = TempDir::new().unwrap();
        let photos = dir.path().join("Photos");
        std::fs::create_dir(&photos).unwrap();
        let backup_photos = dir.path().join("backup").join("Photos");
        std::fs::create_dir_all(&backup_photos).unwrap();

        let mut store = FileStore::new();
        let first = store.commit_root_record(&photos, 2);
        let second = store.commit_root_record(&backup_photos, 5);

        // Before: first-appearance holds the bare key.
        assert_eq!(store.list_top().roots[0].key, "Photos");
        assert_eq!(store.list_top().roots[1].key, "Photos-2");

        // After the first is removed, the remaining one re-keys (per
        // request derivation) — same shape the web listing pinned.
        store.remove_root(first.id).unwrap();
        store.remove_root(second.id).unwrap();
        let third = store.commit_root_record(&backup_photos, 1);
        let top = store.list_top();
        assert_eq!(top.roots.len(), 1);
        assert_eq!(top.roots[0].id, third.id);
        assert_eq!(top.roots[0].key, "Photos");
    }

    // -- list_page ---------------------------------------------------------

    #[test]
    fn a_node_page_materializes_direct_children_and_show_lazy_folders() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("Photos");
        std::fs::create_dir_all(root.join("vacation")).unwrap();
        make_file(&root, "beach.jpg", b"b");
        make_file(&root, "zoo.png", b"z");
        make_file(&root.join("vacation"), "trip.jpg", b"t");

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 3);

        let page = store.list_page(record.id, "", 0, LISTING_PAGE_ROWS).unwrap();

        // Direct members only: the deeper `vacation/trip.jpg` stays
        // lazily known until `vacation` itself is listed.
        let files: Vec<&str> = page
            .rows
            .iter()
            .filter_map(|row| match row {
                PageRow::File { file } => Some(file.name.as_str()),
                PageRow::LazyFolder { .. } => None,
            })
            .collect();
        assert_eq!(files, ["beach.jpg", "zoo.png"]);
        let folders: Vec<&str> = page
            .rows
            .iter()
            .filter_map(|row| match row {
                PageRow::LazyFolder { name } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(folders, ["vacation"]);

        // Rows are a page: files first, then lazily-known folders.
        assert_eq!(page.rows.len(), 3);
        assert_eq!(page.total, 3);
        assert!(!page.has_more);
        assert!(page.failures.is_empty(), "{:?}", page.failures);

        // The listing request materialized the direct children: they
        // are now real shared files with provenance.
        assert_eq!(store.list().len(), 2);
        let beach = store
            .list()
            .iter()
            .find(|file| file.name == "beach.jpg")
            .unwrap();
        let provenance = beach.provenance.as_ref().unwrap();
        assert_eq!(provenance.folder_root, root);
        assert_eq!(provenance.relative_path, "beach.jpg");
    }

    #[test]
    fn repeat_page_fetches_never_duplicate_registrations() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("Photos");
        std::fs::create_dir(&root).unwrap();
        make_file(&root, "beach.jpg", b"b");

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 1);

        let first = store.list_page(record.id, "", 0, LISTING_PAGE_ROWS).unwrap();
        let second = store.list_page(record.id, "", 0, LISTING_PAGE_ROWS).unwrap();

        assert_eq!(store.list().len(), 1, "idempotent materialization");
        assert_eq!(
            match (&first.rows[0], &second.rows[0]) {
                (PageRow::File { file: a }, PageRow::File { file: b }) => (a.id, b.id),
                _ => panic!("expected file rows"),
            }
            .0,
            match &second.rows[0] {
                PageRow::File { file } => file.id,
                _ => unreachable!(),
            },
            "same file id on refetch — no duplicated rows"
        );
    }

    #[test]
    fn pages_windows_slice_files_then_folders_across_both_kinds() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("Photos");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("zzz-folder")).unwrap();
        for name in ["one.txt", "two.txt", "three.txt"] {
            make_file(&root, name, b"x");
        }
        std::fs::create_dir(root.join("aaa-folder")).unwrap();

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 5);

        // A two-row page: the first file rows, then the folder rows.
        let first = store.list_page(record.id, "", 0, 2).unwrap();
        assert_eq!(first.total, 5);
        assert!(first.has_more);
        assert_eq!(
            names(&first.rows),
            ["one.txt", "three.txt"],
            "files name-sorted"
        );

        let second = store.list_page(record.id, "", 2, 2).unwrap();
        assert_eq!(names(&second.rows), ["two.txt", "aaa-folder"]);
        assert!(second.has_more);

        let third = store.list_page(record.id, "", 4, 2).unwrap();
        assert_eq!(names(&third.rows), ["zzz-folder"]);
        assert!(!third.has_more, "the window ended inside the node's rows");
    }

    #[test]
    fn subfolder_pages_list_the_nested_nodes_direct_children() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("Photos");
        std::fs::create_dir_all(root.join("vacation").join("deep")).unwrap();
        make_file(&root.join("vacation"), "trip.jpg", b"t");
        make_file(&root.join("vacation").join("deep"), "c.txt", b"c");
        make_file(&root, "beach.jpg", b"b");

        let mut root_record = store_with_root(&root);


        // Root page: the direct file plus the lazily-known `vacation`.
        let top = root_record.list_page_record("", 0, LISTING_PAGE_ROWS);
        assert_eq!(names(&top.rows), ["beach.jpg", "vacation"]);

        // The subfolder's own page materializes its direct children:
        // ids stay attached to relative paths below the ROOT (`beach.jpg`
        // vs `vacation/trip.jpg`), while the deep folder stays lazy.
        let sub = root_record.list_page_record("vacation", 0, LISTING_PAGE_ROWS);
        assert_eq!(names(&sub.rows), ["trip.jpg", "deep"]);
        assert_eq!(sub.sub_path, "vacation");
        let trip = match &sub.rows[0] {
            PageRow::File { file } => file,
            other => panic!("expected a file row, got {other:?}"),
        };
        assert_eq!(trip.provenance.as_ref().unwrap().relative_path, "vacation/trip.jpg");
    }

    #[test]
    fn unreadable_members_surface_as_inline_page_failures() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("Photos");
        std::fs::create_dir_all(root.join("sealed")).unwrap();
        make_file(&root, "visible.txt", b"v");

        let Some(_guard) = crate::test_support::disk::seal_directory(&root.join("sealed")) else {
            return; // the environment can't deny reads; skip
        };

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 1);

        let page = store.list_page(record.id, "", 0, LISTING_PAGE_ROWS).unwrap();

        // The readable sibling materializes; the sealed subfolder is a
        // lazily-known row… its listing (the next turn) is where its
        // unreadable members would surface.
        assert!(page.failures.is_empty(), "{:?}", page.failures);
        assert_eq!(names(&page.rows), ["visible.txt", "sealed"]);

        // Listing the sealed subfolder fails at the node level: an
        // inline failure on that node, the listing refuses nothing.
        let sealed = store.list_page(record.id, "sealed", 0, LISTING_PAGE_ROWS).unwrap();
        assert_eq!(sealed.rows.len(), 0);
        assert_eq!(sealed.failures.len(), 1);
        assert!(sealed.failures[0].path.ends_with("sealed"));
    }

    #[test]
    fn sub_paths_may_never_escape_the_root_record() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("Photos");
        std::fs::create_dir(&root).unwrap();
        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 0);

        for escape in ["..", "sub/../../escape", "sub\\..\\..", "."] {
            assert!(
                matches!(
                    store.list_page(record.id, escape, 0, LISTING_PAGE_ROWS),
                    Err(crate::error::RemoveError::SubPathEscapesRoot(_))
                ),
                "traversal rejected for {escape:?}"
            );
        }
        // Unknown root still reads as the one domain error.
        assert!(matches!(
            store.list_page(crate::file_store::RootId(42), "", 0, LISTING_PAGE_ROWS),
            Err(crate::error::RemoveError::RootNotFound(_))
        ));
    }

    #[test]
    fn files_added_after_the_first_fetch_appear_on_the_next_live_fetch() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("Photos");
        std::fs::create_dir(&root).unwrap();
        make_file(&root, "first.jpg", b"1");

        let mut store = FileStore::new();
        let record = store.commit_root_record(&root, 1);

        let first = store.list_page(record.id, "", 0, LISTING_PAGE_ROWS).unwrap();
        assert_eq!(names(&first.rows), ["first.jpg"]);

        // The live-subtree contract: contents added on disk after the
        // import show up in a later page fetch.
        make_file(&root, "second.jpg", b"2");
        let again = store.list_page(record.id, "", 0, LISTING_PAGE_ROWS).unwrap();
        assert_eq!(names(&again.rows), ["first.jpg", "second.jpg"]);
        assert_eq!(store.list().len(), 2);
    }

    /// Test-side helpers: row names in order, and a store holding one
    /// root record whose id the call passes through.
    fn names(rows: &[PageRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                PageRow::File { file } => file.name.clone(),
                PageRow::LazyFolder { name } => name.clone(),
            })
            .collect()
    }

    fn store_with_root(root: &std::path::Path) -> FileStore {
        let mut store = FileStore::new();
        store.commit_root_record(root, 3);
        store
    }

    impl FileStore {
        /// Test helper: call `list_page` through the helper root's id.
        fn list_page_record(&mut self, sub_path: &str, offset: usize, limit: usize) -> NodePage {
            let id = self.roots()[0].id;
            self.list_page(id, sub_path, offset, limit).unwrap()
        }
    }
}

