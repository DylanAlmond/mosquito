import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type {
  FailedAdd,
  ImportRunSnapshot,
  ImportStartOutcome,
  RootRun,
  RootRunStatus,
  ServerInfo,
  SharedFile
} from '../types';

const NOT_RUNNING: ServerInfo = { running: false, url: null, port: null, lan_ip: null };

/// Progress polling cadence. ADR-0007 leaves the desktop bridge open:
/// snapshots are queryable, and the app already re-reads everything after
/// every action ("divergence between UI and store is impossible"), so the
/// board polls `import_runs` instead of introducing an event channel.
/// Cheap local reads; the walk itself only reports every 500 counted
/// files, so anything finer is invisible anyway.
const POLL_MS = 250;

const files = ref<SharedFile[]>([]);
const info = ref<ServerInfo>({ ...NOT_RUNNING });
const qrSvg = ref<string | null>(null);
const failedAdds = ref<FailedAdd[]>([]);
const error = ref<string | null>(null);
const busy = ref(false);
/// Live import run snapshots (ADR-0007). Only refreshed while roots are
/// counting; settled runs are dropped once nothing is in flight anymore.
const runs = ref<ImportRunSnapshot[]>([]);

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

/// Re-read everything from the backend. Cheap (local IPC, tiny lists)
/// and it makes divergence between UI and store impossible.
async function refresh() {
  files.value = await invoke<SharedFile[]>('list_files');
  info.value = await invoke<ServerInfo>('get_server_info');
  qrSvg.value = info.value.running ? await invoke<string>('get_qr') : null;
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
/// when nothing is counting anymore, fold the run board away and refresh
/// the share view — committed root records and a server the backend
/// auto-started on commit only surface from here.
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
    return;
  }

  stopPolling();
  runs.value = [];
  await refresh();
}

export function useSharer() {
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

  async function addPaths(paths: string[]) {
    await guard(async () => {
      failedAdds.value = [];
      // (reportedFailures deliberately survives: an earlier failed run
      // must not resurface over this new outcome.)

      // Loose files register here and now; folder roots start a
      // background import run whose progress is shown on the board.
      const outcome = await invoke<ImportStartOutcome>('import_paths', { paths });
      failedAdds.value = outcome.immediate.failed;

      if (outcome.run_id !== null) await pollRuns();

      // Product rule: the server runs whenever there's something to share.
      // Dropping files IS the start button. Folder-only drops wait for the
      // backend to auto-start the server when the first root commits.
      if (outcome.immediate.added.length > 0 && !info.value.running) {
        await invoke<ServerInfo>('start_server');
      }

      await refresh();
    });
  }

  /// Ask a run to stop. Roots already committed stay committed; roots
  /// still counting leave no record (ADR-0007).
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

  async function removeMany(ids: number[]) {
    await guard(async () => {
      const idsToRemove = new Set(ids);
      const filesToRemove = files.value.filter((file) => idsToRemove.has(file.id));

      for (const file of filesToRemove) {
        await invoke<SharedFile>('remove_file', { id: file.id });
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
    files,
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
    removeMany,
    stop,
    dismissFailed
  };
}
