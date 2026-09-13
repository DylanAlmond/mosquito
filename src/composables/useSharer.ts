import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { AddFilesOutcome, FailedAdd, ServerInfo, SharedFile } from '../types';

const NOT_RUNNING: ServerInfo = { running: false, url: null, port: null, lan_ip: null };

const files = ref<SharedFile[]>([]);
const info = ref<ServerInfo>({ ...NOT_RUNNING });
const qrSvg = ref<string | null>(null);
const failedAdds = ref<FailedAdd[]>([]);
const error = ref<string | null>(null);
const busy = ref(false);

export function useSharer() {
  /// Re-read everything from the backend. Cheap (local IPC, tiny lists)
  /// and it makes divergence between UI and store impossible.
  async function refresh() {
    files.value = await invoke<SharedFile[]>('list_files');
    info.value = await invoke<ServerInfo>('get_server_info');
    qrSvg.value = info.value.running ? await invoke<string>('get_qr') : null;
  }

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

      const outcome = await invoke<AddFilesOutcome>('add_files', { paths });
      failedAdds.value = outcome.failed;

      // Product rule: the server runs whenever there's something to share.
      // Dropping files IS the start button.
      if (outcome.added.length > 0 && !info.value.running) {
        await invoke<ServerInfo>('start_server');
      }

      await refresh();
    });
  }

  async function remove(id: number) {
    await guard(async () => {
      await invoke<SharedFile>('remove_file', { id });

      if (files.value.length === 1 && info.value.running) {
        await invoke<void>('stop_server'); // last file gone → nothing to share
      }

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

      if (filesToRemove.length === files.value.length && info.value.running) {
        await invoke<void>('stop_server');
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
    init,
    addPaths,
    remove,
    removeMany,
    stop,
    dismissFailed
  };
}
