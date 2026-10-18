<script setup lang="ts">
import { open, type OpenDialogOptions } from '@tauri-apps/plugin-dialog';
import { computed, ref, watch } from 'vue';
import ChevronIcon from '../assets/chevron-compact-up.svg';
import CopyIcon from '../assets/copy.svg';
import CopiedIcon from '../assets/square-rounded-check.svg';
import ChooseSection from './ChooseSection.vue';
import FileList from './FileList.vue';
import { useDragDrop } from '../composables/useDragDrop';
import { countingRoots, useSharer } from '../composables/useSharer';

const { files, info, qrSvg, runs, addPaths, cancelRun } = useSharer();
const { dragging } = useDragDrop(addPaths);

const copied = ref(false);
const selecting = ref(false);
const filesExpanded = ref(false);

/// Roots still walking, per run — the progress board only shows in-flight
/// counting, never settled runs (ADR-0007: coarse per-root progress).
const activeRuns = computed(() =>
  runs.value
    .map((run) => ({ runId: run.run_id, counting: countingRoots(run) }))
    .filter((run) => run.counting.length > 0)
);

watch(
  () => info.value.running,
  (running) => {
    if (!running) filesExpanded.value = false;
  }
);

// One dialog flow at a time (a picker can't stack); everything else —
// picks, drops while a run is counting, removals — is allowed to
// overlap: further drops just become further import runs (ADR-0007).
async function pick(options: OpenDialogOptions) {
  if (selecting.value) return;

  selecting.value = true;
  try {
    const selected = await open(options);
    if (selected) await addPaths(Array.isArray(selected) ? selected : [selected]);
  } finally {
    selecting.value = false;
  }
}

// Picking folders wholesale is deliberately its own affordance, not a hybrid
// dialog mixed with the file picker (ticket #4).
const chooseFiles = () =>
  pick({ title: 'Choose files to share', multiple: true, directory: false });

const chooseFolder = () =>
  pick({ title: 'Choose folder to share', multiple: true, directory: true });

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
  <template v-if="!info.running">
    <button
      class="dropzone"
      :class="{ 'is-dragging': dragging }"
      type="button"
      :disabled="selecting"
      :aria-busy="selecting"
      @click="chooseFiles"
    >
      <span class="drop-title">Drop files or folders here</span>
    </button>

    <ChooseSection
      :disabled="selecting"
      @choose-files="chooseFiles"
      @choose-folder="chooseFolder"
    />
  </template>

  <template v-else>
    <section
      class="dropzone dropzone--active"
      :class="{ 'is-dragging': dragging }"
      aria-label="Network sharing details"
      :aria-busy="selecting"
      @click="chooseFiles"
    >
      <h2>Sharing to your network</h2>

      <div class="qr" v-html="qrSvg"></div>

      <button
        type="button"
        :aria-label="copied ? 'Share URL copied' : 'Copy share URL'"
        :title="copied ? 'Copied' : 'Copy share URL'"
        :disabled="selecting"
        class="url-row"
        @click.stop="copyUrl"
      >
        <span class="url-row">
          <code class="url">{{ info.url }}</code>

          <CopiedIcon v-if="copied" />
          <CopyIcon v-else />
        </span>

        <span class="sr-only">{{ copied ? 'Copied' : 'Copy URL' }}</span>
      </button>
    </section>

    <ChooseSection
      :disabled="selecting"
      @choose-files="chooseFiles"
      @choose-folder="chooseFolder"
    />

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

  <template v-if="activeRuns.length">
    <section class="import-runs" aria-label="Import progress">
      <div v-for="run in activeRuns" :key="run.runId" class="import-run">
        <p v-for="root in run.counting" :key="root.path" class="import-row">
          counting <code>{{ root.path }}</code> — {{ root.files }} entries
        </p>

        <button
          type="button"
          class="cancel"
          :aria-label="`Cancel import of ${run.counting.map((root) => root.path).join(', ')}`"
          @click="cancelRun(run.runId)"
        >
          Cancel
        </button>
      </div>
    </section>
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

.import-runs {
  display: grid;
  gap: 10px;
  margin-top: 12px;
}

.import-run {
  display: grid;
  gap: 6px;
  justify-items: start;
}

.import-row {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin: 0;
  padding: 10px 14px;
  width: 100%;

  border: 2px solid var(--ink);
  border-radius: var(--radius);
  background: var(--surface);
  font-size: 0.8125rem;
  line-height: 1.3;
}

.import-row code {
  word-break: break-all;
  user-select: text;
}

.cancel {
  padding: 0;
  border: 0;
  background: none;
  cursor: pointer;
  text-decoration: underline;
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
