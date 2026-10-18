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

/** One top-level row: a collapsed, lazily-known root record (ADR-0005). */
export interface RootRow {
  id: number;
  name: string;
  /** Disambiguation for same-named roots (`Photos`, `Photos-2`, …). UI-held state keys on this. */
  key: string;
  /** Files counted at the import — lazily-known contents may have drifted since. */
  file_count: number;
}

export interface TopListing {
  loose: SharedFile[];
  roots: RootRow[];
}

/** One derived row of a node's page: a materialized file or a
    lazily-known subfolder. */
export type NodePageRow =
  | { kind: 'file'; file: SharedFile }
  | { kind: 'lazy_folder'; name: string };

/** One request-scoped page of a node's rows (ADR-0006). */
export interface NodePage {
  root_id: number;
  sub_path: string;
  offset: number;
  rows: NodePageRow[];
  /** Materialization failures of this node — inline markers, not import failures. */
  failures: FailedAdd[];
  total: number;
  has_more: boolean;
}

/** What removing a root record took with it (record plus materialized members). */
export interface RootRemoval {
  record: { id: number; path: string; file_count: number };
  files: SharedFile[];
}
