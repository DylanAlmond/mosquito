<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import CheckedIcon from '../assets/square-rounded-check.svg';
import UncheckedIcon from '../assets/square-rounded.svg';
import TrashIcon from '../assets/trash-x.svg';
import { useSharer } from '../composables/useSharer';
import { groupSharedFiles, type FolderGroup } from '../lib/folderGroups';
import type { SharedFile } from '../types';

const { files, busy, remove, removeMany } = useSharer();

const selectedIds = ref<number[]>([]);

/// Grouping is derived at render time (ADR-0001); the store stays flat.
/// Members keep their position in the shared list, so groups appear in
/// first-appearance order. Loose files lead, ungrouped.
const grouped = computed(() => groupSharedFiles(files.value));

type Row = { kind: 'header'; group: FolderGroup } | { kind: 'file'; file: SharedFile };

const rows = computed<Row[]>(() => {
  const { loose, groups } = grouped.value;
  return [
    ...loose.map((file): Row => ({ kind: 'file', file })),
    ...groups.flatMap((group): Row[] => [
      { kind: 'header', group },
      ...group.files.map((file): Row => ({ kind: 'file', file }))
    ])
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
/// so group headers toggle members in bulk but own nothing themselves.
function groupCheckState(group: FolderGroup): 'checked' | 'mixed' | 'unchecked' {
  if (group.files.every((file) => selectedIds.value.includes(file.id))) return 'checked';
  if (group.files.some((file) => selectedIds.value.includes(file.id))) return 'mixed';
  return 'unchecked';
}

function toggleGroup(group: FolderGroup) {
  const memberIds = group.files.map((file) => file.id);

  if (groupCheckState(group) === 'checked') {
    selectedIds.value = selectedIds.value.filter((id) => !memberIds.includes(id));
  } else {
    const unselected = memberIds.filter((id) => !selectedIds.value.includes(id));
    selectedIds.value = [...selectedIds.value, ...unselected];
  }
}

/// Inside a group the file shows where it hangs under the folder root;
/// loose files keep their basename (the full path stays in the row title).
function memberTitle(file: SharedFile) {
  return file.provenance?.relative_path ?? file.name;
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
      <template v-for="row in rows" :key="row.kind === 'header' ? row.group.folderRoot : row.file.id">
        <li
          v-if="row.kind === 'header'"
          class="group-header"
          role="row"
          :title="row.group.folderRoot"
        >
          <button
            class="checkbox-control"
            type="button"
            role="checkbox"
            :aria-checked="groupCheckState(row.group) === 'checked' ? 'true' : groupCheckState(row.group) === 'mixed' ? 'mixed' : 'false'"
            :aria-label="`Select or deselect all files in ${row.group.name}`"
            :disabled="busy"
            @click="toggleGroup(row.group)"
          >
            <CheckedIcon v-if="groupCheckState(row.group) === 'checked'" />
            <UncheckedIcon v-else />
          </button>

          <span class="group-name">{{ row.group.name }}</span>
        </li>

        <li
          v-else
          class="file-row"
          :class="{ 'is-selected': isSelected(row.file.id) }"
          :title="row.file.path"
          role="row"
          :aria-selected="isSelected(row.file.id)"
          @click="toggleFile(row.file.id)"
        >
          <button
            type="button"
            role="checkbox"
            :aria-checked="isSelected(row.file.id)"
            :aria-label="`${isSelected(row.file.id) ? 'Deselect' : 'Select'} ${memberTitle(row.file)}`"
            :disabled="busy"
            @click.stop="toggleFile(row.file.id)"
          >
            <CheckedIcon v-if="isSelected(row.file.id)" />
            <UncheckedIcon v-else />
          </button>

          <span class="file-name">{{ memberTitle(row.file) }}</span>

          <button
            type="button"
            :aria-label="`Delete ${memberTitle(row.file)}`"
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

/* Rows and group headers are flat siblings; every row but the first gets
   a top border, no matter what kind of row precedes it. */
.file-rows > li:not(:first-child) {
  border-top: 1px solid var(--ink);
}

.group-header {
  grid-template-columns: 32px minmax(0, 1fr);
  min-height: 38px;
  padding: 0 10px;
  background: #f1f1f1;
  color: var(--ink);
  font-size: 0.875rem;
  font-weight: 600;
  line-height: 1;
}

.group-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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
