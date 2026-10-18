import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import type { FailedAdd, NodePageRow, SharedFile, TopListing } from '../types';
import { useSharer } from '../composables/useSharer';
import { invoke } from '@tauri-apps/api/core';
import FileList from './FileList.vue';

vi.mock('@tauri-apps/api/core');

const SERVER_STOPPED = { running: false, url: null, port: null, lan_ip: null };

/// The mirrored page budget (ADR-0006) — the component fetches within it.
const PAGE_SIZE = 200;

// -- fake backend: mirrors listing_pages.rs' derivation rules -------------

interface RootFixture {
  id: number;
  name: string;
  key: string;
  file_count: number;
}

let loose: SharedFile[];
let rootRecords: RootFixture[];
/** Registered membership per root record, exactly what import committed. */
let rootFiles: Record<number, SharedFile[]>;
/** Lazily-known directories that hold no registered file of their own. */
let emptyDirs: Record<number, Record<string, string[]>>;
/** Node-scoped materialization failures (`${rootId}@${subPath}`). */
let pageFailures: Record<string, FailedAdd[]>;

const dirOf = (relativePath: string) => {
  const cut = relativePath.lastIndexOf('/');
  return cut === -1 ? '' : relativePath.slice(0, cut);
};

const byFile = (a: { name: string }, b: { name: string }) =>
  a.name.toLowerCase().localeCompare(b.name.toLowerCase());
const byName = (a: string, b: string) => a.toLowerCase().localeCompare(b.toLowerCase());

function nodeRows(rootId: number, subPath: string): NodePageRow[] {
  const files = (rootFiles[rootId] ?? [])
    .filter((file) => dirOf(file.provenance!.relative_path) === subPath)
    .sort(byFile);
  const sub = subPath ? `${subPath}/` : '';
  const folders = [
    ...(rootFiles[rootId] ?? []).flatMap((file) => {
      const rest = file.provenance!.relative_path.slice(sub.length);
      return rest.includes('/') ? [rest.slice(0, rest.indexOf('/'))] : [];
    }),
    ...(emptyDirs[rootId]?.[subPath] ?? [])
  ]
    .filter((name, index, all) => all.indexOf(name) === index)
    .sort(byName);

  // The core's page rule: file rows name-sorted, then lazy folder rows
  // name-sorted (listing_pages.rs).
  return [
    ...files.map((file) => ({ kind: 'file', file }) as NodePageRow),
    ...folders.map((name) => ({ kind: 'lazy_folder', name }) as NodePageRow)
  ];
}

// -- fixtures ---------------------------------------------------------------

function looseFile(id: number, name: string): SharedFile {
  return { id, name, path: `C:/top/${name}`, size: 7, provenance: null };
}

function member(id: number, folderRoot: string, relativePath: string): SharedFile {
  return {
    id,
    name: relativePath.split('/').pop()!,
    path: `${folderRoot}/${relativePath}`,
    size: 7,
    provenance: { folder_root: folderRoot, relative_path: relativePath }
  };
}

/** One root record: display name is the folder's basename; `key` is the
    store's disambiguation of it (`Photos`, `Photos-2`, …). */
function record(
  id: number,
  folderRoot: string,
  files: SharedFile[],
  key = folderRoot.split('/').pop()!
): RootFixture {
  const name = folderRoot.split('/').pop()!;
  return { id, name, key, file_count: files.length };
}

const PHOTOS = 'C:/Users/me/Photos';
const DOCS = 'D:/Stuff/Docs';

/** Standard three-root board: one loose file, a nesting root, a flat one. */
function standardBoard() {
  const photosFiles = [
    member(21, PHOTOS, 'beach.jpg'),
    member(22, PHOTOS, 'sunsets/shore.jpg'),
    member(23, PHOTOS, 'sunsets/dune.jpg')
  ];
  loose = [looseFile(2, 'notes.txt')];
  rootRecords = [record(10, PHOTOS, photosFiles), record(11, DOCS, [member(31, DOCS, 'a.txt')])];
  rootFiles = { 10: photosFiles, 11: [member(31, DOCS, 'a.txt')] };
  emptyDirs = {};
  pageFailures = {};
}

/// 250 top-level members: the first page fills, the tail waits for
/// fetch-ahead.
function bigBoard() {
  const files = Array.from({ length: 250 }, (_, index) =>
    member(1000 + index, PHOTOS, `row-${String(index).padStart(3, '0')}.txt`)
  );
  loose = [];
  rootRecords = [record(10, PHOTOS, files)];
  rootFiles = { 10: files };
  emptyDirs = {};
  pageFailures = {};
}

