import { defineConfig } from 'vitest/config';
import vue from '@vitejs/plugin-vue';
import svgLoader from 'vite-svg-loader';

export default defineConfig({
  plugins: [vue(), svgLoader()],
  test: {
    environment: 'happy-dom'
  }
});
