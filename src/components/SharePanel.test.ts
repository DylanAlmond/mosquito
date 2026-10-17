import { describe, expect, it, beforeEach, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { open } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { useSharer } from '../composables/useSharer';
import SharePanel from './SharePanel.vue';

vi.mock('@tauri-apps/plugin-dialog');
vi.mock('@tauri-apps/api/core');

vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent: vi.fn(async () => vi.fn()) })
}));

const SERVER_STOPPED = { running: false, url: null, port: null, lan_ip: null };
const SERVER_RUNNING = { running: true, url: 'http://10.0.0.2:7331', port: 7331, lan_ip: '10.0.0.2' };

describe('SharePanel folder picking', () => {
  beforeEach(async () => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation((async (command: string) => {
      switch (command) {
        case 'list_files':
          return [];
        case 'get_server_info':
          return SERVER_STOPPED;
        case 'add_files':
          return { added: [], failed: [] };
        case 'get_qr':
          return '';
        case 'start_server':
        case 'stop_server':
          return SERVER_RUNNING;
        default:
          throw new Error(`unexpected command ${command}`);
      }
    }) as unknown as typeof invoke);

    sharedFilesReset();
  });

  function sharedFilesReset() {
    const sharer = useSharer();
    sharer.info.value = SERVER_STOPPED;
    sharer.files.value = [];
    sharer.failedAdds.value = [];
  }

  it('Choose Folder opens a directory dialog and shares the picked folder', async () => {
    vi.mocked(open).mockResolvedValue(['C:/Users/me/Photos']);
    const wrapper = mount(SharePanel);

    await wrapper.find('.choose-folder').trigger('click');

    expect(open).toHaveBeenCalledWith(
      expect.objectContaining({ directory: true, multiple: true })
    );
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('add_files', {
      paths: ['C:/Users/me/Photos']
    });
  });

  it('the dropzone keeps opening the regular file dialog, not the directory one', async () => {
    vi.mocked(open).mockResolvedValue(null);
    const wrapper = mount(SharePanel);

    await wrapper.find('.dropzone').trigger('click');

    expect(open).toHaveBeenCalledWith(
      expect.objectContaining({ directory: false })
    );
  });

  it('while sharing, Choose Folder is distinct from the card that adds files', async () => {
    const sharer = useSharer();
    sharer.info.value = SERVER_RUNNING;
    sharer.qrSvg.value = '';

    const wrapper = mount(SharePanel);

    // The card opens the file dialog; the folder button must not run both handlers.
    await wrapper.find('.dropzone--active').trigger('click');
    expect(open).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledWith(expect.objectContaining({ directory: false }));

    // Folder button click while running still only opens the directory dialog.
    await wrapper.find('.choose-folder').trigger('click');
    expect(open).toHaveBeenCalledTimes(2);
    expect(open).toHaveBeenLastCalledWith(expect.objectContaining({ directory: true }));
  });
});
