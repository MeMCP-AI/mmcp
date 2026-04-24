<script lang="ts">
  // VSCode-style custom title bar. Replaces the host-OS window
  // decorations (tauri.conf.json: decorations=false). A single
  // horizontal bar holds the app icon, the File/View/Help menus,
  // a draggable title region, and the min/max/close window
  // controls on the far right.

  import { BrainCircuit, ChevronRight, Minus, Square, X } from 'lucide-svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { emit } from '@tauri-apps/api/event';
  import { pickDirectory, setReferencePoint } from '$lib/api/workspace';
  import { settingsStore, type ThemeMode } from '$lib/stores/settings.svelte';
  import { syncStore } from '$lib/stores/sync.svelte';
  import { openDiagnosticsWindow, openSettingsWindow } from '$lib/windows';

  type MenuId = 'file' | 'view' | 'help';

  let openMenu = $state<MenuId | null>(null);
  let themeSubOpen = $state(false);
  let barEl: HTMLElement | undefined = $state();
  let maximized = $state(false);

  const win = getCurrentWindow();

  $effect(() => {
    let unlisten: (() => void) | null = null;
    void (async () => {
      maximized = await win.isMaximized();
      unlisten = await win.onResized(async () => {
        maximized = await win.isMaximized();
      });
    })();
    return () => unlisten?.();
  });

  $effect(() => {
    if (!openMenu) return;
    const handler = (ev: MouseEvent) => {
      if (!barEl) return;
      if (!barEl.contains(ev.target as Node)) openMenu = null;
    };
    const id = setTimeout(() => document.addEventListener('mousedown', handler), 0);
    return () => {
      clearTimeout(id);
      document.removeEventListener('mousedown', handler);
    };
  });

  function toggle(id: MenuId) {
    openMenu = openMenu === id ? null : id;
    if (openMenu !== 'view') themeSubOpen = false;
  }

  async function onOpenProject() {
    openMenu = null;
    const picked = await pickDirectory(settingsStore.values.reference_point, 'Open project');
    if (!picked) return;
    settingsStore.setReferencePoint(picked);
    try {
      await setReferencePoint(picked);
      await emit('workspace:changed');
    } catch (err) {
      console.warn('open project failed', err);
    }
  }

  async function onClearProject() {
    openMenu = null;
    settingsStore.setReferencePoint(null);
    try {
      await setReferencePoint(null);
      await emit('workspace:changed');
    } catch (err) {
      console.warn('clear project failed', err);
    }
  }

  async function onPull() {
    openMenu = null;
    if (!syncStore.configured || syncStore.inFlight) return;
    await syncStore.pull();
  }

  function onOpenSettings() {
    openMenu = null;
    void openSettingsWindow();
  }

  function onOpenDiagnostics() {
    openMenu = null;
    void openDiagnosticsWindow();
  }

  const THEME_OPTIONS: { id: ThemeMode; label: string }[] = [
    { id: 'dark', label: 'Dark' },
    { id: 'oled', label: 'OLED' },
    { id: 'dim', label: 'Dim Light' },
    { id: 'light', label: 'Light' },
    { id: 'system', label: 'System' }
  ];
</script>

<header
  bind:this={barEl}
  data-tauri-drag-region
  class="relative flex h-8 shrink-0 select-none items-stretch border-b border-line bg-surface-1 text-[12px] text-fg-muted"
