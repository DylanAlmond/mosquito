import type { SharedFile } from '../types';

export interface FolderGroup {
  /** Absolute folder root every member was added under. */
  folderRoot: string;
  /** The root's own display name (its last path segment). */
  name: string;
  /** Member files, in shared-list order. */
  files: SharedFile[];
}

export interface GroupedFiles {
  /** Files added directly; sit at the top of the list, ungrouped. */
  loose: SharedFile[];
  /** One group per distinct folder root, in first-appearance order. */
  groups: FolderGroup[];
}

/** Grouping is purely derived at render time (ADR-0001): the store stays flat. */
export function groupSharedFiles(files: SharedFile[]): GroupedFiles {
  const loose = files.filter((file) => file.provenance === null);
  const groups = new Map<string, FolderGroup>();

  for (const file of files) {
    const provenance = file.provenance;
    if (!provenance) continue;

    let group = groups.get(provenance.folder_root);
    if (!group) {
      const segments = provenance.folder_root.split(/[\\/]/).filter(Boolean);
      group = { folderRoot: provenance.folder_root, name: segments[segments.length - 1] ?? '', files: [] };
      groups.set(provenance.folder_root, group);
    }

    group.files.push(file);
  }

  return { loose, groups: [...groups.values()] };
}
