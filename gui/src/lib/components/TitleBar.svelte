<script lang="ts">
  // VSCode-style custom title bar. Replaces the host-OS window
  // decorations (tauri.conf.json: decorations=false). A single
  // horizontal bar holds the app icon, the File/View/Help menus,
  // a draggable title region, and the min/max/close window
  // controls on the far right.

  import { BrainCircuit, ChevronRight } from 'lucide-svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { emit } from '@tauri-apps/api/event';
  import { pickDirectory, setReferencePoint } from '$lib/api/workspace';
  import ArchiveDialog from '$lib/components/ArchiveDialog.svelte';
  import { settingsStore, type ThemeMode } from '$lib/stores/settings.svelte';
  import { syncStore } from '$lib/stores/sync.svelte';
  import { openDiagnosticsWindow, openSettingsWindow } from '$lib/windows';

  type MenuId = 'file' | 'view' | 'help';

  let openMenu = $state<MenuId | null>(null);
  let themeSubOpen = $state(false);
  let barEl: HTMLElement | undefined = $state();
  let maximized = $state(false);
  let altHeld = $state(false);
  let archiveDialogMode = $state<'export' | 'import' | null>(null);

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

  // Alt-accelerator plumbing. Tracking the Alt key lets the labels
  // reveal their underlined hotkey only while the user is actually
  // reaching for one — matches the VSCode / Win32 convention.
  $effect(() => {
    const onDown = (e: KeyboardEvent) => {
      if (e.key === 'Alt') altHeld = true;
      if (e.altKey) {
        const k = e.key.toLowerCase();
        if (k === 'f') { toggle('file'); e.preventDefault(); return; }
        if (k === 'v') { toggle('view'); e.preventDefault(); return; }
        if (k === 'h') { toggle('help'); e.preventDefault(); return; }
      }
      if (openMenu && !e.altKey && e.key.length === 1) {
        const handled = runInMenuAccelerator(openMenu, e.key.toLowerCase());
        if (handled) e.preventDefault();
      }
      if (e.key === 'Escape' && openMenu) {
        openMenu = null;
        themeSubOpen = false;
      }
    };
    const onUp = (e: KeyboardEvent) => {
      if (e.key === 'Alt') altHeld = false;
    };
    const onBlur = () => (altHeld = false);
    window.addEventListener('keydown', onDown);
    window.addEventListener('keyup', onUp);
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('keydown', onDown);
      window.removeEventListener('keyup', onUp);
      window.removeEventListener('blur', onBlur);
    };
  });

  function runInMenuAccelerator(menu: MenuId, k: string): boolean {
    if (menu === 'file') {
      if (k === 'o') { void onOpenProject(); return true; }
      if (k === 'c' && settingsStore.values.reference_point) { void onClearProject(); return true; }
      if (k === 'p' && syncStore.configured && !syncStore.inFlight) { void onPull(); return true; }
      if (k === 'e') { void onExportArchive(); return true; }
      if (k === 'i') { void onImportArchive(); return true; }
      if (k === 's') { onOpenSettings(); return true; }
      if (k === 'q') { openMenu = null; void win.close(); return true; }
    } else if (menu === 'view') {
      if (!themeSubOpen) {
        if (k === 't') { themeSubOpen = true; return true; }
      } else {
        const pick = THEME_ACCEL[k];
        if (pick) {
          settingsStore.setTheme(pick);
          themeSubOpen = false;
          openMenu = null;
          return true;
        }
      }
    } else if (menu === 'help') {
      if (k === 'd') { onOpenDiagnostics(); return true; }
    }
    return false;
  }

  const THEME_ACCEL: Record<string, ThemeMode> = {
    d: 'dark',
    o: 'oled',
    r: 'dim-dark',
    i: 'dim',
    l: 'light',
    s: 'system'
  };

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

  function onExportArchive() {
    openMenu = null;
    archiveDialogMode = 'export';
  }

  function onImportArchive() {
    openMenu = null;
    archiveDialogMode = 'import';
  }

  function onOpenSettings() {
    openMenu = null;
    void openSettingsWindow();
  }

  function onOpenDiagnostics() {
    openMenu = null;
    void openDiagnosticsWindow();
  }

  const THEME_OPTIONS: { id: ThemeMode; label: string; accel: number }[] = [
    { id: 'oled', label: 'OLED', accel: 0 },
    { id: 'dark', label: 'Dark', accel: 0 },
    { id: 'dim-dark', label: 'Dim Dark', accel: 6 },
    { id: 'dim', label: 'Dim Light', accel: 2 },
    { id: 'light', label: 'Light', accel: 0 },
    { id: 'system', label: 'System', accel: 0 }
  ];
