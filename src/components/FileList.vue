<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import ChevronIcon from '../assets/chevron-compact-up.svg';
import CheckedIcon from '../assets/square-rounded-check.svg';
import UncheckedIcon from '../assets/square-rounded.svg';
import TrashIcon from '../assets/trash-x.svg';
import { useSharer } from '../composables/useSharer';
import type { FailedAdd, NodePageRow, RootRow, SharedFile } from '../types';

const { loose, roots, busy, remove, removeSelection, contentVersion, error: shareError } =
  useSharer();

/// How many rows one node's page holds — the listing's page budget
/// (ADR-0006). Mirrors `LISTING_PAGE_ROWS` in
/// `crates/mosquito-core/src/listing_pages.rs`; move both together.
const PAGE_SIZE = 200;

/// How close to the scroll container's end a fetch-ahead begins.
const FETCH_AHEAD_PX = 120;

/// A page window as the backend returns it.
type PageWindow = {
  offset: number;
  rows: NodePageRow[];
  failures: FailedAdd[];
  has_more: boolean;
};

/// Selection — registered file ids and whole lazily-known root groups
/// (issue #24): the root checkbox selects the group, whose removal
/// takes the record plus every materialized member in one action.
const selectedIds = ref<number[]>([]);
const selectedRootIds = ref<number[]>([]);

/// Expansion is UI-held state keyed by the root's disambiguated key +
/// subfolder path (ADR-0002's rule kept): survives re-reads, resets
/// when the app restarts, never persisted. Lazily-known content starts
/// all collapsed — expanding is the act that materializes.
const expandedRoots = ref(new Map<string, boolean>());
const expandedSubfolders = ref(new Set<string>());

type Shade = 0 | 1;
type CheckState = 'checked' | 'mixed' | 'unchecked';

/// One folder node on screen: a root record's group or a lazily-known
/// subfolder under it. `subPath` is relative to the root record — the
/// backend's page addressing never carries absolute paths.
interface FolderRef {
  key: string;
  name: string;
  subPath: string;
  root: RootRow;
  isRoot: boolean;
}

/// A rendered row of the flat list.
type Row =
  | { kind: 'file'; file: SharedFile; depth: number; shade: Shade; rootId: number | null }
  | { kind: 'folder'; folder: FolderRef; depth: number; shade: Shade }
  | { kind: 'marker'; failures: FailedAdd[]; depth: number; shade: Shade };

/// Per-node paged feed: the rows already fetched (windows merged at
/// their offsets), the materialization failures per fetched window
/// (replaced on every window re-fetch, so resolved ones clear), and the
/// fetch-ahead cursor. A collapsed node holds nothing.
interface PageState {
  key: string;
  rootKey: string;
  subPath: string;
  rootId: number;
  rows: NodePageRow[];
  /// Failures seen per fetched window offset — the rendered `failures`
  /// is their offset-ordered union.
  windowFailures: Record<number, FailedAdd[]>;
  failures: FailedAdd[];
  fetchedThrough: number;
  hasMore: boolean;
  loading: boolean;
}

/// Expanded node pages by node key. `\u0000` joins the root key and the
/// sub path, so any character a name can carry stays addressable.
const pages = ref(new Map<string, PageState>());

function nodeKey(rootKey: string, subPath: string) {
  return subPath ? `${rootKey}\u0000${subPath}` : rootKey;
}

const rootFolder = (root: RootRow): FolderRef => ({
  key: root.key,
  name: root.name,
  subPath: '',
  root,
  isRoot: true
});

function ensurePageState(folder: FolderRef): PageState {
  let state = pages.value.get(folder.key);
  if (!state) {
    state = {
      key: folder.key,
      rootKey: folder.root.key,
      subPath: folder.subPath,
      rootId: folder.root.id,
      rows: [],
      windowFailures: {},
      failures: [],
      fetchedThrough: 0,
      hasMore: false,
      loading: false
    };
    pages.value.set(folder.key, state);
  }
  return state;
}

