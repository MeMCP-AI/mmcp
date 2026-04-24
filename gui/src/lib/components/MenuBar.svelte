<script lang="ts">
  // App-level menu bar. Sits at the very top of every window and
  // holds the desktop-convention File / View / Help menus. The
  // individual items route through the same Tauri commands the
  // rest of the GUI already uses (pick_directory for Open
  // Project, window.close for Quit, etc.).
  //
  // Menus are click-to-open popovers — a single menu can be
  // active at a time; opening one closes the others.

  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { pickDirectory, setReferencePoint } from '$lib/api/workspace';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { syncStore } from '$lib/stores/sync.svelte';
  import { openDiagnosticsWindow, openSettingsWindow } from '$lib/windows';
  import { emit } from '@tauri-apps/api/event';

  type MenuId = 'file' | 'view' | 'help';

  let openMenu = $state<MenuId | null>(null);
  let barEl: HTMLElement | undefined = $state();

  // Outside-click closer, registered only while a menu is open.
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
  }

  async function onOpenProject() {
    openMenu = null;
    const picked = await pickDirectory(
      settingsStore.values.reference_point,
      'Open project'
    );
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

  async function onQuit() {
    openMenu = null;
    await getCurrentWindow().close();
  }
</script>

<nav
  bind:this={barEl}
  class="relative flex h-7 shrink-0 items-center gap-0.5 border-b border-line bg-surface-1 px-1 text-[12px] text-fg-muted"
>
  <!-- File -->
  <button
    type="button"
    class="rounded-sm px-2 py-0.5 hover:bg-surface-2 hover:text-fg {openMenu === 'file' ? 'bg-surface-2 text-fg' : ''}"
    aria-haspopup="menu"
    aria-expanded={openMenu === 'file'}
    onclick={() => toggle('file')}
  >
    File
  </button>
  {#if openMenu === 'file'}
    <ul
      class="absolute left-0 top-full z-40 mt-0 min-w-[220px] overflow-hidden rounded-md border border-line bg-surface-1 text-xs text-fg shadow-lg"
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
          onclick={onQuit}
        >
          <span>Quit</span>
        </button>
      </li>
    </ul>
  {/if}

  <!-- View -->
  <button
    type="button"
    class="rounded-sm px-2 py-0.5 hover:bg-surface-2 hover:text-fg {openMenu === 'view' ? 'bg-surface-2 text-fg' : ''}"
    aria-haspopup="menu"
    aria-expanded={openMenu === 'view'}
    onclick={() => toggle('view')}
  >
    View
  </button>
  {#if openMenu === 'view'}
    <ul
      class="absolute left-12 top-full z-40 mt-0 min-w-[220px] overflow-hidden rounded-md border border-line bg-surface-1 text-xs text-fg shadow-lg"
      role="menu"
    >
      {#each [
        { id: 'dark' as const, label: 'Theme: Dark' },
        { id: 'light' as const, label: 'Theme: Light' },
        { id: 'system' as const, label: 'Theme: System' }
      ] as opt (opt.id)}
        {@const active = settingsStore.values.theme === opt.id}
        <li>
          <button
            type="button"
            role="menuitemradio"
            aria-checked={active}
            class="flex w-full items-center justify-between px-3 py-1.5 text-left hover:bg-surface-2 {active ? 'text-selected-fg' : ''}"
            onclick={() => {
              settingsStore.setTheme(opt.id);
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

  <!-- Help -->
  <button
    type="button"
    class="rounded-sm px-2 py-0.5 hover:bg-surface-2 hover:text-fg {openMenu === 'help' ? 'bg-surface-2 text-fg' : ''}"
    aria-haspopup="menu"
    aria-expanded={openMenu === 'help'}
    onclick={() => toggle('help')}
  >
    Help
  </button>
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

  <span class="ml-auto select-none text-[11px] text-fg-subtle">User MMCP GUI</span>
</nav>
