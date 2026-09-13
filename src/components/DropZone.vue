<script setup lang="ts">
defineProps<{ dragging: boolean }>();
</script>

<template>
  <div class="dropzone" :class="{ 'is-dragging': dragging }">
    <p class="drop-title">Drop files here</p>
    <p class="drop-hint">
      Anyone on your Wi-Fi will be able to download them until you stop sharing.
    </p>
  </div>
</template>
