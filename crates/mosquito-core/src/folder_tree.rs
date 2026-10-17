//! The Rust mirror of the desktop folder-tree derivation
//! (`src/lib/folderTree.ts`): one concept, two definitions, kept equal
//! by tests on both sides — never by sharing code (ADR-0002).
//!
//! Pure and deterministic: insertion order everywhere, files before
//! subfolders at every level, first-appearance order for roots and
//! subfolders. The store stays flat (ADR-0001); this runs at render
//! time only.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::file_store::SharedFile;

pub struct FolderNode<'a> {
    /// The root's display name (its last path segment) for roots; the
    /// subfolder's own segment for nested nodes.
    pub name: String,
    /// Absolute folder root every file under this node was added under.
    pub folder_root: PathBuf,
    /// Path below the folder root where this subfolder hangs; empty for
    /// root nodes.
    pub sub_path: String,
    /// Nesting depth: folder roots sit at depth 0, their subfolders at
    /// depth 1, and so on.
    pub depth: usize,
    /// Files stored directly in this folder, in shared-list order.
    pub files: Vec<&'a SharedFile>,
    /// Subfolders nested inside, in first-appearance (add) order.
    pub children: Vec<FolderNode<'a>>,
}

pub struct FolderTree<'a> {
    /// Files added directly; sit at the top of the list, ungrouped.
    pub loose: Vec<&'a SharedFile>,
    /// One root node per distinct folder root, in first-appearance order.
    pub roots: Vec<FolderNode<'a>>,
}

