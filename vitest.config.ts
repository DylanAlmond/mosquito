import { defineConfig } from 'vitest/config';
import vue from '@vitejs/plugin-vue';
import svgLoader from 'vite-svg-loader';

export default defineConfig({
  plugins: [vue(), svgLoader()],
  test: {
    environment: 'happy-dom',
    // Playwright owns the .spec.ts files under harness/web.
    exclude: ['node_modules/**', 'harness/**']
  }
});
