import { invoke } from '@tauri-apps/api/core';
import type { AddFilesOutcome, ServerInfo, SharedFile } from './types';

export const api = {
  addFiles: (paths: string[]) => invoke<AddFilesOutcome>('add_files', { paths }),
  removeFile: (id: number) => invoke<SharedFile>('remove_file', { id }),
  listFiles: () => invoke<SharedFile[]>('list_files'),
  startServer: () => invoke<ServerInfo>('start_server'),
  stopServer: () => invoke<void>('stop_server'),
  getServerInfo: () => invoke<ServerInfo>('get_server_info'),
  getQr: () => invoke<string>('get_qr')
};
