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

  function mountList() {
    useSharer().files.value = sharedFiles;
    return mount(FileList);
  }

  function rowTexts(wrapper: ReturnType<typeof mountList>): string[] {
    return wrapper.findAll('.file-rows > li').map((row) => {
      const name = row.find('.file-name');
      return (name.exists() ? name : row.find('.folder-name')).text();
    });
  }

  function folderRow(wrapper: ReturnType<typeof mountList>, name: string) {
    return wrapper
      .findAll('.folder-row')
      .find((row) => row.find('.folder-name').text() === name)!;
  }

  it('renders loose files, then each folder root expanded with its direct files before its collapsed subfolders', () => {
    const wrapper = mountList();

    // Subfolder contents (vacation/beach.jpg) are not rendered collapsed.
    expect(rowTexts(wrapper)).toEqual([
      'notes.txt',
      'Photos',
      'beach.jpg',
      'vacation',
      'Docs',
      'a.txt'
    ]);

    // Roots start expanded, subfolders start collapsed.
    expect(folderRow(wrapper, 'Photos')!.find('[aria-expanded]').attributes('aria-expanded')).toBe(
      'true'
    );
    expect(folderRow(wrapper, 'vacation')!.find('[aria-expanded]').attributes('aria-expanded')).toBe(
      'false'
    );
    expect(rowTexts(wrapper)).not.toContain('vacation/beach.jpg');
  });

  it('toggles a subfolder through its chevron only — not by clicking the row or its name', async () => {
    const wrapper = mountList();

    // Row and name clicks must never change expansion (chevron-only).
    await folderRow(wrapper, 'vacation')!.trigger('click');
    await folderRow(wrapper, 'vacation')!.find('.folder-name').trigger('click');
    expect(rowTexts(wrapper)).not.toContain('vacation/beach.jpg');

    await folderRow(wrapper, 'vacation')!.find('[aria-label="Expand vacation"]').trigger('click');
    expect(rowTexts(wrapper)).toEqual([
      'notes.txt',
      'Photos',
      'beach.jpg',
      'vacation',
      'beach.jpg',
      'Docs',
      'a.txt'
    ]);
    expect(folderRow(wrapper, 'vacation')!.attributes('aria-expanded')).toBeUndefined();
    expect(
      folderRow(wrapper, 'vacation')!.find('[aria-label="Collapse vacation"]').attributes(
        'aria-expanded'
      )
    ).toBe('true');
  });

  it('checks a folder checkbox when only some descendants are selected — hidden members included', async () => {
    const wrapper = mountList();

    const photosRow = folderRow(wrapper, 'Photos')!;
    await photosRow.find('[aria-label="Select or deselect all files in Photos"]').trigger('click');
    await photosRow.find('[aria-label="Select or deselect all files in Photos"]').trigger('click');
    // Selecting twice sets everything, then deselects everything again:
    // subtree counts both the visible beach.jpg and the one folded away
    // inside the collapsed `vacation` subfolder.
    expect(photosRow.find('[aria-checked]').attributes('aria-checked')).toBe('false');

    await photosRow.find('[aria-label="Select or deselect all files in Photos"]').trigger('click');
    // ... full subtree now selected (aria-checked true).
    expect(photosRow.find('[aria-checked]').attributes('aria-checked')).toBe('true');

    // The visible member drops out; the hidden member still keeps
    // Photos part-selected, which is only visible as a "mixed" checkbox.
    const visibleBeach = wrapper
      .findAll('.file-row')
      .find((row) => row.text() === 'beach.jpg')!;
    await visibleBeach.trigger('click');
    expect(photosRow.find('[aria-checked]').attributes('aria-checked')).toBe('mixed');
  });

  it('a subfolder row acts on its own subtree only', async () => {
    const wrapper = mountList();

    // Expand `vacation`, then act on the subfolder itself.
    await folderRow(wrapper, 'vacation')!.find('[aria-label="Expand vacation"]').trigger('click');

    const vacationRow = folderRow(wrapper, 'vacation')!;
    await vacationRow
      .find('[aria-label="Select or deselect all files in vacation"]')
      .trigger('click');

    const selected = wrapper.findAll('.file-row.is-selected').map((row) => row.text());
    // Exactly one row is highlighted: the member inside `vacation`.
    // Note `Photos` shows the same basename for its other member; only
    // one of them is selected.
    expect(selected).toHaveLength(1);

    // The parent folder reflects a partial select: one of its two members.
    expect(
      folderRow(wrapper, 'Photos')!.find('[aria-checked]').attributes('aria-checked')
    ).toBe('mixed');
  });

  it('stamps tree rows with their depth tokens and keeps loose files flush (ADR-0004)', async () => {
    const wrapper = mountList();

    const findRow = (text: string, selector: string) =>
      wrapper
        .findAll('.file-rows > li')
        .find((row: any) => {
          const match = row.find(selector);
          return match.exists() && match.text() === text;
        })!;

    // Loose files carry no depth attributes: they stay flush.
    const loose = findRow('notes.txt', '.file-name');
    expect(loose.attributes('style')).toBeUndefined();
    expect(loose.attributes('data-depth')).toBeUndefined();

    // Folder roots indent at their own depth; the chevron token rides
    // the same row.
    const photos = findRow('Photos', '.folder-name');
    expect(photos.attributes('style')!.replace(/\s+/g, '')).toBe('--depth:0;');
    expect(photos.attributes('data-depth')).toBe('0');

    // Members one level under the folder storing them; the extra
    // chevron-slot catch-up lives in the stylesheet, not the token.
    const member = findRow('beach.jpg', '.file-name');
    expect(member.attributes('style')!.replace(/\s+/g, '')).toBe('--depth:1;');
    expect(member.attributes('data-depth')).toBe('1');
    // Subfolder rows carry their own depth.
    const vacation = findRow('vacation', '.folder-name');
    expect(vacation.attributes('style')!.replace(/\s+/g, '')).toBe('--depth:1;');
    expect(vacation.attributes('data-depth')).toBe('1');

    // Expanding the subfolder reveals its member at depth 2 (the first
    // `beach.jpg` stays Photos' depth-1 member).
    await vacation.find('[aria-label="Expand vacation"]').trigger('click');
    expect(
      wrapper
        .findAll('.file-row')
        .some(
          (row: any) =>
            row.find('.file-name').text() === 'beach.jpg' &&
            row.attributes('style')!.replace(/\s+/g, '') === '--depth:2;' &&
            row.attributes('data-depth') === '2'
        )
    ).toBe(true);
  });

  it('keeps collapse state when the store re-reads files after an action', async () => {
    const wrapper = mountList();

    await folderRow(wrapper, 'vacation')!.find('[aria-label="Expand vacation"]').trigger('click');

    // The next re-read lands the same members again (any action
    // triggers one; assigning is what every action ends with).
    useSharer().files.value = [...sharedFiles];

    // The expanded subfolder survives the re-read…
    expect(
      folderRow(wrapper, 'vacation')!.find('[aria-expanded]').attributes('aria-expanded')
    ).toBe('true');

    // …and so does the collapsed one.
    await folderRow(wrapper, 'vacation')!.find('[aria-label="Collapse vacation"]').trigger('click');
    useSharer().files.value = [...sharedFiles];
    expect(
      folderRow(wrapper, 'vacation')!.find('[aria-expanded]').attributes('aria-expanded')
    ).toBe('false');
  });

  it('delete selected removes loose file and every folder member it was grouped with', async () => {
    const wrapper = mountList();

    const docsRow = folderRow(wrapper, 'Docs')!;
    await docsRow.find('[aria-label="Select or deselect all files in Docs"]').trigger('click');

    const looseRow = wrapper.findAll('.file-row').find((row) => row.text() === 'notes.txt')!;
    await looseRow.trigger('click');

    await wrapper.find('[aria-label="Delete selected files"]').trigger('click');

    const removedIds = vi.mocked(invoke).mock.calls
      .filter(([command]) => command === 'remove_file')
      .map(([, args]) => (args as { id: number }).id);

    expect(removedIds.sort()).toEqual([2, 3]);
  });

  it('removing a folder\u2019s last member removes its folder row too', async () => {
    const wrapper = mountList();

    const docsRow = folderRow(wrapper, 'Docs')!;
    await docsRow.find('[aria-label="Select or deselect all files in Docs"]').trigger('click');
    await wrapper.find('[aria-label="Delete selected files"]').trigger('click');
    await wrapper.vm.$nextTick();
    await wrapper.vm.$nextTick();

    expect(folderRow(wrapper, 'Docs')).toBeUndefined();
    expect(rowTexts(wrapper)).toEqual(['notes.txt', 'Photos', 'beach.jpg', 'vacation']);
  });
});
