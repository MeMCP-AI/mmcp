import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

// `VITE_MMCP_SERVER_URL` picks the mmcp-server the dev server calls.
// The server must list `http://127.0.0.1:3000` in `MMCP_CORS_ALLOWED_ORIGINS`.
export default defineConfig({
  plugins: [tailwindcss(), sveltekit()],
  clearScreen: false,
  server: {
    port: 3000,
    strictPort: true,
    host: '127.0.0.1'
  },
  envPrefix: ['VITE_'],
  build: {
    target: 'es2022'
  }
});