/// Merge one fetched page window into the node's feed at its offset:
/// fetch-ahead appends, re-fetches after actions replace their window —
/// either way rows never accumulate twice.
function applyPage(key: string, page: PageWindow) {
  const state = pages.value.get(key);
  if (!state) return;

  const rows = [...state.rows];
  rows.length = Math.max(rows.length, page.offset + page.rows.length);
  rows.splice(page.offset, page.rows.length, ...page.rows);
  state.rows = rows;
  state.fetchedThrough = Math.max(state.fetchedThrough, page.offset + page.rows.length);
  state.hasMore = page.has_more;

  // The refetched window replaces its recorded failures wholesale: a
  // member that read fine this time loses a stale marker, a member that
  // broke since gains one.
  state.windowFailures[page.offset] = page.failures;
  state.failures = Object.entries(state.windowFailures)
    .sort(([left], [right]) => Number(left) - Number(right))
    .flatMap(([, window]) => window);
}

/// Fetch one node window; the page request itself materializes the
/// node's direct children (ADR-0005 — the listing IS the trigger).
/// Returns the number of rows the window held, or null on failure —
/// callers fetching sequences stop after a failure.
async function fetchWindow(key: string, rootId: number, subPath: string, offset: number) {
  const state = pages.value.get(key);
  if (!state) return null;
  state.loading = true;
  try {
    const page = await invoke<PageWindow>('list_page', {
      rootId,
      subPath,
      offset,
      limit: PAGE_SIZE
    });
    applyPage(key, page);
    return page.rows.length;
  } catch (e) {
    shareError.value = String(e);
    return null;
  } finally {
    state.loading = false;
  }
}

/// Expansion: the first page fetch of a node is what materializes its
/// direct children; a later re-expand refetches from the first offset
/// over the store's idempotent registrations, so rows never duplicate.
async function expandToPage(folder: FolderRef) {
  const state = ensurePageState(folder);
  if (state.fetchedThrough === 0 && !state.loading) {
    await fetchWindow(folder.key, folder.root.id, folder.subPath, 0);
  }
}

function isExpanded(folder: FolderRef) {
  if (folder.isRoot) return expandedRoots.value.get(folder.key) ?? false;
  return expandedSubfolders.value.has(folder.key);
}

/// Collapsing empties the node's page plus everything under it —
/// unmaterialized subtrees are never held (issue #24). The `\u0000` and
/// `/` boundaries keep pruning whole-segment exact: `Photos` cannot
/// prune `Photos-2`.
function pruneUnder(folder: FolderRef) {
  const ownPrefix = folder.key + '\u0000';
  const prefix = folder.isRoot ? ownPrefix : `${folder.key}/`;
  for (const key of [...pages.value.keys()]) {
    if (key === folder.key || key.startsWith(prefix)) pages.value.delete(key);
  }
  for (const key of [...expandedSubfolders.value]) {
    if (key.startsWith(prefix)) expandedSubfolders.value.delete(key);
  }
}

async function toggleFolder(folder: FolderRef) {
  const open = !isExpanded(folder);

  if (folder.isRoot) {
    const next = new Map(expandedRoots.value);
    next.set(folder.key, open);
    expandedRoots.value = next;
  } else {
    const next = new Set(expandedSubfolders.value);
    if (open) next.add(folder.key);
    else next.delete(folder.key);
    expandedSubfolders.value = next;
  }

  if (open) await expandToPage(folder);
  else pruneUnder(folder);
}

/// Shading alternates per depth (ADR-0002): an expanded folder's whole
/// region takes this shade, and a collapsed folder row falls back to
/// the shade it inherited.
function expandedRegionShade(depth: number): Shade {
  return depth % 2 === 0 ? 1 : 0;
}

/// The rows a node's page feeds render: materialized file rows,
/// lazily-known subfolder rows (recursively, when expanded), and the
/// node's inline materialization-failure marker last.
function folderRows(folder: FolderRef, depth: number, parentShade: Shade, out: Row[]) {
  const open = isExpanded(folder);
  const shade: Shade = open ? expandedRegionShade(depth) : parentShade;
  out.push({ kind: 'folder', folder, depth, shade });

  if (!open) return;

  const state = pages.value.get(folder.key);
  if (!state) return;

  const regionShade = expandedRegionShade(depth);
  for (const pageRow of state.rows) {
    if (pageRow.kind === 'file') {
      out.push({
        kind: 'file',
        file: pageRow.file,
        depth: depth + 1,
        shade: regionShade,
        rootId: folder.root.id
      });
    } else {
      const subPath = folder.subPath ? `${folder.subPath}/${pageRow.name}` : pageRow.name;
      folderRows(
        {
          key: nodeKey(folder.root.key, subPath),
          name: pageRow.name,
          subPath,
          root: folder.root,
          isRoot: false
        },
        depth + 1,
        regionShade,
        out
      );
    }
  }

  if (state.failures.length > 0) {
    out.push({ kind: 'marker', failures: state.failures, depth: depth + 1, shade: regionShade });
  }
}

