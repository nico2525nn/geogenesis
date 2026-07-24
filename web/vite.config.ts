import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  server: {
    port: 3000,
    open: true,
  },
  build: {
    target: 'es2020',
    outDir: 'dist',
    rollupOptions: {
      external: ['geogenesis-wasm'],
    },
  },
  optimizeDeps: {
    exclude: ['geogenesis-wasm'],
  },
  worker: {
    format: 'es',
  },
});
