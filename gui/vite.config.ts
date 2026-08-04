import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

// Tauri dev server expects the frontend on a fixed port; 1420 is the
// Tauri default and the `tauri.conf.json` devUrl is pinned to it.
export default defineConfig({
  plugins: [tailwindcss(), sveltekit()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: '127.0.0.1',
    watch: {
      // Tauri's src-tauri/ recompiles on its own; ignore it here so
      // Vite doesn't trigger spurious reloads when cargo touches
      // target/.
      ignored: ['**/src-tauri/**']
    }
  },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    // Tauri 2 defaults to ES2022 for the webview; match it.
    target: 'es2022',
    // Vite 8's rolldown-based build no longer bundles esbuild; 'oxc'
    // is the new built-in Rust minifier and the documented default.
    minify: process.env.TAURI_ENV_DEBUG ? false : 'oxc',
    sourcemap: !!process.env.TAURI_ENV_DEBUG
  }
});
