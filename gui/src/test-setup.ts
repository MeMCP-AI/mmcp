// Preload for `bun test`.
// Registers happy-dom globals and a Svelte-to-JS loader for `.svelte` files.
import { GlobalRegistrator } from '@happy-dom/global-registrator';
import { plugin } from 'bun';
import { SveltePlugin } from 'bun-plugin-svelte';

GlobalRegistrator.register();

plugin(SveltePlugin({ forceSide: 'client' }));
