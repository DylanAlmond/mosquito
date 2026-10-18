import type { SharedFile } from '../types';

export interface FolderNode {
  /** The root's display name (its last path segment) for roots; the subfolder's own segment for nested nodes. */
  name: string;
  /** Absolute folder root every file under this node was added under. */
  folderRoot: string;
  /** Path below the folder root where this subfolder hangs; empty for root nodes. */
  subPath: string;
  /** Nesting depth: folder roots sit at depth 0, their subfolders at depth 1, and so on. */
  depth: number;
  /** Files stored directly in this folder, in shared-list order. */
  files: SharedFile[];
  /** Subfolders nested inside, in first-appearance order (add order). */
  children: FolderNode[];
}

export interface FolderTree {
  /** Files added directly; sit at the top of the list, ungrouped. */
  loose: SharedFile[];
  /** One root node per distinct folder root, in first-appearance order. */
  roots: FolderNode[];
}

/**
 * Derivation is purely render-time (ADR-0001/0002): the store stays flat
 * and the tree derives from each member's relative path. One definition
 * per language, mirrored by the listing's Rust derivation — kept equal
 * by tests, not by sharing code.
 */
export function buildFolderTree(files: SharedFile[]): FolderTree {
  const loose = files.filter((file) => file.provenance === null);

  const roots = new Map<string, FolderNode>();

  for (const file of files) {
    const provenance = file.provenance;
    if (!provenance) continue;

    let root = roots.get(provenance.folder_root);
    if (!root) {
      // Insertion order of `roots`/`children` is the first-appearance
      // order; files are handled before subfolders because children are
      // assembled in a second pass below.
      const segments = provenance.folder_root.split(/[\\/]/).filter(Boolean);
      root = {
        name: segments[segments.length - 1] ?? '',
        folderRoot: provenance.folder_root,
        subPath: '',
        depth: 0,
        files: [],
        children: []
      };
      roots.set(provenance.folder_root, root);
    }

    // The relative path's last segment is the file itself; everything
    // before it is the subfolder walk (empty for a directly-stored file).
    const segments = provenance.relative_path.split('/').filter(Boolean);
    placeInto(root, segments.slice(0, -1), file);

  }

  return { loose, roots: [...roots.values()] };
}

/// Drops `segments.length` levels below `node` (creating subfolders in
/// first-appearance order) and files the leaf. `Map` keeps child
/// insertion order while letting a repeated segment resolve to the
/// existing node.
function placeInto(node: FolderNode, segments: string[], file: SharedFile): void {
  let parent = node;
  let walked = '';

  for (const segment of segments) {
    walked = walked ? `${walked}/${segment}` : segment;

    let child = parent.children.find((existing) => existing.subPath === walked);
    if (!child) {
      child = {
        name: segment,
        folderRoot: parent.folderRoot,
        subPath: walked,
        depth: parent.depth + 1,
        files: [],
        children: []
      };
      parent.children.push(child);
    }
    parent = child;
  }

  parent.files.push(file);
}
