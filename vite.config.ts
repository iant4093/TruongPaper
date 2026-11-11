import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { resolve } from 'path';

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // Rust build output is large and never needs frontend hot reload.
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    target: ['es2022', 'chrome110', 'safari16'],
    rollupOptions: {
      input: {
        main: resolve(__dirname, 'index.html'),
        tray: resolve(__dirname, 'tray.html'),
      },
    },
  },
});