// -- mounting helpers -------------------------------------------------------

async function settle(iterations = 20) {
  for (let i = 0; i < iterations; i += 1) await Promise.resolve();
}

const mounted = () => mount(FileList);

const rowTexts = (wrapper: ReturnType<typeof mounted>) =>
  wrapper.findAll('.file-rows > li').map((row) => {
    const folder = row.find('.folder-name');
    return folder.exists() ? folder.text() : row.find('.file-name').text();
  });

function folderRow(wrapper: ReturnType<typeof mounted>, name: string, which = 0) {
  return wrapper
    .findAll('.folder-row')
    .filter((row) => row.find('.folder-name').text() === name)[which]!;
}

const chevronOf = (row: ReturnType<typeof folderRow>) =>
  row!.find('[aria-label^="Expand"], [aria-label^="Collapse"]');

async function expand(row: NonNullable<ReturnType<typeof folderRow>>) {
  await chevronOf(row).trigger('click');
  await settle();
}

const invokes = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([c]) => c === command);

describe('FileList (paged feed)', () => {
  beforeEach(async () => {
    standardBoard();

    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation((async (command: string, args?: Record<string, unknown>) => {
      switch (command) {
        case 'list_top':
          return { loose, roots: rootRecords } satisfies TopListing;
        case 'get_server_info':
          return SERVER_STOPPED;
        case 'get_qr':
          return '';
        case 'list_page': {
          const rootId = args?.rootId as number;
          const subPath = args?.subPath as string;
          const offset = (args?.offset as number) ?? 0;
          const limit = (args?.limit as number) ?? PAGE_SIZE;
          if (!rootRecords.some((root) => root.id === rootId)) {
            throw new Error(`unknown root ${rootId}`);
          }
          const rows = nodeRows(rootId, subPath);
          const window = rows.slice(offset, offset + limit);
          return {
            root_id: rootId,
            sub_path: subPath,
            offset,
            rows: window,
            failures: pageFailures[`${rootId}@${subPath}`] ?? [],
            total: rows.length,
            has_more: offset + window.length < rows.length
          };
        }
        case 'remove_file': {
          const id = args?.id as number;
          const inLoose = loose.find((file) => file.id === id);
          loose = loose.filter((file) => file.id !== id);
          if (inLoose) return inLoose;
          let removed: SharedFile | undefined;
          for (const [rootId, files] of Object.entries(rootFiles)) {
            const hit = files.find((file) => file.id === id);
            if (hit) {
              removed = hit;
              rootFiles[Number(rootId)] = files.filter((file) => file.id !== id);
            }
          }
          if (!removed) throw new Error(`unknown file ${id}`);
          return removed;
        }
        case 'remove_root': {
          const rootId = args?.rootId as number;
          const record = rootRecords.find((root) => root.id === rootId);
          if (!record) throw new Error(`unknown root ${rootId}`);
          const removedFiles = rootFiles[rootId] ?? [];
          rootRecords = rootRecords.filter((root) => root.id !== rootId);
          const { [rootId]: _dropped, ...rest } = rootFiles;
          rootFiles = rest;
          return { record: { ...record }, files: removedFiles };
        }
        default:
          throw new Error(`unexpected command ${command}`);
      }
    }) as unknown as typeof invoke);

    // The composable's refs are module-level singletons; drain them
    // between tests, then re-read the freshly seeded board.
    const sharer = useSharer();
    sharer.loose.value = [];
    sharer.roots.value = [];
    sharer.failedAdds.value = [];
    sharer.error.value = null;
    sharer.busy.value = false;
    sharer.contentVersion.value = 0;
    sharer.info.value = SERVER_STOPPED;
    await sharer.init();
  });

  it('renders loose files then one collapsed lazily-known row per root record, fetching nothing', () => {
    const wrapper = mounted();

    expect(rowTexts(wrapper)).toEqual([
      'notes.txt',
      'Photos',
      'Docs'
    ]);
    expect(invokes('list_page')).toHaveLength(0);

    const photos = folderRow(wrapper, 'Photos')!;
    expect(chevronOf(photos).attributes('aria-expanded')).toBe('false');
    // The root row shows its counted total — display only; showing it
    // materializes nothing.
    expect(photos.text()).toContain('3 files');
  });

  it('expanding a root fetches its first page and lists materialized files and lazy subfolders', async () => {
    const wrapper = mounted();

    await expand(folderRow(wrapper, 'Photos')!);

    expect(vi.mocked(invoke)).toHaveBeenCalledWith('list_page', {
      rootId: 10,
      subPath: '',
      offset: 0,
      limit: PAGE_SIZE
    });
    // The page windows: the registered direct file first, then the
    // lazily-known `sunsets` subfolder (no eager enumeration of it).
    expect(rowTexts(wrapper)).toEqual(['notes.txt', 'Photos', 'beach.jpg', 'sunsets', 'Docs']);
  });

  it('lazy-folder rows expand too — their page fetches with the sub path and carries no checkbox', async () => {
    const wrapper = mounted();

    await expand(folderRow(wrapper, 'Photos')!);
    await expand(folderRow(wrapper, 'sunsets')!);

    expect(vi.mocked(invoke)).toHaveBeenCalledWith('list_page', {
      rootId: 10,
      subPath: 'sunsets',
      offset: 0,
      limit: PAGE_SIZE
    });
    expect(rowTexts(wrapper)).toEqual([
      'notes.txt',
      'Photos',
      'beach.jpg',
      'sunsets',
      'dune.jpg',
      'shore.jpg',
      'Docs'
    ]);

    // Lazily-known content is expandable only: no group checkbox and no
    // per-row delete on the lazy row itself.
    const sunsets = folderRow(wrapper, 'sunsets')!;
    expect(sunsets.find('[role="checkbox"]').exists()).toBe(false);
    expect(sunsets.text()).not.toContain('files');
  });

  it('stamps paged rows with depth tokens and keeps loose files flush', async () => {
    const wrapper = mounted();
    await expand(folderRow(wrapper, 'Photos')!);
    await expand(folderRow(wrapper, 'sunsets')!);

    const depthOf = (name: string, selector: string) =>
      wrapper
        .findAll('.file-rows > li')
        .find((row) => row.find(selector).exists() && row.find(selector).text() === name)!;

    // Loose files stay flush.
    const notes = depthOf('notes.txt', '.file-name');
    expect(notes.attributes('style')).toBeUndefined();
    expect(notes.attributes('data-depth')).toBeUndefined();

    expect(depthOf('Photos', '.folder-name').attributes('data-depth')).toBe('0');
    expect(depthOf('beach.jpg', '.file-name').attributes('data-depth')).toBe('1');
    expect(depthOf('sunsets', '.folder-name').attributes('data-depth')).toBe('1');
    expect(depthOf('dune.jpg', '.file-name').attributes('data-depth')).toBe('2');
  });

  it('materialization failures of a node surface as an inline marker, selectable-free', async () => {
    pageFailures = { '10@sunsets': [{ path: `${PHOTOS}/sunsets/locked`, error: 'denied' }] };
    await useSharer().init(); // fixture changed on the store, re-read

    const wrapper = mounted();
    await expand(folderRow(wrapper, 'Photos')!);
    await expand(folderRow(wrapper, 'sunsets')!);

    const marker = wrapper.find('.materialize-marker');
    expect(marker.exists()).toBe(true);
    expect(marker.text()).toContain(`1 item`);
    expect(marker.text()).toContain(`couldn't be read`);
    expect(marker.attributes('aria-label')).toContain("couldn't be read");
  });

  it('fetch-ahead appends the next window of an expanded node near the end', async () => {
    bigBoard();
    await useSharer().init();
    const wrapper = mounted();

    await expand(folderRow(wrapper, 'Photos')!);
    expect(wrapper.findAll('.file-row')).toHaveLength(PAGE_SIZE);
    expect(invokes('list_page')).toHaveLength(1);

    await wrapper.find('.file-rows').trigger('scroll');
    await settle();

    expect(vi.mocked(invoke)).toHaveBeenCalledWith('list_page', {
      rootId: 10,
      subPath: '',
      offset: PAGE_SIZE,
      limit: PAGE_SIZE
    });
    // The tail window appended: 250 members, no duplicates.
    const fileRows = wrapper.findAll('.file-row');
    expect(fileRows).toHaveLength(250);
    expect(new Set(fileRows.map((row) => row.find('.file-name').text())).size).toBe(250);
  });

  it('re-reads after actions refetch covered windows — rows merge at their offset, never duplicating', async () => {
    bigBoard();
    await useSharer().init();
    const wrapper = mounted();

    await expand(folderRow(wrapper, 'Photos')!);
    await wrapper.find('.file-rows').trigger('scroll'); // second window in
    expect(wrapper.findAll('.file-row')).toHaveLength(250);

    // Any action ends in a re-read (remove_file here); the covered
    // windows refetch and merge at their offsets.
    await wrapper.findAll('.file-row')[0].find('[aria-label^="Delete"]').trigger('click');
    await settle();

    const names = wrapper.findAll('.file-row').map((row) => row.find('.file-name').text());
    expect(names).toHaveLength(249);
    expect(new Set(names).size).toBe(249);
  });

  it('expansion and selection survive re-reads; a collapsed node refetches only on re-expand', async () => {
    const wrapper = mounted();
    await expand(folderRow(wrapper, 'Photos')!);

    // Pick the one materialized member; the group checkbox goes mixed.
    const beach = wrapper.findAll('.file-row').find((row) => row.text() === 'beach.jpg')!;
    await beach.find('[role="checkbox"]').trigger('click');
    expect(folderRow(wrapper, 'Photos')!.find('[aria-checked]').attributes('aria-checked')).toBe(
      'mixed'
    );

    await useSharer().init(); // the next action's re-read
    await settle();

    // Expansion and selection both survive…
    expect(chevronOf(folderRow(wrapper, 'Photos')!).attributes('aria-expanded')).toBe('true');
    expect(wrapper.findAll('.file-row.is-selected')).toHaveLength(1);

    // …and collapsing prunes the node's pages: no further window fetches
    // on the next re-read, and nothing under it renders.
    await expand(folderRow(wrapper, 'Photos')!); // collapse
    const pagesCalls = invokes('list_page').length;
    await useSharer().init();
    await settle();
    expect(invokes('list_page')).toHaveLength(pagesCalls);
    expect(rowTexts(wrapper)).toEqual(['notes.txt', 'Photos', 'Docs']);

    // Re-expanding is the trigger that materializes again.
    await expand(folderRow(wrapper, 'Photos')!);
    expect(rowTexts(wrapper)).toEqual(['notes.txt', 'Photos', 'beach.jpg', 'sunsets', 'Docs']);
  });

  it('checking a root group selects the whole lazily-known record; delete removes it via remove_root in one action', async () => {
    const wrapper = mounted();
    await expand(folderRow(wrapper, 'Photos')!);

    await folderRow(wrapper, 'Photos')!
      .find('[aria-label="Select or deselect all files in Photos"]')
      .trigger('click');
    await wrapper.find('[aria-label="Delete selected files"]').trigger('click');
    await settle();

    // The record plus every materialized member — one remove_root, and
    // no per-file removals for its members (removing a folder and then
    // downloading covers hidden content on the receiver side).
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('remove_root', { rootId: 10 });
    expect(invokes('remove_root')).toHaveLength(1);
    expect(
      invokes('remove_file').some((call) => (call[1] as { id: number }).id === 21)
    ).toBe(false);

    const sharer = useSharer();
    expect(sharer.roots.value.map((root) => root.name)).toEqual(['Docs']);
    expect(rowTexts(wrapper)).toEqual(['notes.txt', 'Docs']);
  });

  it('a selected member shows the lazy group mixed; unchecking the group drops member picks', async () => {
    const wrapper = mounted();
    await expand(folderRow(wrapper, 'Photos')!);

    const photosRow = folderRow(wrapper, 'Photos')!;
    const beach = wrapper.findAll('.file-row').find((row) => row.text() === 'beach.jpg')!;
    await beach.find('[role="checkbox"]').trigger('click');
    expect(photosRow.find('[aria-checked]').attributes('aria-checked')).toBe('mixed');

    // Check the group: everything materialized under it resolves through
    // the node, then the group owns the selection.
    await photosRow.find('[aria-label="Select or deselect all files in Photos"]').trigger('click');
    expect(photosRow.find('[aria-checked]').attributes('aria-checked')).toBe('true');
    expect(wrapper.findAll('.file-row.is-selected')).toHaveLength(1);

    // Unchecking drops the members' individual picks too.
    await photosRow.find('[aria-label="Select or deselect all files in Photos"]').trigger('click');
    expect(photosRow.find('[aria-checked]').attributes('aria-checked')).toBe('false');
    expect(wrapper.findAll('.file-row.is-selected')).toHaveLength(0);
  });

  it('delete selected splits mixed selections: groups through remove_root, stray loose ids through remove_file', async () => {
    const wrapper = mounted();

    // Whole Docs group plus the stray loose file, one action.
    await folderRow(wrapper, 'Docs')!
      .find('[aria-label="Select or deselect all files in Docs"]')
      .trigger('click');
    await wrapper.findAll('.file-row')[0].trigger('click'); // notes.txt
    await wrapper.find('[aria-label="Delete selected files"]').trigger('click');
    await settle();

    expect(vi.mocked(invoke)).toHaveBeenCalledWith('remove_root', { rootId: 11 });
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('remove_file', { id: 2 });
    expect(rowTexts(wrapper)).toEqual(['Photos']);
  });

  it('select-all covers what the list can see: rendered ids and whole lazy groups', async () => {
    const wrapper = mounted();

    await wrapper.find('[aria-label="Select all shared files"]').trigger('click');
    // Collapsed lazily-known records go as whole groups; nothing was
    // materialized into view, so selection materializes nothing.
    expect(invokes('list_page')).toHaveLength(0);
    await wrapper.find('[aria-label="Delete selected files"]').trigger('click');
    await settle();

    expect(invokes('remove_root')).toHaveLength(2); // both records
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('remove_file', { id: 2 });
    expect(rowTexts(wrapper)).toEqual([]);
  });

  it('same-named root records keep distinct keys, so selection acts on one group only', async () => {
    const second = [member(41, 'E:/elsewhere/Photos', 'cliff.jpg')];
    rootRecords = [
      record(10, PHOTOS, rootFiles[10]),
      record(12, 'E:/elsewhere/Photos', second, 'Photos-2'),
      record(11, DOCS, rootFiles[11])
    ];
    rootFiles = { ...rootFiles, 12: second };
    await useSharer().init();
    const wrapper = mounted();

    expect(rowTexts(wrapper)).toEqual(['notes.txt', 'Photos', 'Photos', 'Docs']);

    // Expand the second-by-key record: the page fetch targets its id.
    await expand(folderRow(wrapper, 'Photos', 1)!);
    const pageCalls = invokes('list_page');
    const page = pageCalls[pageCalls.length - 1]![1] as { rootId: number };
    expect(page.rootId).toBe(12);

    // Select the second group and delete it: the first record untouched.
    await folderRow(wrapper, 'Photos', 1)!
      .find('[aria-label="Select or deselect all files in Photos"]')
      .trigger('click');
    await wrapper.find('[aria-label="Delete selected files"]').trigger('click');
    await settle();

    const removedRoots = invokes('remove_root').map((call) => (call[1] as { rootId: number }).rootId);
    expect(removedRoots).toEqual([12]);
    expect(rowTexts(wrapper)).toEqual(['notes.txt', 'Photos', 'Docs']);
  });

  it('per-row delete removes exactly that materialized member', async () => {
    const wrapper = mounted();
    await expand(folderRow(wrapper, 'Photos')!);

    const beach = wrapper.findAll('.file-row').find((row) => row.text() === 'beach.jpg')!;
    await beach.find('[aria-label="Delete beach.jpg"]').trigger('click');
    await settle();

    expect(vi.mocked(invoke)).toHaveBeenCalledWith('remove_file', { id: 21 });
    expect(invokes('remove_root')).toHaveLength(0);
    // The parent remains; the dropped member stops rendering.
    expect(rowTexts(wrapper)).toEqual(['notes.txt', 'Photos', 'sunsets', 'Docs']);
  });

  it('a node whose content emptied out shows no stale rows on the next re-read', async () => {
    const wrapper = mounted();
    await expand(folderRow(wrapper, 'Docs')!);
    expect(rowTexts(wrapper)).toEqual(['notes.txt', 'Photos', 'Docs', 'a.txt']);

    // The record's last member goes; the record itself stays.
    await wrapper
      .findAll('.file-row')
      .find((row) => row.text() === 'a.txt')!
      .find('[aria-label^="Delete"]')
      .trigger('click');
    await settle();

    // The refetch covered no rows at all — the node must not keep its
    // old materialized rows as zombies.
    expect(rowTexts(wrapper)).toEqual(['notes.txt', 'Photos', 'Docs']);
    expect(folderRow(wrapper, 'Docs')!.find('[aria-expanded]').attributes('aria-expanded')).toBe(
      'true'
    );
  });

  it('resolved materialization failures lose their inline marker on the next re-read', async () => {
    pageFailures = { '10@sunsets': [{ path: `${PHOTOS}/sunsets/locked`, error: 'denied' }] };
    await useSharer().init();
    const wrapper = mounted();
    await expand(folderRow(wrapper, 'Photos')!);
    await expand(folderRow(wrapper, 'sunsets')!);
    expect(wrapper.find('.materialize-marker').exists()).toBe(true);

    // The folder reads fine now; the next re-read refetches the window
    // and the marker goes.
    pageFailures = {};
    await useSharer().init();
    await settle();
    expect(wrapper.find('.materialize-marker').exists()).toBe(false);
  });
});
