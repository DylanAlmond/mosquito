<script setup lang="ts">
/// The Choose File / Choose Folder affordances, shared by the stopped
/// and running states. Picking folders wholesale is its own affordance,
/// not a hybrid dialog mixed with the file picker (ticket #4).
defineProps<{ disabled: boolean }>();

defineEmits<{ 'choose-files': []; 'choose-folder': [] }>();
</script>

<template>
  <section class="choose-section">
    <button
      class="choose-button"
      type="button"
      aria-label="Choose file"
      :disabled="disabled"
      @click="$emit('choose-files')"
    >
      Choose File
    </button>

    <span>or</span>

    <button
      class="choose-button"
      type="button"
      aria-label="Choose folder"
      :disabled="disabled"
      @click="$emit('choose-folder')"
    >
      Choose Folder
    </button>
  </section>
</template>

<style scoped>
.choose-section {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 1rem;
  width: 100%;
  margin: 1rem 0;
  color: var(--ink);
  font-size: 0.9375rem;
}

.choose-button {
  display: flex;
  align-items: center;
  justify-content: center;
  width: fit-content;
  height: 2.25rem;
  padding: 0;
  border: 0;
  text-decoration: underline;
  cursor: pointer;
}

.choose-button:disabled {
  cursor: default;
  opacity: 0.45;
}
</style>
