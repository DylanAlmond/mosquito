<script setup lang="ts">
import { onMounted } from 'vue';
import DropZone from './components/DropZone.vue';
import FileList from './components/FileList.vue';
import SharePanel from './components/SharePanel.vue';
import { useDragDrop } from './composables/useDragDrop';
import { useSharer } from './composables/useSharer';

const { files, info, qrSvg, failedAdds, error, busy, init, addPaths, remove, stop, dismissFailed } =
  useSharer();

// Dropping is the start button; the composable hands paths straight through.
const { dragging } = useDragDrop(addPaths);

onMounted(init);
</script>

<template>
  <main class="shell">
    <header class="header">
      <h1>🦟 Mosquito</h1>
      <p class="tagline">Bite-sized file sharing.</p>
    </header>

    <DropZone :dragging="dragging" />

    <p v-if="error" class="error" role="alert">{{ error }}</p>

    <section v-if="failedAdds.length" class="failed">
      <p class="failed-title">Some items couldn't be shared:</p>
      <ul>
        <li v-for="f in failedAdds" :key="f.path">
          <code>{{ f.path }}</code> — {{ f.error }}
        </li>
      </ul>
      <button class="dismiss" @click="dismissFailed">Dismiss</button>
    </section>

    <FileList :files="files" @remove="remove" />

    <SharePanel :info="info" :qr-svg="qrSvg" :busy="busy" @stop="stop" />
  </main>
</template>
