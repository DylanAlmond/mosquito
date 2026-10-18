export interface FolderProvenance {
  /** Absolute path of the folder root this file was added under. */
  folder_root: string;
  /** Path below the folder root, forward-slash-joined (`sub/b.txt`). */
  relative_path: string;
}

export interface SharedFile {
  id: number;
  name: string;
  path: string;
  size: number;
  /** Where this file hangs in a folder, when it was added as part of one. Null for files added directly. */
  provenance: FolderProvenance | null;
}

export interface FailedAdd {
  path: string;
  error: string;
}

export interface AddFilesOutcome {
  added: SharedFile[];
  failed: FailedAdd[];
}

export type RootRunStatus = 'Counting' | 'Committed' | 'Cancelled' | { Failed: string };

export interface RootRun {
  /** Absolute path of the folder being counted. */
  path: string;
  /** Files counted so far. */
  files: number;
  status: RootRunStatus;
}

export interface ImportRunSnapshot {
  run_id: number;
  roots: RootRun[];
}

export interface ImportStartOutcome {
  /** The background run counting the drop's folders, or null for loose files only. */
  run_id: number | null;
  immediate: AddFilesOutcome;
}

export interface ServerInfo {
  running: boolean;
  url: string | null;
  port: number | null;
  lan_ip: string | null;
}