/// The flat list: loose files lead, then one collapsed group per root
/// record. Anything lazily-known renders only through pages of expanded
/// nodes — display never registers content (issue #24).
const rows = computed<Row[]>(() => {
  const out: Row[] = [];
  for (const file of loose.value) {
    out.push({ kind: 'file', file, depth: 0, shade: 0, rootId: null });
  }
  for (const root of roots.value) {
    folderRows(rootFolder(root), 0, 0, out);
  }
  return out;
});

const rootCounts = computed(() => {
  const counts = new Map<string, number>();
  for (const root of roots.value) counts.set(root.key, root.file_count);
  return counts;
});

// -- selection -----------------------------------------------------------

const renderedFileIds = computed(() =>
  rows.value.flatMap((row) => (row.kind === 'file' ? [row.file.id] : []))
);

/// Which root group (if any) a known file id belongs to — the root
/// checkbox's mixed state reads from it, and removal skips ids a group
/// removal already sweeps.
const fileOwnerRoot = computed(() => {
  const owners = new Map<number, number>();
  for (const row of rows.value) {
    if (row.kind === 'file' && row.rootId !== null) owners.set(row.file.id, row.rootId);
  }
  return owners;
});

watch(renderedFileIds, (ids) => {
  if (selectedIds.value.some((id) => !ids.includes(id))) {
    selectedIds.value = selectedIds.value.filter((id) => ids.includes(id));
  }
});

watch(roots, (nextRoots) => {
  if (selectedRootIds.value.some((id) => !nextRoots.some((root) => root.id === id))) {
    selectedRootIds.value = selectedRootIds.value.filter((id) =>
      nextRoots.some((root) => root.id === id)
    );
  }
});

function isSelected(id: number) {
  return selectedIds.value.includes(id);
}

/// Whether a root group's whole pick covers a file row: the group owns
/// its subtree until it is unchecked (ADR-0006 — selection resolves
/// through the node), so a covered member shows the tick and its own
/// checkbox goes inert — flipping a covered member off isn't
/// representable (there are no exclusions to a node selection); the
/// group checkbox is the way to label finer intent. Loose files have no
/// owner and are never covered.
function ownerGroupSelected(id: number): boolean {
  const owner = fileOwnerRoot.value.get(id);
  return owner !== undefined && selectedRootIds.value.includes(owner);
}

/// The tick a rendered file row shows: its own pick, or the group's
/// whole pick over it.
function rowSelected(id: number): boolean {
  return isSelected(id) || ownerGroupSelected(id);
}

/// Toggle one member's own pick — covered members are inert: the click
/// reaches here through the whole row too, not only the checkbox.
function toggleFile(id: number) {
  if (isSelected(id)) {
    selectedIds.value = selectedIds.value.filter((selectedId) => selectedId !== id);
  } else if (!ownerGroupSelected(id)) {
    selectedIds.value = [...selectedIds.value, id];
  }
}

const rootChecks = computed(() => {
  const states = new Map<string, CheckState>();
  for (const root of roots.value) {
    if (selectedRootIds.value.includes(root.id)) {
      states.set(root.key, 'checked');
    } else {
      const memberPicked = [...fileOwnerRoot.value.entries()].some(
        ([id, ownerRoot]) => ownerRoot === root.id && isSelected(id)
      );
      states.set(root.key, memberPicked ? 'mixed' : 'unchecked');
    }
  }
  return states;
});

function checkAria(state: CheckState | undefined) {
  const check = state ?? 'unchecked';
  return check === 'checked' ? 'true' : check === 'mixed' ? 'mixed' : 'false';
}

function toggleRoot(root: RootRow) {
  if (selectedRootIds.value.includes(root.id)) {
    // Unchecking the group also drops its members' individual picks —
    // the group checkbox owns its subtree like it always did
    // (selection resolves through the node, ADR-0006).
    const owned = new Set(
      [...fileOwnerRoot.value]
        .filter(([, owner]) => owner === root.id)
        .map(([id]) => id)
    );
    selectedIds.value = selectedIds.value.filter((id) => !owned.has(id));
    selectedRootIds.value = selectedRootIds.value.filter((id) => id !== root.id);
  } else {
    selectedRootIds.value = [...selectedRootIds.value, root.id];
  }
}

