<script setup lang="ts">
import { getCurrentWindow } from '@tauri-apps/api/window';
import X from '../assets/x.svg';
const appWindow = getCurrentWindow();
</script>

<template>
  <header class="header" data-tauri-drag-region="deep">
    <h1>Mosquito</h1>
    <button
      class="window-close"
      type="button"
      aria-label="Close Mosquito"
      @click="appWindow.close()"
    >
      <X />
    </button>
  </header>
</template>

<style scoped>
.header {
  position: relative;
  height: 48px;
  flex: 0 0 48px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.header h1 {
  margin: 0;
  font-size: 1rem;
  font-weight: 400;
  line-height: 1;
}

.window-close {
  position: absolute;
  top: 12px;
  right: 0;
  width: 24px;
  height: 24px;
  padding: 0;
  border: 0;
  background: none;
  cursor: pointer;
}

.window-close img {
  display: block;
  width: 1.5rem;
  height: 1.5rem;
}
</style>