>
  <div data-tauri-drag-region class="flex items-center gap-1 pl-2 pr-1">
    <BrainCircuit size={14} class="text-fg" />
  </div>

  <!-- Menus -->
  <nav class="flex items-stretch">
    <button
      type="button"
      class="px-2 hover:bg-surface-2 hover:text-fg {openMenu === 'file' ? 'bg-surface-2 text-fg' : ''}"
      aria-haspopup="menu"
      aria-expanded={openMenu === 'file'}
      onclick={() => toggle('file')}
    >
      File
    </button>
    <button
      type="button"
      class="px-2 hover:bg-surface-2 hover:text-fg {openMenu === 'view' ? 'bg-surface-2 text-fg' : ''}"
      aria-haspopup="menu"
      aria-expanded={openMenu === 'view'}
      onclick={() => toggle('view')}
    >
      View
    </button>
    <button
      type="button"
      class="px-2 hover:bg-surface-2 hover:text-fg {openMenu === 'help' ? 'bg-surface-2 text-fg' : ''}"
      aria-haspopup="menu"
      aria-expanded={openMenu === 'help'}
      onclick={() => toggle('help')}
    >
      Help
    </button>
  </nav>

  <!-- Draggable title -->
  <div
    data-tauri-drag-region
    class="flex flex-1 items-center justify-center px-3 text-[11px] text-fg-subtle"
  >
    User MMCP GUI
  </div>

  <!-- Window controls (VSCode-style: tall rectangular, red-on-hover close) -->
  <div class="flex items-stretch">
    <button
      type="button"
      class="flex w-[46px] items-center justify-center hover:bg-surface-2 hover:text-fg"
      aria-label="Minimize"
      onclick={() => win.minimize()}
    >
      <Minus size={14} />
    </button>
    <button
      type="button"
      class="flex w-[46px] items-center justify-center hover:bg-surface-2 hover:text-fg"
      aria-label={maximized ? 'Restore' : 'Maximize'}
      onclick={() => win.toggleMaximize()}
    >
      {#if maximized}
        <!-- VSCode restore glyph: back square + front square slightly offset. -->
        <svg
          width="12"
          height="12"
          viewBox="0 0 12 12"
          fill="none"
          stroke="currentColor"
          stroke-width="1"
          aria-hidden="true"
        >
          <rect x="3" y="1" width="8" height="8" />
          <path d="M9 3 H1 V11 H9 V9" />
        </svg>
      {:else}
        <Square size={12} />
      {/if}
    </button>
    <button
      type="button"
      class="flex w-[46px] items-center justify-center hover:bg-red-600 hover:text-white"
      aria-label="Close"
      onclick={() => win.close()}
    >
      <X size={14} />
    </button>
  </div>

  <!-- Menu popovers -->
  {#if openMenu === 'file'}
    <ul
      class="absolute left-8 top-full z-40 mt-0 min-w-[220px] overflow-hidden rounded-md border border-line bg-surface-1 text-xs text-fg shadow-lg"
      role="menu"
    >
      <li>
        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between px-3 py-1.5 text-left hover:bg-surface-2"
          onclick={onOpenProject}
        >
          <span>Open Project…</span>
          <span class="text-[10px] text-fg-subtle">cwd</span>
        </button>
      </li>
      <li>
        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between px-3 py-1.5 text-left hover:bg-surface-2 disabled:text-fg-subtle disabled:hover:bg-transparent"
          disabled={!settingsStore.values.reference_point}
          onclick={onClearProject}
        >
          <span>Clear Project Anchor</span>
        </button>
      </li>
      <li class="border-t border-line">
        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between px-3 py-1.5 text-left hover:bg-surface-2 disabled:text-fg-subtle disabled:hover:bg-transparent"
          disabled={!syncStore.configured || syncStore.inFlight}
          onclick={onPull}
        >
          <span>Pull Now</span>
        </button>
      </li>
      <li>
        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between px-3 py-1.5 text-left hover:bg-surface-2"
          onclick={onOpenSettings}
        >
          <span>Settings…</span>
        </button>
      </li>
      <li class="border-t border-line">
        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between px-3 py-1.5 text-left hover:bg-surface-2"
          onclick={() => {
            openMenu = null;
            void win.close();
          }}
        >
          <span>Quit</span>
        </button>
      </li>
    </ul>
  {/if}

  {#if openMenu === 'view'}
    <ul
      class="absolute left-16 top-full z-40 mt-0 min-w-[180px] overflow-hidden rounded-md border border-line bg-surface-1 text-xs text-fg shadow-lg"
      role="menu"
    >
      <li class="relative">
        <button
          type="button"
          role="menuitem"
          aria-haspopup="menu"
          aria-expanded={themeSubOpen}
          class="flex w-full items-center justify-between gap-2 px-3 py-1.5 text-left hover:bg-surface-2 {themeSubOpen ? 'bg-surface-2' : ''}"
          onmouseenter={() => (themeSubOpen = true)}
          onclick={() => (themeSubOpen = !themeSubOpen)}
        >
          <span>Theme</span>
          <ChevronRight size={12} class="text-fg-subtle" />
        </button>
        {#if themeSubOpen}
          <ul
            class="absolute left-full top-0 ml-0 min-w-[180px] overflow-hidden rounded-md border border-line bg-surface-1 shadow-lg"
            role="menu"
          >
            {#each THEME_OPTIONS as opt (opt.id)}
              {@const active = settingsStore.values.theme === opt.id}
              <li>
                <button
                  type="button"
                  role="menuitemradio"
                  aria-checked={active}
                  class="flex w-full items-center justify-between px-3 py-1.5 text-left hover:bg-surface-2 {active ? 'text-selected-fg' : ''}"
                  onclick={() => {
                    settingsStore.setTheme(opt.id);
                    themeSubOpen = false;
                    openMenu = null;
                  }}
                >
                  <span>{opt.label}</span>
                  {#if active}<span class="text-[10px] text-selected-fg">●</span>{/if}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </li>
    </ul>
  {/if}

  {#if openMenu === 'help'}
    <ul
      class="absolute left-24 top-full z-40 mt-0 min-w-[220px] overflow-hidden rounded-md border border-line bg-surface-1 text-xs text-fg shadow-lg"
      role="menu"
    >
      <li>
        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between px-3 py-1.5 text-left hover:bg-surface-2"
          onclick={onOpenDiagnostics}
        >
          <span>Diagnose…</span>
        </button>
      </li>
    </ul>
  {/if}
</header>