const allSelected = computed(
  () =>
    renderedFileIds.value.length + roots.value.length > 0 &&
    selectedIds.value.length === renderedFileIds.value.length &&
    selectedRootIds.value.length === roots.value.length
);
const someSelected = computed(
  () => (selectedIds.value.length > 0 || selectedRootIds.value.length > 0) && !allSelected.value
);
const selectedCount = computed(() => selectedIds.value.length + selectedRootIds.value.length);

function toggleAll() {
  if (allSelected.value) {
    selectedIds.value = [];
    selectedRootIds.value = [];
  } else {
    // Everything the list can see: rendered materialized ids and the
    // whole lazy groups. Lazily-known rows nobody opened stay out —
    // selection never materializes content it hasn't shown.
    selectedIds.value = [...renderedFileIds.value];
    selectedRootIds.value = roots.value.map((root) => root.id);
  }
}

// -- removal -------------------------------------------------------------

function removeOne(id: number) {
  selectedIds.value = selectedIds.value.filter((selectedId) => selectedId !== id);
  remove(id);
}

function removeSelected() {
  if (!selectedCount.value) return;

  const groupIds = new Set(selectedRootIds.value);
  // Member ids of a removed group are swept by the group's own removal
  // — the record plus every materialized member as one action.
  const fileIds = selectedIds.value.filter((id) => {
    const owner = fileOwnerRoot.value.get(id);
    return owner === undefined || !groupIds.has(owner);
  });

  selectedIds.value = [];
  selectedRootIds.value = [];

  if (groupIds.size > 0 || fileIds.length > 0) {
    removeSelection(fileIds, [...groupIds]);
  }
}

// -- fetch-ahead (infinite scroll, ADR-0006) -----------------------------

const listElement = ref<HTMLElement | null>(null);

/// Near the end of the container, every expanded node with a further
/// page pulls its next window, so the tail is seamless wherever it
/// lands — the hold stays bounded: at most one extra page-budget
/// window per expanded node (ADR-0006's page budget), and collapsed
/// nodes hold nothing. Selection and expansion ride untouched.
async function fetchAhead() {
  for (const state of [...pages.value.values()]) {
    if (!state.hasMore || state.loading) continue;
    await fetchWindow(state.key, state.rootId, state.subPath, state.fetchedThrough);
  }
}

function onScroll() {
  const element = listElement.value;
  if (!element) return;
  const nearEnd =
    element.scrollTop + element.clientHeight >= element.scrollHeight - FETCH_AHEAD_PX;
  if (nearEnd) void fetchAhead();
}

// -- re-truthfulness after actions ---------------------------------------

/// Every action ends in a re-read (the composable's refresh bumps
/// `contentVersion`): re-fetch the covered windows of each expanded
/// node so pages can't drift from the store, then trim the tail — a
/// removed root's nodes lose their pages entirely.
async function refetchCoveredWindows() {
  for (const state of [...pages.value.values()]) {
    if (!roots.value.some((root) => root.key === state.rootKey)) {
      pages.value.delete(state.key);
      continue;
    }

    let covered = 0;
    let failed = false;
    for (let offset = 0; offset < state.fetchedThrough && !failed; offset += PAGE_SIZE) {
      const count = await fetchWindow(state.key, state.rootId, state.subPath, offset);
      if (count === null) failed = true;
      else covered = Math.max(covered, offset + count);
    }

    const fresh = pages.value.get(state.key);
    // One node's failed window must not veto the others' refresh — the
    // error banner carries what went wrong, and this node keeps its
    // previous rows rather than being wiped by a transient failure.
    if (!fresh || failed) continue;

    // The refetched total may have shrunk below the old fetch (content
    // removed elsewhere, or emptied entirely): trim the tail to what
    // the pass actually covered so rows can't outlive the store.
    fresh.rows.length = covered;
    fresh.fetchedThrough = covered;
  }
}

watch(contentVersion, () => {
  void refetchCoveredWindows();
});

// -- inline display helpers ----------------------------------------------

function markerText(count: number) {
  return `${count} item${count === 1 ? '' : 's'} of this folder couldn't be read`;
}

function markerTitle(failures: FailedAdd[]) {
  return failures.map((failure) => `${failure.path} — ${failure.error}`).join('\n');
}

