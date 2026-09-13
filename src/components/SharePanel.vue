<script setup lang="ts">
import { open } from '@tauri-apps/plugin-dialog';
import { ref, watch } from 'vue';
import ChevronIcon from '../assets/chevron-compact-up.svg';
import CopyIcon from '../assets/copy.svg';
import CopiedIcon from '../assets/square-rounded-check.svg';
import FileList from './FileList.vue';
import { useDragDrop } from '../composables/useDragDrop';
import { useSharer } from '../composables/useSharer';

const { files, info, qrSvg, busy, addPaths } = useSharer();
const { dragging } = useDragDrop(addPaths);

const copied = ref(false);
const selecting = ref(false);
const filesExpanded = ref(false);

watch(
  () => info.value.running,
  (running) => {
    if (!running) filesExpanded.value = false;
  }
);

async function chooseFiles() {
  if (busy.value || selecting.value) return;

  selecting.value = true;
  try {
    const selected = await open({
      title: 'Choose files to share',
      multiple: true,
      directory: false
    });

    if (selected) await addPaths(selected);
  } finally {
    selecting.value = false;
  }
}

async function copyUrl() {
  if (!info.value.url) return;
  try {
    await navigator.clipboard.writeText(info.value.url);
    copied.value = true;
    setTimeout(() => (copied.value = false), 1500);
  } catch {
    // Clipboard can be denied; the URL is selectable text anyway.
  }
}
</script>

<template>
  <button
    v-if="!info.running"
    class="dropzone"
    :class="{ 'is-dragging': dragging }"
    type="button"
    :disabled="busy || selecting"
    :aria-busy="busy || selecting"
    @click="chooseFiles"
  >
    <span class="drop-title">Click or Drop files here</span>
  </button>

  <template v-else>
    <section
      class="dropzone dropzone--active"
      :class="{ 'is-dragging': dragging }"
      aria-label="Network sharing details"
      :aria-busy="busy || selecting"
      @click="chooseFiles"
    >
      <h2>Sharing to your network</h2>

      <div class="qr" v-html="qrSvg"></div>

      <p class="url-row">
        <code class="url">{{ info.url }}</code>

        <button
          type="button"
          :aria-label="copied ? 'Share URL copied' : 'Copy share URL'"
          :title="copied ? 'Copied' : 'Copy share URL'"
          :disabled="busy || selecting"
          @click.stop="copyUrl"
        >
          <CopiedIcon v-if="copied" />
          <CopyIcon v-else />

          <span class="sr-only">{{ copied ? 'Copied' : 'Copy URL' }}</span>
        </button>
      </p>
    </section>

    <Transition name="files">
      <div v-if="filesExpanded && files.length" class="file-list-transition">
        <FileList />
      </div>
    </Transition>

    <button
      class="files-toggle"
      type="button"
      :aria-expanded="filesExpanded"
      :aria-label="filesExpanded ? 'Hide shared files' : 'Show shared files'"
      :disabled="files.length === 0"
      @click="filesExpanded = !filesExpanded"
    >
      <ChevronIcon class="icon" :class="{ 'is-collapsed': !filesExpanded }" />
    </button>
  </template>
</template>

<style scoped>
.dropzone {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 1.5rem;
  width: 100%;
  min-height: 15rem;

  border: 2px dashed var(--ink);
  border-radius: var(--radius);

  background: var(--surface);
  transition: background-color 160ms ease;

  margin-bottom: 1.25rem;

  &.dropzone--active {
    margin-bottom: 0;
  }

  &:not(:disabled) {
    cursor: pointer;
  }

  &:hover,
  &.is-dragging {
    background: #f1f1f1;
  }

  h2 {
    font-size: 1rem;
    font-weight: 400;
  }
}

.drop-title {
  margin: 0;
  font-size: 1.25rem;
  text-align: center;
}

.qr {
  width: 16.5rem;
  height: 16.5rem;
}

.url-row {
  display: flex;
  gap: 0.75rem;
  align-items: center;
}

.url {
  font-family: 'DynaPuff', cursive;
  overflow: hidden;
  color: var(--ink);
  font-size: 1.25rem;
  font-weight: 600;
  text-overflow: ellipsis;
  text-decoration: underline;
}

.files-toggle {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 3rem;
  padding: 0;
  border: 0;
  background: none;
  cursor: pointer;
}

.files-toggle .icon.is-collapsed {
  transform: rotate(180deg);
}

.files-toggle:disabled {
  cursor: default;
  opacity: 0.45;
}

.file-list-transition {
  width: 100%;
  min-width: 0;
  display: grid;
  grid-template-rows: 1fr;
  margin-top: 16px;
}

.files-enter-active,
.files-leave-active {
  overflow: hidden;
  transition:
    grid-template-rows 280ms ease,
    opacity 180ms ease;
}

.files-enter-from,
.files-leave-to {
  grid-template-rows: 0fr;
  opacity: 0;
}

.files-enter-to,
.files-leave-from {
  grid-template-rows: 1fr;
  opacity: 1;
}

.file-list-transition :deep(.file-table) {
  min-height: 0;
  overflow: hidden;
}

.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
</style>
