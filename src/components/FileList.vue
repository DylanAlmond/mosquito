<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import CheckedIcon from '../assets/square-rounded-check.svg';
import UncheckedIcon from '../assets/square-rounded.svg';
import TrashIcon from '../assets/trash-x.svg';
import { useSharer } from '../composables/useSharer';

const { files, busy, remove, removeMany } = useSharer();

const selectedIds = ref<number[]>([]);

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
      <li
        v-for="file in files"
        :key="file.id"
        class="file-row"
        :class="{ 'is-selected': isSelected(file.id) }"
        :title="file.path"
        role="row"
        :aria-selected="isSelected(file.id)"
        @click="toggleFile(file.id)"
      >
        <button
          type="button"
          role="checkbox"
          :aria-checked="isSelected(file.id)"
          :aria-label="`${isSelected(file.id) ? 'Deselect' : 'Select'} ${file.name}`"
          :disabled="busy"
          @click.stop="toggleFile(file.id)"
        >
          <CheckedIcon v-if="isSelected(file.id)" />
          <UncheckedIcon v-else />
        </button>

        <span class="file-name">{{ file.name }}</span>

        <button
          type="button"
          :aria-label="`Delete ${file.name}`"
          :disabled="busy"
          @click.stop="removeOne(file.id)"
        >
          <TrashIcon />
        </button>
      </li>
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

.file-row + .file-row {
  border-top: 1px solid var(--ink);
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
