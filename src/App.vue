<script setup lang="ts">
import { onMounted, ref } from 'vue';
import SharePanel from './components/SharePanel.vue';
import { useSharer } from './composables/useSharer';
import { useWindowHeight } from './composables/useWindowHeight';
import Header from './components/Header.vue';

const { failedAdds, error, init, dismissFailed } = useSharer();
const shell = ref<HTMLElement | null>(null);

useWindowHeight(shell);

onMounted(init);
</script>

<template>
  <main ref="shell" class="shell">
    <Header />

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

    <SharePanel />
  </main>
</template>

<style scoped>
.shell {
  display: flex;
  flex-direction: column;
  margin: 0;
  padding: 0 1.25rem;
}

.error {
  padding: 0.75rem;
  border: 2px solid var(--ink);
  border-radius: var(--radius);
  background: var(--surface);
  font-size: 0.8125rem;
  line-height: 1.3;
}

.failed {
  margin-top: 12px;
  padding: 12px 14px;
  border: 2px solid var(--ink);
  border-radius: var(--radius);
  background: var(--surface);
  font-size: 0.8125rem;
}

.failed-title {
  margin: 0 0 6px;
  font-weight: 600;
}

.failed ul {
  margin: 0 0 8px;
  padding-left: 18px;
}

.failed code {
  word-break: break-all;
  user-select: text;
}

.dismiss {
  padding: 0;
  border: 0;
  background: none;
  cursor: pointer;
  text-decoration: underline;
}
</style>
