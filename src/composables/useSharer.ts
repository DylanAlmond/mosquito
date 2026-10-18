import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type {
  FailedAdd,
  ImportRunSnapshot,
  ImportStartOutcome,
  RootRemoval,
  RootRow,
  RootRun,
  RootRunStatus,
  ServerInfo,
  SharedFile,
  TopListing
} from '../types';

const NOT_RUNNING: ServerInfo = { running: false, url: null, port: null, lan_ip: null };

const loose = ref<SharedFile[]>([]);
const roots = ref<RootRow[]>([]);
const info = ref<ServerInfo>({ ...NOT_RUNNING });
const qrSvg = ref<string | null>(null);
const failedAdds = ref<FailedAdd[]>([]);
const error = ref<string | null>(null);
const busy = ref(false);
/// Live import run snapshots (ADR-0007). Only refreshed while roots are
/// counting; settled runs are dropped once nothing is in flight anymore.
const runs = ref<ImportRunSnapshot[]>([]);

/// Bumped on every re-read of the store. Components holding paged
/// feeds watch it and re-fetch their pages, so the list can't drift
/// from the store after any action.
const contentVersion = ref(0);

/// Progress polling cadence. ADR-0007 leaves the desktop bridge open:
/// snapshots are queryable, and the app already re-reads everything after
/// every action ("divergence between UI and store is impossible"), so the
/// board polls `import_runs` instead of introducing an event channel.
/// Cheap local reads; the walk itself only reports every 500 counted
/// files, so anything finer is invisible anyway.
const POLL_MS = 250;

/// Root-record failures already handed to the failed-add banner. The
/// registry keeps settled runs listed forever, so each run-root failure
/// is reported only ever once: a later drop dismisses the banner without
/// resurrecting the old failure above the new outcome.
const reportedFailures = new Set<string>();

let pollTimer: ReturnType<typeof setInterval> | null = null;

function statusName(status: RootRunStatus): string {
  return typeof status === 'string' ? status : 'Failed';
}

/// The roots of a run still walking — what the progress board renders.
export function countingRoots(run: ImportRunSnapshot): RootRun[] {
  return run.roots.filter((root) => statusName(root.status) === 'Counting');
}

