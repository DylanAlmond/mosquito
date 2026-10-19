import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { open } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import type { ImportRunSnapshot, ImportStartOutcome } from '../types';
import { stopRunPolling, useSharer } from '../composables/useSharer';
import SharePanel from './SharePanel.vue';

vi.mock('@tauri-apps/plugin-dialog');
vi.mock('@tauri-apps/api/core');

vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent: vi.fn(async () => vi.fn()) })
}));

const SERVER_STOPPED = { running: false, url: null, port: null, lan_ip: null };
const SERVER_RUNNING = { running: true, url: 'http://10.0.0.2:7331', port: 7331, lan_ip: '10.0.0.2' };

describe('SharePanel import runs', () => {
  let importPathsOutcomes: ImportStartOutcome[];
  let importRuns: ImportRunSnapshot[];

  beforeEach(async () => {
    importPathsOutcomes = [];
    importRuns = [];
    vi.useFakeTimers();

    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation((async (command: string) => {
      switch (command) {
        case 'list_top':
          return { loose: [], roots: [] };
        case 'get_server_info':
          return SERVER_STOPPED;
        case 'get_qr':
          return '';
        case 'import_paths':
          return importPathsOutcomes.shift() ?? { run_id: null, immediate: { added: [], failed: [] } };
        case 'import_runs':
          return structuredClone(importRuns);
        case 'cancel_import':
          return true;
        case 'start_server':
        case 'stop_server':
          return SERVER_RUNNING;
        default:
          throw new Error(`unexpected command ${command}`);
      }
    }) as unknown as typeof invoke);

    sharedFilesReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function sharedFilesReset() {
    const sharer = useSharer();
    sharer.info.value = SERVER_STOPPED;
    sharer.loose.value = [];
    sharer.roots.value = [];
    sharer.failedAdds.value = [];
    sharer.runs.value = [];
    sharer.busy.value = false;
    stopRunPolling();
  }

  const counting = (runId: number, path: string, files: number): ImportRunSnapshot => ({
    run_id: runId,
    roots: [{ path, files, status: 'Counting' }]
  });

  const folderOutcome = (runId: number | null): ImportStartOutcome => ({
    run_id: runId,
    immediate: { added: [], failed: [] }
  });

  const pickFolder = async (
    wrapper: ReturnType<typeof mount>,
    path: string,
    { dialog = true }: { dialog?: boolean } = {}
  ) => {
    if (dialog) vi.mocked(open).mockResolvedValue([path]);
    await wrapper.find('[aria-label="Choose folder"]').trigger('click');
    await settle();
  };

  /// Faketimers set: drain promises without touching the clocks.
  const settle = async () => {
    for (let i = 0; i < 10; i += 1) await Promise.resolve();
  };

  const progress = (wrapper: ReturnType<typeof mount>) =>
    wrapper.find('[aria-label="Import progress"]');

  it('a folder pick starts an import run and shows its counting progress', async () => {
    importPathsOutcomes = [folderOutcome(1)];
    importRuns = [counting(1, 'C:/Users/me/Photos', 0)];
    const wrapper = mount(SharePanel);

    await pickFolder(wrapper, 'C:/Users/me/Photos');

    expect(vi.mocked(invoke)).toHaveBeenCalledWith('import_paths', {
      paths: ['C:/Users/me/Photos']
    });
    expect(progress(wrapper).exists()).toBe(true);
    expect(progress(wrapper).text()).toContain('counting');
    expect(progress(wrapper).text()).toContain('C:/Users/me/Photos');
    expect(progress(wrapper).text()).toContain('0 entries');
  });

  it('counting progress advances during the walk', async () => {
    importPathsOutcomes = [folderOutcome(1)];
    importRuns = [counting(1, 'C:/Users/me/Photos', 0)];
    const wrapper = mount(SharePanel);

    await pickFolder(wrapper, 'C:/Users/me/Photos');
    expect(progress(wrapper).text()).toContain('0 entries');

    importRuns = [counting(1, 'C:/Users/me/Photos', 500)];
    await vi.advanceTimersByTimeAsync(250);
    await settle();

    expect(progress(wrapper).text()).toContain('500 entries');
  });

  it('cancelling discards the in-flight root with nothing committed', async () => {
    importPathsOutcomes = [folderOutcome(1)];
    importRuns = [counting(1, 'C:/Users/me/Photos', 0)];
    const wrapper = mount(SharePanel);
    await pickFolder(wrapper, 'C:/Users/me/Photos');

    // A cancelled run leaves no record — nothing to refresh into the list.
    importRuns = [];
    await progress(wrapper)
      .find('[aria-label="Cancel import of C:/Users/me/Photos"]')
      .trigger('click');
    await settle();

    expect(vi.mocked(invoke)).toHaveBeenCalledWith('cancel_import', { runId: 1 });
    expect(progress(wrapper).exists()).toBe(false);
  });

  it('a re-drop after cancelling completes normally', async () => {
    importPathsOutcomes = [folderOutcome(1), folderOutcome(2)];
    importRuns = [counting(1, 'C:/Users/me/Photos', 0)];
    const wrapper = mount(SharePanel);
    await pickFolder(wrapper, 'C:/Users/me/Photos');

    importRuns = []; // cancelled: nothing committed
    await progress(wrapper)
      .find('[aria-label="Cancel import of C:/Users/me/Photos"]')
      .trigger('click');
    await settle();

    importRuns = [counting(2, 'C:/Users/me/Photos', 2)];
    await pickFolder(wrapper, 'C:/Users/me/Photos');
    await vi.advanceTimersByTimeAsync(250);
    await settle();

    expect(progress(wrapper).exists()).toBe(true);
    expect(progress(wrapper).text()).toContain('2 entries');
  });

  it('a committed root settles its run and the shared view refreshes', async () => {
    importPathsOutcomes = [folderOutcome(1)];
    importRuns = [counting(1, 'C:/Users/me/Photos', 0)];
    const wrapper = mount(SharePanel);
    await pickFolder(wrapper, 'C:/Users/me/Photos');

    const listCalls = () =>
      vi.mocked(invoke).mock.calls.filter(([command]) => command === 'list_top').length;
    const callsBefore = listCalls();

    importRuns = [
      {
        run_id: 1,
        roots: [{ path: 'C:/Users/me/Photos', files: 12, status: 'Committed' }]
      }
    ];
    await vi.advanceTimersByTimeAsync(250);
    await settle();


    expect(progress(wrapper).exists()).toBe(false);
    expect(listCalls()).toBeGreaterThan(callsBefore);
  });

  it('concurrent drops queue as additional independent runs, each with its own progress and cancel state', async () => {
    importPathsOutcomes = [folderOutcome(1), folderOutcome(2)];
    importRuns = [counting(1, 'C:/Users/me/Photos', 0)];
    const wrapper = mount(SharePanel);
    await pickFolder(wrapper, 'C:/Users/me/Photos');

    importRuns = [counting(1, 'C:/Users/me/Photos', 40), counting(2, 'C:/Users/me/Docs', 3)];
    await pickFolder(wrapper, 'C:/Users/me/Docs');

    const runs = wrapper.findAll('.import-run');
    expect(runs).toHaveLength(2);
    expect(runs[0].text()).toContain('C:/Users/me/Photos');
    expect(runs[0].text()).toContain('40 entries');
    expect(runs[1].text()).toContain('C:/Users/me/Docs');
    expect(runs[1].text()).toContain('3 entries');

    // Cancelling one run touches only that run.
    importRuns = [
      counting(1, 'C:/Users/me/Photos', 40),
      {
        run_id: 2,
        roots: [{ path: 'C:/Users/me/Docs', files: 3, status: 'Cancelled' }]
      }
    ];
    await runs[1].find('[aria-label^="Cancel import of"]').trigger('click');
    await settle();

    expect(vi.mocked(invoke)).toHaveBeenCalledWith('cancel_import', { runId: 2 });
    expect(wrapper.findAll('.import-run')).toHaveLength(1);
    expect(wrapper.findAll('.import-run')[0].text()).toContain('C:/Users/me/Photos');
  });

  it('an in-flight import run never disables the other surface actions', async () => {
    importPathsOutcomes = [folderOutcome(1)];
    importRuns = [counting(1, 'C:/Users/me/Photos', 0)];
    const wrapper = mount(SharePanel);
    await pickFolder(wrapper, 'C:/Users/me/Photos');

    expect(wrapper.find('.dropzone').attributes()).not.toHaveProperty('disabled');
    expect(wrapper.find('[aria-label="Choose folder"]').attributes()).not.toHaveProperty('disabled');

    // Further picks still work while the run is counting.
    vi.mocked(open).mockClear();
    await pickFolder(wrapper, 'C:/Users/me/Docs');
    expect(open).toHaveBeenCalledWith(expect.objectContaining({ directory: true }));
  });

  it('a busy action elsewhere never blocks picks either', async () => {
    const sharer = useSharer();
    sharer.busy.value = true;
    const wrapper = mount(SharePanel);

    expect(wrapper.find('.dropzone').attributes()).not.toHaveProperty('disabled');
    await pickFolder(wrapper, 'C:/Users/me/Photos', { dialog: false });
    expect(open).toHaveBeenCalledWith(expect.objectContaining({ directory: true }));
  });

  it('a settled failure never resurfaces over a later drop', async () => {
    const sharer = useSharer();
    importPathsOutcomes = [folderOutcome(3)];
    importRuns = [
      {
        run_id: 3,
        roots: [{ path: 'C:/Users/me/Archive', files: 0, status: { Failed: 'unreadable' } }]
      }
    ];
    const wrapper = mount(SharePanel);
    await pickFolder(wrapper, 'C:/Users/me/Archive');
    expect(sharer.failedAdds.value).toEqual([{ path: 'C:/Users/me/Archive', error: 'unreadable' }]);

    // A later drop dismisses the banner; the old failure must not return.
    sharer.failedAdds.value = [];
    importPathsOutcomes = []; // the new drop is a loose-only outcome
    await pickFolder(wrapper, 'C:/Users/me/Docs');

    expect(sharer.failedAdds.value).toEqual([]);
  });

  it('an unreadable root fails through the existing failed-add flow', async () => {
    importPathsOutcomes = [folderOutcome(1)];
    importRuns = [
      {
        run_id: 1,
        roots: [{ path: 'C:/Users/me/Photos', files: 0, status: { Failed: 'unreadable' } }]
      }
    ];
    const sharer = useSharer();
    const wrapper = mount(SharePanel);

    await pickFolder(wrapper, 'C:/Users/me/Photos');

    expect(sharer.failedAdds.value).toEqual([
      { path: 'C:/Users/me/Photos', error: 'unreadable' }
    ]);
    expect(progress(wrapper).exists()).toBe(false);
  });

  it('a loose-only drop keeps the synchronous flow', async () => {
    importPathsOutcomes = [
      {
        run_id: null,
        immediate: {
          added: [
            {
              id: 1,
              name: 'notes.txt',
              path: 'C:/Users/me/notes.txt',
              size: 3,
              provenance: null
            }
          ],
          failed: [{ path: 'C:/Users/me/denied.txt', error: 'denied' }]
        }
      }
    ];
    const sharer = useSharer();
    const wrapper = mount(SharePanel);

    vi.mocked(open).mockResolvedValue(['C:/Users/me/notes.txt']);
    await wrapper.find('.dropzone').trigger('click');
    await settle();

    expect(progress(wrapper).exists()).toBe(false);
    expect(sharer.failedAdds.value).toEqual([
      { path: 'C:/Users/me/denied.txt', error: 'denied' }
    ]);
    // Dropping files IS the start button (loose adds land immediately).
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('start_server');
  });

  it('the dropzone keeps opening the regular file dialog, not the directory one', async () => {
    vi.mocked(open).mockResolvedValue(null);
    const wrapper = mount(SharePanel);

    await wrapper.find('.dropzone').trigger('click');
    await settle();

    expect(open).toHaveBeenCalledWith(expect.objectContaining({ directory: false }));
  });
});
