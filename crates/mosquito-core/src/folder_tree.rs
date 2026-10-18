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
    use serde::Deserialize;

    /// The one fixture both folder-tree mirrors' tests read (ADR-0002):
    /// this module and the TypeScript test in
    /// `src/lib/folderTree.test.ts` must each derive the fixture's
    /// outline from its input list exactly. The former hand-maintained
    /// case lists are replaced by it — every rule they pinned (loose
    /// files, files before subfolders, first-appearance ordering,
    /// arbitrary depth, duplicates at the same spot, same-named roots
    /// and subfolders, root naming splitting on either separator) is
    /// encoded in the fixture.
    const FIXTURE: &str = "test/fixtures/folder-tree/tree-input-and-outline.json";

    #[derive(Deserialize)]
    struct Fixture {
        #[allow(dead_code)]
        notes: Vec<String>,
        input: Vec<FixtureEntry>,
        outline: Outline,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureEntry {
        id: u64,
        name: String,
        path: String,
        size: u64,
        provenance: Option<FixtureProvenance>,
    }

    #[derive(Deserialize)]
    struct FixtureProvenance {
        folder_root: String,
        relative_path: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Outline {
        loose: Vec<u64>,
        roots: Vec<OutlineNode>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct OutlineNode {
        name: String,
        folder_root: String,
        sub_path: String,
        depth: usize,
        files: Vec<u64>,
        children: Vec<OutlineNode>,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/fixtures/folder-tree/tree-input-and-outline.json"
        )))
        .expect("the shared fixture must parse as JSON")
    }

    fn shared_files(fixture: &Fixture) -> Vec<SharedFile> {
        fixture
            .input
            .iter()
            .map(|entry| SharedFile {
                id: FileId(entry.id),
                name: entry.name.clone(),
                path: PathBuf::from(&entry.path),
                size: entry.size,
                provenance: entry.provenance.as_ref().map(|provenance| FolderProvenance {
                    folder_root: PathBuf::from(&provenance.folder_root),
                    relative_path: provenance.relative_path.clone(),
                }),
            })
            .collect()
    }

    fn ids(files: &[&SharedFile]) -> Vec<u64> {
        files.iter().map(|file| file.id.0).collect()
    }

    fn assert_node(node: &FolderNode, expected: &OutlineNode, spot: &str) {
        assert!(
            node.name == expected.name,
            "{FIXTURE}: {spot}: folder name {:?} disagrees with the fixture's {:?}",
            node.name,
            expected.name
        );
        assert!(
            node.folder_root == PathBuf::from(&expected.folder_root),
            "{FIXTURE}: {spot}: folder_root {:?} disagrees with the fixture's {:?}",
            node.folder_root,
            expected.folder_root
        );
        assert!(
            node.sub_path == expected.sub_path,
            "{FIXTURE}: {spot}: sub_path {:?} disagrees with the fixture's {:?}",
            node.sub_path,
            expected.sub_path
        );
        assert!(
            node.depth == expected.depth,
            "{FIXTURE}: {spot}: depth {} disagrees with the fixture's {}",
            node.depth,
            expected.depth
        );
        assert!(
            ids(&node.files) == expected.files,
            "{FIXTURE}: {spot}: files (ids in shared-list order) {:?} disagree with the fixture's {:?}",
            ids(&node.files),
            expected.files
        );
        let children: Vec<&str> = node.children.iter().map(|child| child.name.as_str()).collect();
        let expected_children: Vec<&str> =
            expected.children.iter().map(|child| child.name.as_str()).collect();
        assert!(
            children == expected_children,
            "{FIXTURE}: {spot}: children (first-appearance order) {children:?} disagree with the fixture's {expected_children:?}"
        );
        for (index, child) in expected.children.iter().enumerate() {
            assert_node(&node.children[index], child, &format!("{spot}/{}", child.name));
        }
    }

    #[test]
    fn derives_the_shared_fixture_input_into_exactly_the_shared_fixture_outline() {
        let fixture = fixture();
        let files = shared_files(&fixture);
        let tree = build_folder_tree(&files);

        let loose_ids: Vec<u64> = tree.loose.iter().map(|file| file.id.0).collect();
        assert!(
            loose_ids == fixture.outline.loose,
            "{FIXTURE}: loose-file ids {loose_ids:?} disagree with the fixture's {:?}",
            fixture.outline.loose
        );
        // The listed loose files must all actually be loose (no provenance).
        assert!(
            tree.loose.iter().all(|file| file.provenance.is_none()),
            "{FIXTURE}: a listed loose file carried provenance"
        );

        let root_names: Vec<&str> = tree.roots.iter().map(|root| root.name.as_str()).collect();
        let expected_names: Vec<&str> =
            fixture.outline.roots.iter().map(|root| root.name.as_str()).collect();
        assert!(
            root_names == expected_names,
            "{FIXTURE}: root names in first-appearance order {root_names:?} disagree with the fixture's {expected_names:?}"
        );
        for (index, expected) in fixture.outline.roots.iter().enumerate() {
            assert_node(&tree.roots[index], expected, &format!("roots[{index}]"));
        }
    }
}
