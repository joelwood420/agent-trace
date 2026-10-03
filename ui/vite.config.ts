import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// Settings follow the Tauri guide for Vite: https://v2.tauri.app/start/frontend/vite/
export default defineConfig({
  plugins: [react()],
  // Keep Rust compiler output visible in the terminal during `cargo tauri dev`.
  clearScreen: false,
  server: {
    // Must match build.devUrl in src-tauri/tauri.conf.json.
    port: 5173,
    strictPort: true,
    // Only listen on this machine.
    host: 'localhost',
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
  build: {
    // WebView2 on Windows is Chromium based.
    target: 'chrome105',
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
})
