<script setup lang="ts">
import type { SharedFile } from '../types';
import { formatSize } from '../lib/format';

defineProps<{ files: SharedFile[] }>();
defineEmits<{ remove: [id: number] }>();
</script>

<template>
  <section v-if="files.length" class="files">
    <h2>Sharing</h2>
    <ul>
      <li v-for="f in files" :key="f.id" :title="f.path">
        <span class="file-name">{{ f.name }}</span>
        <span class="file-size">{{ formatSize(f.size) }}</span>
        <button
          class="file-remove"
          :aria-label="`Stop sharing ${f.name}`"
          @click="$emit('remove', f.id)"
        >
          ✕
        </button>
      </li>
    </ul>
  </section>
</template>