/// Derive the tree at render time. Mirrors the TypeScript derivation
/// exactly: each member walks its relative path one segment at a time,
/// creating subfolders on first appearance and sharing them on repeat.
pub fn build_folder_tree(files: &[SharedFile]) -> FolderTree<'_> {
    /// Growable arena of plain nodes; assembled into borrowed
    /// `FolderNode`s afterwards. Mutable insertions otherwise fight the
    /// borrowed file references.
    struct Node {
        name: String,
        /// Set for every node: roots carry their own path, subfolders
        /// inherit it (matching the TypeScript mirror).
        folder_root: PathBuf,
        sub_path: String,
        depth: usize,
        /// Indices into `files` — the slice is untouched until assembly.
        files: Vec<usize>,
        children: Vec<usize>,
        /// Walked subfolder path -> arena index, for repeat visits.
        child_index: HashMap<String, usize>,
    }

    let mut arena: Vec<Node> = Vec::new();
    // Lossy root string -> arena index. Two distinct non-UTF-8 roots
    // would collapse; the desktop's string-keyed `folderRoot` grouping
    // is shaped the same, so parity keeps both pages aligned.
    let mut root_index: HashMap<String, usize> = HashMap::new();

    for (position, file) in files.iter().enumerate() {
        let Some(provenance) = &file.provenance else {
            continue;
        };

        let root_key = provenance.folder_root.to_string_lossy().into_owned();
        let root = match root_index.get(&root_key) {
            Some(&index) => index,
            None => {
                let name = root_key
                    .split(['/', '\\'])
                    .filter(|segment| !segment.is_empty())
                    .next_back()
                    .unwrap_or_default()
                    .to_string();
                let index = arena.len();
                arena.push(Node {
                    name,
                    folder_root: provenance.folder_root.clone(),
                    sub_path: String::new(),
                    depth: 0,
                    files: Vec::new(),
                    children: Vec::new(),
                    child_index: HashMap::new(),
                });
                root_index.insert(root_key, index);
                index
            }
        };

        // The relative path's last segment is the file itself; the walk
        // covers the folders above it (empty for a directly-stored file).
        let segments: Vec<&str> = provenance
            .relative_path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect();
        let mut current = root;
        let mut walked = String::new();
        for segment in segments.iter().take(segments.len().saturating_sub(1)) {
            if !walked.is_empty() {
                walked.push('/');
            }
            walked.push_str(segment);

            current = match arena[current].child_index.get(walked.as_str()) {
                Some(&index) => index,
                None => {
                    let index = arena.len();
                    arena.push(Node {
                        name: (*segment).to_string(),
                        // Subfolder nodes inherit the root's absolute
                        // path, like the TypeScript mirror's
                        // `folderRoot` field does.
                        folder_root: provenance.folder_root.clone(),
                        sub_path: walked.clone(),
                        depth: arena[current].depth + 1,
                        files: Vec::new(),
                        children: Vec::new(),
                        child_index: HashMap::new(),
                    });
                    arena[current].children.push(index);
                    arena[current].child_index.insert(walked.clone(), index);
                    index
                }
            };
        }
        arena[current].files.push(position);
    }

    fn assemble<'a>(arena: &[Node], index: usize, files: &'a [SharedFile]) -> FolderNode<'a> {
        let node = &arena[index];
        FolderNode {
            name: node.name.clone(),
            folder_root: node.folder_root.clone(),
            sub_path: node.sub_path.clone(),
            depth: node.depth,
            files: node
                .files
                .iter()
                .map(|&position| &files[position])
                .collect(),
            children: node
                .children
                .iter()
                .map(|&child| assemble(arena, child, files))
                .collect(),
        }
    }

    let roots = (0..arena.len())
        .filter(|&index| arena[index].depth == 0)
        .map(|index| assemble(&arena, index, files))
        .collect();

    FolderTree {
        loose: files
            .iter()
            .filter(|file| file.provenance.is_none())
            .collect(),
        roots,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_store::{FileId, FolderProvenance};

    fn shared_file(id: u64, name: &str, provenance: Option<FolderProvenance>) -> SharedFile {
        SharedFile {
            id: FileId(id),
            name: name.into(),
            path: PathBuf::from(format!("C:\\nowhere\\{name}")),
            size: 1,
            provenance,
        }
    }

    fn loose(id: u64, name: &str) -> SharedFile {
        shared_file(id, name, None)
    }

    fn member(id: u64, root: &str, relative_path: &str) -> SharedFile {
        shared_file(
            id,
            relative_path.rsplit('/').next().unwrap_or(relative_path),
            Some(FolderProvenance {
                folder_root: PathBuf::from(root),
                relative_path: relative_path.into(),
            }),
        )
    }

    fn ids(files: &[&SharedFile]) -> Vec<u64> {
        files.iter().map(|file| file.id.0).collect()
    }

    fn child<'a>(node: &'a FolderNode<'a>, name: &str) -> &'a FolderNode<'a> {
        node.children
            .iter()
            .find(|child| child.name == name)
            .unwrap()
    }

    #[test]
    fn keeps_loose_files_out_of_any_folder_node() {
        let files = vec![
            member(1, "C:/Photos", "beach.jpg"),
            loose(2, "notes.txt"),
            member(3, "C:/Photos", "a.txt"),
        ];

        let tree = build_folder_tree(&files);

        assert_eq!(ids(&tree.loose), [2]);
        // The loose file never lands in a folder node.
        assert!(
            tree.roots
                .iter()
                .all(|root| !root.files.iter().any(|file| file.id.0 == 2))
        );
    }

    #[test]
    fn orders_roots_by_first_appearance_and_holds_files_before_subfolders() {
        let files = vec![
            member(1, "C:/Photos", "travel/beach.jpg"),
            member(2, "C:/Photos", "sunrise.jpg"),
            member(3, "C:/Docs", "a.txt"),
            member(4, "C:/Photos", "notes/todo.txt"),
        ];

        let tree = build_folder_tree(&files);

        let photos = &tree.roots[0];
        assert_eq!(photos.name, "Photos");
        assert_eq!(photos.depth, 0);
        // Directly-stored member first, subfolder children after.
        assert_eq!(ids(&photos.files), [2]);
        assert_eq!(
            photos
                .children
                .iter()
                .map(|child| child.name.as_str())
                .collect::<Vec<_>>(),
            ["travel", "notes"]
        );
        assert_eq!(ids(&child(photos, "travel").files), [1]);
        // `Photos` appears before `Docs` because its first member came first.
        assert_eq!(tree.roots[1].name, "Docs");
    }

    #[test]
    fn unwraps_arbitrarily_deep_nesting_with_depths() {
        let files = vec![member(1, "C:/Photos", "travel/beach/sunset.jpg")];

        let tree = build_folder_tree(&files);

        let root = &tree.roots[0];
        assert!(root.files.is_empty());
        let beach = &child(&child(root, "travel"), "beach");
        assert_eq!(beach.sub_path, "travel/beach");
        assert_eq!(beach.depth, 2);
        // Subfolder nodes inherit the root's absolute path.
        assert_eq!(beach.folder_root, root.folder_root);
        assert_eq!(ids(&beach.files), [1]);
    }

    #[test]
    fn lists_subfolders_in_first_appearance_order_not_alphabetically() {
        let files = vec![
            member(1, "C:/Photos", "zebra/file-a.txt"),
            member(2, "C:/Photos", "alpha/file-b.txt"),
            member(3, "C:/Photos", "zebra/file-c.txt"),
        ];

        let tree = build_folder_tree(&files);

        let root = &tree.roots[0];
        assert_eq!(
            root.children
                .iter()
                .map(|child| child.name.as_str())
                .collect::<Vec<_>>(),
            ["zebra", "alpha"]
        );
        assert_eq!(ids(&child(root, "zebra").files), [1, 3]);
    }

    #[test]
    fn routes_same_named_subfolders_to_their_own_roots() {
        let files = vec![
            member(1, "C:/one/Photos", "a/beach.jpg"),
            member(2, "D:/two/Photos", "b/a/beach.jpg"),
            member(3, "D:/two/Photos", "beach.jpg"),
        ];

        let tree = build_folder_tree(&files);

        assert_eq!(tree.roots.len(), 2);
        assert_eq!(
            tree.roots
                .iter()
                .map(|root| root.name.as_str())
                .collect::<Vec<_>>(),
            ["Photos", "Photos"]
        );
        assert_eq!(child(&tree.roots[0], "a").files.len(), 1);
        assert_eq!(ids(&child(&child(&tree.roots[1], "b"), "a").files), [2]);
        assert_eq!(ids(&tree.roots[1].files), [3]);
    }

    #[test]
    fn keeps_a_file_added_twice_as_two_entries_at_the_same_spot() {
        let files = vec![
            member(1, "C:/Photos", "beach.jpg"),
            member(2, "C:/Photos", "beach.jpg"),
        ];

        let tree = build_folder_tree(&files);

        assert_eq!(ids(&tree.roots[0].files), [1, 2]);
        assert!(tree.roots[0].children.is_empty());
    }
}
