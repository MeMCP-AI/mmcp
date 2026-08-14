<script lang="ts">
  // Thin custom chrome for child windows (Settings, Diagnostics,
  // future singletons). No menu bar — those are main-window
  // concerns — just a drag region with the window title and the
  // three Windows control buttons. Shares the responsive sizing
  // model from TitleBar: --tb-h drives control width and glyph
  // size so every window looks proportional to the main one.

  import { getCurrentWindow } from '@tauri-apps/api/window';

  interface Props {
    title: string;
  }

  let { title }: Props = $props();
  let maximized = $state(false);
  const win = getCurrentWindow();

  $effect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void (async () => {
      maximized = await win.isMaximized();
      const off = await win.onResized(async () => {
        maximized = await win.isMaximized();
      });
      if (cancelled) off();
      else unlisten = off;
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  });
</script>

<header
  data-tauri-drag-region
  style="--tb-h: 2rem; --tb-ctrl-w: calc(var(--tb-h) * 1.4375); --tb-glyph: calc(var(--tb-h) * 0.375); height: var(--tb-h);"
  class="relative flex shrink-0 select-none items-stretch border-b border-line bg-surface-1 text-[0.75rem] text-fg-muted"
>
  <div
    data-tauri-drag-region
    class="flex flex-1 items-center px-3 text-[11px] text-fg-subtle"
  >
    {title}
  </div>

  <div class="flex items-stretch">
    <button
      type="button"
      style="width: var(--tb-ctrl-w);"
      class="flex items-center justify-center hover:bg-surface-2 hover:text-fg"
      aria-label="Minimize"
      onclick={() => win.minimize()}
    >
      <svg class="tb-glyph" viewBox="0 0 10 10" aria-hidden="true"
        ><line x1="1" y1="5" x2="9" y2="5" /></svg>
    </button>
    <button
      type="button"
      style="width: var(--tb-ctrl-w);"
      class="flex items-center justify-center hover:bg-surface-2 hover:text-fg"
      aria-label={maximized ? 'Restore' : 'Maximize'}
      onclick={() => win.toggleMaximize()}
    >
      {#if maximized}
        <svg class="tb-glyph" viewBox="0 0 10 10" aria-hidden="true">
          <rect x="3" y="1" width="6" height="6" />
          <path d="M1 3 H7 V9 H1 Z" />
        </svg>
      {:else}
        <svg class="tb-glyph" viewBox="0 0 10 10" aria-hidden="true">
          <rect x="1" y="1" width="8" height="8" />
        </svg>
      {/if}
    </button>
    <button
      type="button"
      style="width: var(--tb-ctrl-w);"
      class="flex items-center justify-center hover:bg-red-600 hover:text-white"
      aria-label="Close"
      onclick={() => win.close()}
    >
      <svg class="tb-glyph" viewBox="0 0 10 10" aria-hidden="true">
        <line x1="1" y1="1" x2="9" y2="9" />
        <line x1="9" y1="1" x2="1" y2="9" />
      </svg>
    </button>
  </div>
</header>

<style>
  .tb-glyph {
    width: var(--tb-glyph);
    height: var(--tb-glyph);
    stroke: currentColor;
    stroke-width: 1;
    fill: none;
    shape-rendering: crispEdges;
  }
</style>