function folderCountText(count: number | undefined) {
  return count === undefined ? '' : `${count} ${count === 1 ? 'file' : 'files'}`;
}
</script>

<template>
  <section class="file-table" :aria-busy="busy">
    <div class="files-header" role="row">
      <button
        class="checkbox-control"
        type="button"
        role="checkbox"
        :aria-checked="someSelected ? 'mixed' : allSelected ? 'true' : 'false'"
        aria-label="Select all shared files"
        :disabled="busy"
        @click="toggleAll"
      >
        <CheckedIcon v-if="allSelected" />
        <UncheckedIcon v-else />
      </button>

      <span>File Name</span>

      <button
        class="file-action"
        type="button"
        aria-label="Delete selected files"
        :disabled="selectedCount === 0 || busy"
        @click="removeSelected"
      >
        <TrashIcon />
      </button>
    </div>

    <ul ref="listElement" class="file-rows thin-scrollbar" role="rowgroup" @scroll="onScroll">
      <template
        v-for="row in rows"
        :key="row.kind === 'file' ? `f-${row.file.id}` : row.kind === 'folder' ? `d-${row.folder.key}` : `m-${row.depth}-${row.failures.map((failure) => failure.path).join('|')}`"
      >
        <li
          v-if="row.kind === 'folder'"
          class="folder-row"
          role="row"
          :class="{ 'shade-1': row.shade === 1 }"
          :style="{ '--depth': String(row.depth) }"
          :data-depth="String(row.depth)"
        >
          <button
            v-if="row.folder.isRoot"
            class="checkbox-control"
            type="button"
            role="checkbox"
            :aria-checked="checkAria(rootChecks.get(row.folder.key))"
            :aria-label="`Select or deselect all files in ${row.folder.name}`"
            :disabled="busy"
            @click.stop="toggleRoot(row.folder.root)"
          >
            <CheckedIcon v-if="rootChecks.get(row.folder.key) === 'checked'" />
            <UncheckedIcon v-else />
          </button>
          <span v-else aria-hidden="true" class="checkbox-spacer"></span>

          <button
            class="folder-chevron"
            :class="{ 'is-collapsed': !isExpanded(row.folder) }"
            type="button"
            :aria-expanded="isExpanded(row.folder) ? 'true' : 'false'"
            :aria-label="`${isExpanded(row.folder) ? 'Collapse' : 'Expand'} ${row.folder.name}`"
            @click="toggleFolder(row.folder)"
          >
            <ChevronIcon />
          </button>

          <span class="folder-name">{{ row.folder.name }}</span>

          <small v-if="row.folder.isRoot" class="folder-count">
            {{ folderCountText(rootCounts.get(row.folder.key)) }}
          </small>
        </li>

        <li
          v-else-if="row.kind === 'file'"
          class="file-row"
          :class="{ 'is-selected': rowSelected(row.file.id), 'shade-1': row.shade === 1 }"
          :title="row.file.path"
          :style="row.rootId !== null ? { '--depth': String(row.depth) } : undefined"
          :data-depth="row.rootId !== null ? String(row.depth) : undefined"
          role="row"
          :aria-selected="rowSelected(row.file.id)"
          @click="toggleFile(row.file.id)"
        >
          <button
            type="button"
            role="checkbox"
            :aria-checked="rowSelected(row.file.id)"
            :aria-label="`${isSelected(row.file.id) ? 'Deselect' : 'Select'} ${row.file.name}`"
            :disabled="busy || ownerGroupSelected(row.file.id)"
            @click.stop="toggleFile(row.file.id)"
          >
            <CheckedIcon v-if="rowSelected(row.file.id)" />
            <UncheckedIcon v-else />
          </button>

          <span class="file-name">{{ row.file.name }}</span>

          <button
            type="button"
            :aria-label="`Delete ${row.file.name}`"
            :disabled="busy"
            @click.stop="removeOne(row.file.id)"
          >
            <TrashIcon />
          </button>
        </li>

        <li
          v-else
          class="materialize-marker"
          role="row"
          :class="{ 'shade-1': row.shade === 1 }"
          :style="{ '--depth': String(row.depth) }"
          :data-depth="String(row.depth)"
          :aria-label="markerText(row.failures.length)"
          :title="markerTitle(row.failures)"
        >
          <span aria-hidden="true" class="checkbox-spacer"></span>
          <p class="marker-text">{{ markerText(row.failures.length) }}</p>
        </li>
      </template>
    </ul>
  </section>
