import { describe, expect, it, beforeEach, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import type { SharedFile } from '../types';
import { useSharer } from '../composables/useSharer';
import { invoke } from '@tauri-apps/api/core';
import FileList from './FileList.vue';

vi.mock('@tauri-apps/api/core');

const SERVER_STOPPED = { running: false, url: null, port: null, lan_ip: null };

let sharedFiles: SharedFile[] = [];

const PHOTOS = 'C:/Users/me/Photos';
const DOCS = 'D:/Stuff/Docs';

function loose(id: number, name: string): SharedFile {
  return { id, name, path: `C:/top/${name}`, size: 7, provenance: null };
}

function member(id: number, relativePath: string, folderRoot: string): SharedFile {
  return {
    id,
    name: relativePath.split('/').pop()!,
    path: `${folderRoot}/${relativePath}`,
    size: 7,
    provenance: { folder_root: folderRoot, relative_path: relativePath }
  };
}

describe('FileList', () => {
  beforeEach(async () => {
    sharedFiles = [
      member(1, 'vacation/beach.jpg', PHOTOS),
      loose(2, 'notes.txt'),
      member(3, 'a.txt', DOCS),
      member(4, 'beach.jpg', PHOTOS)
    ];

    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation((async (command: string, args?: Record<string, unknown>) => {
      switch (command) {
        case 'list_files':
          return sharedFiles;
        case 'get_server_info':
          return SERVER_STOPPED;
        case 'get_qr':
          return '';
        case 'remove_file': {
          const removed = sharedFiles.find((f) => f.id === args?.id);
          sharedFiles = sharedFiles.filter((f) => f.id !== args?.id);
          return removed;
        }
        case 'start_server':
        case 'stop_server':
          return null;
        default:
          throw new Error(`unexpected command ${command}`);
      }
    }) as unknown as typeof invoke);

    // The composable's refs are module-level singletons; drain them between tests.
    sharedFiles = [];
    await useSharer().init();
    useSharer().files.value = [];
    sharedFiles = [
      member(1, 'vacation/beach.jpg', PHOTOS),
      loose(2, 'notes.txt'),
      member(3, 'a.txt', DOCS),
      member(4, 'beach.jpg', PHOTOS)
    ];
  });

  it('renders loose files first, then one header per folder root with its members', () => {
    useSharer().files.value = sharedFiles;
    const wrapper = mount(FileList);

    const rows = wrapper.findAll('.file-rows > li');
    const names = rows.map((row) => {
      const name = row.find('.file-name');
      return (name.exists() ? name : row.find('.group-name')).text();
    });
    const headers = rows.filter((row) => row.classes().includes('group-header'));

    expect(names).toEqual([
      'notes.txt',
      'Photos',
      'vacation/beach.jpg',
      'beach.jpg',
      'Docs',
      'a.txt'
    ]);
    expect(headers).toHaveLength(2);
    expect(headers.map((h) => h.find('.group-name').text())).toEqual(['Photos', 'Docs']);
  });

  it('group header checkbox selects and deselects all of that folder\u2019s members', async () => {
    useSharer().files.value = sharedFiles;
    const wrapper = mount(FileList);

    const photosHeader = wrapper
      .findAll('.group-header')
      .find((row) => row.find('.group-name').text() === 'Photos')!;

    await photosHeader.find('.checkbox-control').trigger('click');

    expect(
      wrapper.findAll('.file-row').map((row) => row.classes().includes('is-selected'))
    ).toEqual([false, true, true, false]);

    await photosHeader.find('.checkbox-control').trigger('click');

    expect(wrapper.findAll('.file-row.is-selected')).toHaveLength(0);
  });

  it('group header checkbox reads mixed when only part of the group is selected', async () => {
    useSharer().files.value = sharedFiles;
    const wrapper = mount(FileList);

    const photosHeader = wrapper
      .findAll('.group-header')
      .find((row) => row.find('.group-name').text() === 'Photos')!;

    await photosHeader.find('.checkbox-control').trigger('click');
    const memberRow = wrapper.findAll('.file-row').find((row) => row.text() === 'beach.jpg')!;
    await memberRow.trigger('click'); // row click toggles the member off

    expect(photosHeader.find('.checkbox-control').attributes('aria-checked')).toBe('mixed');
  });

  it('delete selected removes loose file and every folder member it was grouped with', async () => {
    useSharer().files.value = sharedFiles;
    const wrapper = mount(FileList);

    const docsHeader = wrapper
      .findAll('.group-header')
      .find((row) => row.find('.group-name').text() === 'Docs')!;
    await docsHeader.find('.checkbox-control').trigger('click');

    const looseRow = wrapper.findAll('.file-row').find((row) => row.text() === 'notes.txt')!;
    await looseRow.trigger('click');

    await wrapper.find('[aria-label="Delete selected files"]').trigger('click');

    const removedIds = vi.mocked(invoke).mock.calls
      .filter(([command]) => command === 'remove_file')
      .map(([, args]) => (args as { id: number }).id);

    expect(removedIds.sort()).toEqual([2, 3]);
  });

  it('removing a folder\u2019s last member removes its header too', async () => {
    useSharer().files.value = sharedFiles;
    const wrapper = mount(FileList);

    const docsHeader = wrapper
      .findAll('.group-header')
      .find((row) => row.find('.group-name').text() === 'Docs')!;
    await docsHeader.find('.checkbox-control').trigger('click');
    await wrapper.find('[aria-label="Delete selected files"]').trigger('click');
    await wrapper.vm.$nextTick();
    await wrapper.vm.$nextTick();

    const headers = wrapper.findAll('.group-header');
    expect(headers.map((h) => h.find('.group-name').text())).toEqual(['Photos']);
    expect(wrapper.findAll('.file-row').map((r) => r.text())).toEqual([
      'notes.txt',
      'vacation/beach.jpg',
      'beach.jpg'
    ]);
  });
});
