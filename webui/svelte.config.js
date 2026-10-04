import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    // Pure SPA emitted into `build/`.
    // mmcp-server serves it on its own origin through `serve --webui-dir`.
    // The SPA fallback lets client-side routing cover every in-app navigation.
    adapter: adapter({
      pages: 'build',
      assets: 'build',
      fallback: 'index.html',
      precompress: false,
      strict: true
    }),
    typescript: {
      // The browser program leaves the test files out, since they import `bun:test`.
      // `tsconfig.test.json` checks them with the Bun types.
      // Paths are relative to the generated `.svelte-kit/tsconfig.json`.
      config: (config) => {
        config.exclude.push('../src/**/*.test.ts');
      }
    }
  }
};

export default config;