</script>

<header
  bind:this={barEl}
  data-tauri-drag-region
  style="--tb-h: 2rem; --tb-ctrl-w: calc(var(--tb-h) * 1.4375); --tb-glyph: calc(var(--tb-h) * 0.375); height: var(--tb-h);"
  class="relative flex shrink-0 select-none items-stretch border-b border-line bg-surface-1 text-[0.75rem] text-fg-muted {altHeld ? 'alt-held' : ''}"
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
      <u class="acc">F</u>ile
    </button>
    <button
      type="button"
      class="px-2 hover:bg-surface-2 hover:text-fg {openMenu === 'view' ? 'bg-surface-2 text-fg' : ''}"
      aria-haspopup="menu"
      aria-expanded={openMenu === 'view'}
      onclick={() => toggle('view')}
    >
      <u class="acc">V</u>iew
    </button>
    <button
      type="button"
      class="px-2 hover:bg-surface-2 hover:text-fg {openMenu === 'help' ? 'bg-surface-2 text-fg' : ''}"
      aria-haspopup="menu"
      aria-expanded={openMenu === 'help'}
      onclick={() => toggle('help')}
    >
      <u class="acc">H</u>elp
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
          <span><u class="acc">O</u>pen Project…</span>
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
          <span><u class="acc">C</u>lear Project Anchor</span>
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
          <span><u class="acc">P</u>ull Now</span>
        </button>
      </li>
      <li class="border-t border-line">
        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between px-3 py-1.5 text-left hover:bg-surface-2"
          onclick={onExportArchive}
        >
          <span><u class="acc">E</u>xport Archive…</span>
        </button>
      </li>
      <li>
        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between px-3 py-1.5 text-left hover:bg-surface-2"
          onclick={onImportArchive}
        >
          <span><u class="acc">I</u>mport Archive…</span>
        </button>
      </li>
      <li class="border-t border-line">
        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center justify-between px-3 py-1.5 text-left hover:bg-surface-2"
          onclick={onOpenSettings}
        >
          <span><u class="acc">S</u>ettings…</span>
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
          <span><u class="acc">Q</u>uit</span>
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
          <span><u class="acc">T</u>heme</span>
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
                  <span
                    >{opt.label.slice(0, opt.accel)}<u class="acc"
                      >{opt.label[opt.accel]}</u
                    >{opt.label.slice(opt.accel + 1)}</span
                  >
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
          <span><u class="acc">D</u>iagnose…</span>
        </button>
      </li>
    </ul>
  {/if}
</header>

{#if archiveDialogMode}
  <ArchiveDialog mode={archiveDialogMode} onClose={() => (archiveDialogMode = null)} />
{/if}

<style>
  /* Title-bar chrome glyphs share a 10x10 viewBox; size scales off
     --tb-glyph on the header so bar height is the single source of
     truth. Inline SVG over a webfont: guarantees pixel-aligned
     stroke widths and identical centering for every glyph. */
  .tb-glyph {
    width: var(--tb-glyph);
    height: var(--tb-glyph);
    stroke: currentColor;
    stroke-width: 1;
    fill: none;
    shape-rendering: crispEdges;
  }

  /* Alt-accelerator underline. The <u> marker sits in the label
     at all times (so reading order is stable), but the underline
     only appears while Alt is held — matches the Win32 / VSCode
     menu-bar convention. */
  .acc {
    text-decoration: none;
  }
  .alt-held .acc {
    text-decoration: underline;
  }
</style>
