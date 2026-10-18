<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import ChevronIcon from '../assets/chevron-compact-up.svg';
import CheckedIcon from '../assets/square-rounded-check.svg';
import UncheckedIcon from '../assets/square-rounded.svg';
import TrashIcon from '../assets/trash-x.svg';
import { useSharer } from '../composables/useSharer';
import { buildFolderTree, type FolderNode } from '../lib/folderTree';
import type { SharedFile } from '../types';

const { files, busy, remove, removeMany } = useSharer();

const selectedIds = ref<number[]>([]);

/// The folder tree is derived at render time (ADR-0002); the store stays
/// flat. Members keep their position in the shared list, so roots and
/// subfolders appear in first-appearance order.
const tree = computed(() => buildFolderTree(files.value));

/// Expansion is UI-held state keyed by folder root + subfolder path
/// (ADR-0002): it survives re-reads after every action and resets when
/// the app restarts — it is never persisted. Roots default expanded,
/// subfolders default collapsed; a checked key means "away from the
/// default". Root key: the folder root itself (no subfolder path).
const collapsedRoots = ref(new Set<string>());
const expandedSubfolders = ref(new Set<string>());

function rootKey(node: FolderNode) {
  return node.folderRoot;
}

function subKey(node: FolderNode) {
  return `${node.folderRoot}\u0000${node.subPath}`;
}

function isExpanded(node: FolderNode) {
  return node.subPath === ''
    ? !collapsedRoots.value.has(rootKey(node))
    : expandedSubfolders.value.has(subKey(node));
}

function toggleFolder(node: FolderNode) {
  if (node.subPath === '') {
    const key = rootKey(node);
    const next = new Set(collapsedRoots.value);
    next.has(key) ? next.delete(key) : next.add(key);
    collapsedRoots.value = next;
  } else {
    const key = subKey(node);
    const next = new Set(expandedSubfolders.value);
    next.has(key) ? next.delete(key) : next.add(key);
    expandedSubfolders.value = next;
  }
}

/// Shading alternates per depth (ADR-0002) and rows indent their name
/// text 16px per depth (ADR-0004): an expanded folder's whole region
/// takes this shade, and a collapsed folder row falls back to the shade
/// it inherited.
type Shade = 0 | 1;

/// The shade an expanded folder paints over itself and everything
/// inside it: grey at even depth, back to the plain surface at odd.
function expandedRegionShade(depth: number): Shade {
  return depth % 2 === 0 ? 1 : 0;
}

type Row =
  | { kind: 'file'; file: SharedFile; depth: number; shade: Shade }
  | { kind: 'folder'; node: FolderNode; depth: number; shade: Shade };

function folderRows(node: FolderNode, parentShade: Shade): Row[] {
  const open = isExpanded(node);
  const shade: Shade = open ? expandedRegionShade(node.depth) : parentShade;
  const out: Row[] = [{ kind: 'folder', node, depth: node.depth, shade }];

  if (!open) return out;

  // Members sit one level below the folder that stores them — same
  // depth the mirror (listing.rs) stamps them with.
  for (const file of node.files) out.push({ kind: 'file', file, depth: node.depth + 1, shade });
  for (const child of node.children) out.push(...folderRows(child, shade));
  return out;
}

const rows = computed<Row[]>(() => {
  const { loose, roots } = tree.value;
  return [
    ...loose.map((file): Row => ({ kind: 'file', file, depth: 0, shade: 0 })),
    ...roots.flatMap((root): Row[] => folderRows(root, 0))
  ];
});

const allSelected = computed(
  () => files.value.length > 0 && selectedIds.value.length === files.value.length
);

const someSelected = computed(() => selectedIds.value.length > 0 && !allSelected.value);
const selectedCount = computed(() => selectedIds.value.length);

watch(
  () => files.value.map((file) => file.id),
  (fileIds) => {
    selectedIds.value = selectedIds.value.filter((id) => fileIds.includes(id));
  },
  { immediate: true }
);

function isSelected(id: number) {
  return selectedIds.value.includes(id);
}

function toggleFile(id: number) {
  if (isSelected(id)) {
    selectedIds.value = selectedIds.value.filter((selectedId) => selectedId !== id);
  } else {
    selectedIds.value = [...selectedIds.value, id];
  }
}

/// Cosmetic grouping: selection always operates on individual file ids,
/// so folder rows toggle their whole subtree in bulk but own nothing
/// themselves — members hidden inside collapsed subfolders included.
function nodeMemberIds(node: FolderNode): number[] {
  return [
    ...node.files.map((file) => file.id),
    ...node.children.flatMap((child) => nodeMemberIds(child))
  ];
}

type CheckState = 'checked' | 'mixed' | 'unchecked';