</template>

<style scoped>
/* Row layout matches the pre-paged lists: the header and member rows
   share one 38px strip; folder rows keep their checkbox and chevron
   columns, with the counted total parked at the far end. */
.file-table {
  width: 100%;
  overflow: hidden;
  border: 2px solid var(--ink);
  border-radius: var(--radius);
  background: var(--surface);
}

.files-header,
.file-row {
  display: grid;
  grid-template-columns: 32px minmax(0, 1fr) 32px;
  align-items: center;
  column-gap: 4px;
  /* Files match the folder row height (ADR-0004 revision): the 38px
     strip is the shared row height, header included. */
  min-height: 38px;
  padding: 0 10px;
}

.files-header {
  border-bottom: 2px solid var(--ink);
  font-size: 0.875rem;
  font-weight: 600;
  line-height: 1;
}

.file-rows {
  margin: 0;
  padding: 0;
  list-style: none;
  overflow-y: auto;
  max-height: 20rem;
}

.file-row {
  cursor: pointer;
  transition:
    background-color 140ms ease,
    color 140ms ease;
}

/* Rows are flat siblings; every row but the first gets a top border,
   no matter what kind of row precedes it. */
.file-rows > li:not(:first-child) {
  border-top: 1px solid var(--ink);
}

/* The shared shade token (style.css) — the whole expanded folder incl.
   its contents reads as one region. */
.shade-1 {
  background: var(--folder-shade);
}

/* One font size for all rows (ADR-0004): the depth cue moves to
   geometry — name text indents 16px per depth; the folder chevron
   travels with it so the tree edge reads as one line. Member rows
   catch up the chevron slot (36px: column + gap) on top of their own
   depth, so everything inside a folder sits exactly one level right of
   the folder's own name. Loose rows carry no depth tokens: flush. */
.folder-row,
.file-row {
  font-size: 0.875rem;
}

.folder-row {
  display: grid;
  grid-template-columns: 32px 32px minmax(0, 1fr) auto;
  align-items: center;
  column-gap: 4px;
  min-height: 38px;
  padding: 0 10px;
  color: var(--ink);
  font-weight: 600;
  line-height: 1;
}

.folder-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  padding-left: calc(16px * var(--depth, 0));
}

.folder-chevron {
  margin-left: calc(16px * var(--depth, 0));
}

.file-row[data-depth] .file-name {
  padding-left: calc(16px * var(--depth, 0) + 36px);
}

.folder-chevron svg {
  display: block;
  width: 1.5rem;
  height: 1.5rem;
  transition: transform 140ms ease;
}

.folder-chevron.is-collapsed svg {
  transform: rotate(180deg);
}

@media (prefers-reduced-motion: reduce) {
  .folder-chevron svg {
    transition: none;
  }
}

/* The root row also carries its counted file total — display only;
   showing it materializes nothing (issue #24, first box). */
.folder-count {
  justify-self: end;
  padding-left: 8px;
  color: var(--ink);
  opacity: 0.6;
  font-size: 0.6875rem;
  font-weight: 400;
  white-space: nowrap;
}

.materialize-marker {
  display: grid;
  grid-template-columns: 32px minmax(0, 1fr);
  align-items: center;
  column-gap: 4px;
  min-height: 38px;
  padding: 0 10px;
  font-size: 0.8125rem;
  line-height: 1.2;
}

.marker-text {
  margin: 0;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--ink);
  opacity: 0.65;
  font-style: italic;
  padding-left: calc(16px * var(--depth, 0) + 36px);
}

.checkbox-spacer {
  display: block;
}

.file-row:hover,
.file-row.is-selected {
  background: var(--ink);
  color: var(--surface);
}

.file-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-action {
  justify-self: end;
}

.files-header .file-action,
.file-row .file-action {
  opacity: 0.5;
}

.files-header .file-action:not(:disabled),
.file-row:hover .file-action,
.file-row.is-selected .file-action {
  opacity: 1;
}

.checkbox-control:disabled,
.file-action:disabled {
  cursor: default;
}

.file-icon {
  display: block;
  width: 1.5rem;
  height: 1.5rem;
  flex: 0 0 1.5rem;
}

.file-row:hover .file-icon,
.file-row.is-selected .file-icon {
  filter: brightness(0) invert(1);
}
</style>
