import react from '@vitejs/plugin-react'
import { defineConfig, type Plugin } from 'vite'

/**
 * The app runs with Tauri's `freezePrototype` on, which freezes
 * `Object.prototype`. d3-color (used by React Flow for zoom and pan) does
 * `prototype.constructor = constructor` on a plain object, and assigning a
 * property that a frozen prototype already has throws in strict mode, so the
 * whole UI failed to load. This rewrites that one statement to
 * `Object.defineProperty`, which is allowed and has the same effect.
 * If d3-color changes and the statement is gone, the build fails so the fix
 * gets looked at again. See docs/DECISIONS.md.
 */
function d3ColorFrozenPrototypeFix(): Plugin {
  const target = 'prototype.constructor = constructor;'
  const replacement =
    "Object.defineProperty(prototype, 'constructor', { value: constructor, writable: true, configurable: true });"
  return {
    name: 'snitchcraft-d3-color-frozen-prototype',
    transform(code, id) {
      const path = id.split('?')[0]?.replaceAll('\\', '/') ?? ''
      if (!path.endsWith('/d3-color/src/define.js')) return null
      if (!code.includes(target)) {
        this.error('d3-color/src/define.js changed: review the freezePrototype fix in vite.config.ts')
      }
      return { code: code.replace(target, replacement), map: null }
    },
  }
}

// Settings follow the Tauri guide for Vite: https://v2.tauri.app/start/frontend/vite/
export default defineConfig({
  plugins: [react(), d3ColorFrozenPrototypeFix()],
  // The dev server pre-bundles dependencies separately, so the fix must
  // apply there too.
  optimizeDeps: {
    rolldownOptions: {
      plugins: [d3ColorFrozenPrototypeFix()],
    },
  },
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
