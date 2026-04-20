import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

// `VITE_MMCP_SERVER_URL` picks the upstream mmcp-server during dev.
// Falls back to the value wired into the layout when unset.
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
