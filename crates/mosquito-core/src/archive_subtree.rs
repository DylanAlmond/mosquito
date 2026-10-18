//! Whole-subtree materialization for archive downloads (ADR-0005):
//! the node-keyed zip route's "materialize" half, kept beside the
//! membership logic in [`crate::file_store`] and the byte writing in
//! [`crate::server`].
//!
//! Nothing here is streaming — the receiver explicitly asked for the
//! whole subtree's bytes, so the request deliberately pays large-scale
//! memory once: every live directory under the node is materialized
//! and the members collected before planning starts. This is kept out
//! of the shared "browse" path, whose locks stay one directory deep
//! (ADR-0006): each materialization step takes the store mutex for a
//! single directory scan and drops it before the next, so listings
//! keep moving while the walk proceeds.

use std::sync::Mutex;

use crate::RemoveError;
use crate::file_store::{FileStore, RootId, SharedFile};

/// Materialize every lazily-known directory under one node address
/// (a root record id plus a relative sub path, empty for the whole
/// root) and return the node's registered members, any depth down.
///
/// Materialization rides the store's existing idempotency key —
/// (folder root, relative path) — so re-zipping the same subtree
/// converges on one registration per file and never duplicates
/// entries. Per-entry materialization failures (unreadable members,
/// skipped links) never block the walk: like the listing's inline
/// markers, those members simply never registered, so the archive
/// holds exactly what materialized. Unknown ids and escaping sub
/// paths are caller errors surfaced as [`RemoveError`].
pub(crate) fn collect_subtree(
    store: &Mutex<FileStore>,
    root_id: RootId,
    sub_path: &str,
) -> Result<Vec<SharedFile>, RemoveError> {
    // One brief lock to resolve and validate the node address: wrong
    // answers are a 404 before any disk walk happens.
    let root = {
        let store = store.lock().expect("file store not poisoned");
        store.node_root(root_id, sub_path)?
    };

    // Depth-first over the live directories materialize_at hands
    // back: each lock covers one directory's scan at most — the same
    // hold one listing page fetch takes (ADR-0006).
    let mut queue = vec![sub_path.to_string()];
    while let Some(node_sub_path) = queue.pop() {
        let scan = {
            let mut store = store.lock().expect("file store not poisoned");
            store.materialize_at(&root, &node_sub_path)
        };

        for name in scan.lazy_subfolders {
            let child = match node_sub_path.as_str() {
                "" => name,
                _ => format!("{node_sub_path}/{name}"),
            };
            queue.push(child);
        }
    }

    // One final brief lock for the selection snapshot: everything
    // registered under the node across the whole walk.
    let selected = {
        let store = store.lock().expect("file store not poisoned");
        store.subtree_members(&root, sub_path)
    };
    Ok(selected)
}
