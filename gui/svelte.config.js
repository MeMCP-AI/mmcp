import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    // adapter-static emits a pure SPA into `build/`, which Tauri
    // serves in-process. fallback: 'app.html' routes every path to
    // the SvelteKit client router. It must differ from the
    // pre-rendered root page's own output name (`index.html`) in
    // the same `build/` directory, or the fallback silently
    // clobbers that pre-rendered page.
    adapter: adapter({
      pages: 'build',
      assets: 'build',
      fallback: 'app.html',
      precompress: false,
      strict: true
    })
  }
};

export default config;