function nodeCheckState(node: FolderNode): CheckState {
  const memberIds = nodeMemberIds(node);

  if (memberIds.every((id) => selectedIds.value.includes(id))) return 'checked';
  if (memberIds.some((id) => selectedIds.value.includes(id))) return 'mixed';
  return 'unchecked';
}

/// Each folder row's check state, computed once per row — the folder
/// checkboxes need it for both their aria-checked and their icon.
const folderCheckStates = computed(() => {
  const states = new Map<FolderNode, CheckState>();

  for (const row of rows.value) {
    if (row.kind === 'folder') states.set(row.node, nodeCheckState(row.node));
  }

  return states;
});

/// The checkbox's exact aria-checked spelling for a check state; a row
/// missing from the map (it is not a folder row) reads as unchecked.
function checkStateAria(state: CheckState | undefined) {
  return state === 'checked' ? 'true' : state === 'mixed' ? 'mixed' : 'false';
}

function toggleNode(node: FolderNode) {
  const memberIds = nodeMemberIds(node);

  if (nodeCheckState(node) === 'checked') {
    selectedIds.value = selectedIds.value.filter((id) => !memberIds.includes(id));
  } else {
    const unselected = memberIds.filter((id) => !selectedIds.value.includes(id));
    selectedIds.value = [...selectedIds.value, ...unselected];
  }
}

/// Rows show the file's own name; where it hangs under the root is the
/// tree's job now. The full path stays in the row title.
function rowTitle(row: Row) {
  if (row.kind === 'file') return row.file.path;
  return row.node.subPath
    ? `${row.node.folderRoot}/${row.node.subPath}`
    : row.node.folderRoot;
}

function toggleAll() {
  selectedIds.value = allSelected.value ? [] : files.value.map((file) => file.id);
}

function removeOne(id: number) {
  selectedIds.value = selectedIds.value.filter((selectedId) => selectedId !== id);
  remove(id);
}

function removeSelected() {
  if (!selectedIds.value.length) return;

  const ids = [...selectedIds.value];
  selectedIds.value = [];
  removeMany(ids);
}
</script>

<template>
  <section v-if="files.length" class="file-table" aria-label="Shared files" :aria-busy="busy">
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

    <ul class="file-rows thin-scrollbar" role="rowgroup">
      <template v-for="row in rows" :key="row.kind === 'file' ? row.file.id : `${row.node.folderRoot}/${row.node.subPath}`">
        <li
          v-if="row.kind === 'folder'"
          class="folder-row"
          role="row"
          :style="`--depth:${row.depth}`"
          :class="{ 'shade-1': row.shade === 1 }"
          :title="rowTitle(row)"
        >
          <button
            class="checkbox-control"
            type="button"
            role="checkbox"
            :aria-checked="checkStateAria(folderCheckStates.get(row.node))"
            :aria-label="`Select or deselect all files in ${row.node.name}`"
            :disabled="busy"
            @click="toggleNode(row.node)"
          >
            <CheckedIcon v-if="folderCheckStates.get(row.node) === 'checked'" />
            <UncheckedIcon v-else />
          </button>

          <button
            class="folder-chevron"
            :class="{ 'is-collapsed': !isExpanded(row.node) }"
            type="button"
            :aria-expanded="isExpanded(row.node) ? 'true' : 'false'"
            :aria-label="`${isExpanded(row.node) ? 'Collapse' : 'Expand'} ${row.node.name}`"
            @click="toggleFolder(row.node)"
          >
            <ChevronIcon />
          </button>

          <span class="folder-name">{{ row.node.name }}</span>
        </li>

        <li
          v-else
          class="file-row"
          :class="{ 'is-selected': isSelected(row.file.id), 'shade-1': row.shade === 1 }"
          :title="rowTitle(row)"
          :style="`--depth:${row.depth}`"
          role="row"
          :aria-selected="isSelected(row.file.id)"
          @click="toggleFile(row.file.id)"
        >
          <button
            type="button"
            role="checkbox"
            :aria-checked="isSelected(row.file.id)"
            :aria-label="`${isSelected(row.file.id) ? 'Deselect' : 'Select'} ${row.file.name}`"
            :disabled="busy"
            @click.stop="toggleFile(row.file.id)"
          >
            <CheckedIcon v-if="isSelected(row.file.id)" />
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
      </template>
    </ul>
  </section>
</template>

<style scoped>
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
  min-height: 48px;
  padding: 0 10px;
}

.files-header {
  border-bottom: 2px solid var(--ink);
  font-size: 1rem;
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
   geometry — the name text indents 16px per depth while the controls
   keep their grid columns (the row sets a matching `--depth`). */
.folder-row,
.file-row {
  font-size: 0.875rem;
}

.folder-name,
.file-name {
  padding-left: calc(16px * var(--depth, 0));
}

.folder-row {
  display: grid;
  grid-template-columns: 32px 32px minmax(0, 1fr);
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