function stopPolling() {
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

/// Same as the internal stop, exported for full teardown of the
/// module-scoped singleton (test resets).
export function stopRunPolling() {
  stopPolling();
}

/// Re-read the derived top listing and the server state. Cheap (local
/// IPC, tiny lists) and it makes divergence between UI and store
/// impossible. The tree itself is derived by the backend per request, so
/// the UI never derives folder structure any more (ADR-0006).
async function refresh() {
  const top = await invoke<TopListing>('list_top');
  loose.value = top.loose;
  roots.value = top.roots;
  info.value = await invoke<ServerInfo>('get_server_info');
  qrSvg.value = info.value.running ? await invoke<string>('get_qr') : null;
  contentVersion.value += 1;
}

/// The counting tick's lighter read (ADR-0008): the top listing plus a
/// pure server-info read, refreshed while any root's walk is still
/// running — records commit at drop, so their rows and their climbing
/// counts surface from here. `contentVersion` deliberately stays put:
/// count updates register nothing, so expanded paged nodes have no
/// reason to refetch; settled runs and actions re-truth them through
/// [`refresh`].
async function refreshTop() {
  const top = await invoke<TopListing>('list_top');
  loose.value = top.loose;
  roots.value = top.roots;
  info.value = await invoke<ServerInfo>('get_server_info');
  // First time the drop's auto-start shows through, hand the share panel
  // its QR — the settle refresh would otherwise hold it until the walk
  // ends. Runs once per rise, so the tick stays bounded and cheap.
  if (info.value.running && qrSvg.value === null) {
    qrSvg.value = await invoke<string>('get_qr');
  }
}

function startPolling() {
  if (pollTimer) return;
  pollTimer = setInterval(() => {
    void pollRuns().catch((e: unknown) => {
      // Fold the board away rather than freezing it on stale counts;
      // the error banner carries what went wrong.
      error.value = String(e);
      stopPolling();
      runs.value = [];
    });
  }, POLL_MS);
}

/// One tick of run observation: refresh snapshots, surface root failures
/// through the failed-add banner (the existing import-scoped flow), and
/// while anything is still counting, re-read the top listing so the
/// committed-at-drop rows show up and their live counts climb (ADR-0008).
/// When nothing is counting anymore, fold the run board away and do the
/// full refresh — settled runs, and the QR once the share is definitely
/// up (it went up at drop).
async function pollRuns() {
  runs.value = await invoke<ImportRunSnapshot[]>('import_runs');

  for (const run of runs.value) {
    for (const root of run.roots) {
      if (typeof root.status === 'string') continue;

      const key = `${run.run_id}:${root.path}`;
      if (reportedFailures.has(key)) continue;

      reportedFailures.add(key);
      failedAdds.value = [...failedAdds.value, { path: root.path, error: root.status.Failed }];
    }
  }

  if (runs.value.some((run) => countingRoots(run).length > 0)) {
    startPolling();
    await refreshTop();
    return;
  }

  stopPolling();
  runs.value = [];
  await refresh();
}

export function useSharer() {
  /// Share content present? Loose files count, and so does a root
  /// record alone — a lazily-known import is shareable before anything
  /// under it is registered.
  const hasContent = computed(() => loose.value.length > 0 || roots.value.length > 0);

  /// Every action funnels through here
  async function guard(fn: () => Promise<void>) {
    error.value = null;
    busy.value = true;

    try {
      await fn();
    } catch (e) {
      error.value = String(e);
    } finally {
      busy.value = false;
    }
  }

  async function init() {
    await guard(refresh);
  }

  /// Import many paths: loose files register immediately; folders commit
  /// their root records at drop (ADR-0008) and the backend starts the
  /// server in that same synchronous path — the walk only keeps the
  /// records' counts live, its progress showing on the board.
  async function addPaths(paths: string[]) {
    await guard(async () => {
      failedAdds.value = [];
      // (reportedFailures deliberately survives: an earlier failed run
      // must not resurface over this new outcome.)

      const outcome = await invoke<ImportStartOutcome>('import_paths', { paths });
      failedAdds.value = outcome.immediate.failed;

      if (outcome.run_id !== null) await pollRuns();

      // Loose adds are still the frontend's to start (the backend only
      // auto-starts on a folder drop, ADR-0008); a folder drop's server
      // is already listening, so this guard skips it — info is current
      // because pollRuns' ticks re-read the server state.
      if (outcome.immediate.added.length > 0 && !info.value.running) {
        await invoke<ServerInfo>('start_server');
      }

      await refresh();
    });
  }

  /// Ask a run to stop. The outcome contract is removal (ADR-0008):
  /// counting roots have their committed-at-drop records removed,
  /// everything materialized under them with them; roots already
  /// committed stay.
  async function cancelRun(runId: number) {
    await guard(async () => {
      await invoke<boolean>('cancel_import', { runId });
      await pollRuns();
    });
  }

  async function remove(id: number) {
    await guard(async () => {
      await invoke<SharedFile>('remove_file', { id });
      // The backend stops the server itself when nothing is shared
      // anymore — no files and no root records (the frontend can't see
      // in-flight imports, so it must never veto that rule).
      await refresh();
    });
  }

  /// Remove one whole root record: the record plus every materialized
  /// member in one action, then stop the server if nothing is left.
  async function removeRoot(rootId: number) {
    await guard(async () => {
      await invoke<RootRemoval>('remove_root', { rootId });
      await refresh();
    });
  }

  /// One delete action for a mixed selection: whole root groups go
  /// through `remove_root` (record plus every materialized member),
  /// single file ids through `remove_file`. The caller pre-splits the
  /// selection: member ids under a removed group are already swept by
  /// the group's own removal and must not be removed twice.
  async function removeSelection(ids: number[], rootIds: number[]) {
    await guard(async () => {
      for (const rootId of rootIds) {
        await invoke<RootRemoval>('remove_root', { rootId });
      }
      for (const id of ids) {
        await invoke<SharedFile>('remove_file', { id });
      }
      await refresh();
    });
  }

  async function stop() {
    await guard(async () => {
      await invoke<void>('stop_server');
      await refresh();
    });
  }

  function dismissFailed() {
    failedAdds.value = [];
  }

  return {
    loose,
    roots,
    hasContent,
    contentVersion,
    info,
    qrSvg,
    failedAdds,
    error,
    busy,
    runs,
    init,
    addPaths,
    cancelRun,
    remove,
    removeRoot,
    removeSelection,
    stop,
    dismissFailed
  };
}
