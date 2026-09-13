import { ref } from 'vue';
import { api } from '../api';
import type { FailedAdd, ServerInfo, SharedFile } from '../types';

const NOT_RUNNING: ServerInfo = { running: false, url: null, port: null, lan_ip: null };

export function useSharer() {
  const files = ref<SharedFile[]>([]);
  const info = ref<ServerInfo>({ ...NOT_RUNNING });
  const qrSvg = ref<string | null>(null);
  const failedAdds = ref<FailedAdd[]>([]);
  const error = ref<string | null>(null);
  const busy = ref(false);

  /// Re-read everything from the backend. Cheap (local IPC, tiny lists)
  /// and it makes divergence between UI and store impossible.
  async function refresh() {
    files.value = await api.listFiles();
    info.value = await api.getServerInfo();
    qrSvg.value = info.value.running ? await api.getQr() : null;
  }

  /// Every action funnels through here: clear the error, run, catch.
  async function guard(fn: () => Promise<void>) {
    error.value = null;
    busy.value = true;
    try {
      await fn();
    } catch (e) {
      error.value = String(e); // the Rust error message, via IPC
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
      const outcome = await api.addFiles(paths);
      failedAdds.value = outcome.failed;
      // Product rule: the server runs whenever there's something to share.
      // Dropping files IS the start button.
      if (outcome.added.length > 0 && !info.value.running) {
        await api.startServer();
      }
      await refresh();
    });
  }

  async function remove(id: number) {
    await guard(async () => {
      await api.removeFile(id);
      if (files.value.length === 1 && info.value.running) {
        await api.stopServer(); // last file gone → nothing to share
      }
      await refresh();
    });
  }

  async function stop() {
    await guard(async () => {
      await api.stopServer();
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
    stop,
    dismissFailed
  };
}
