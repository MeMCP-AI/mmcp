<script lang="ts">
  // App-shell top bar. Hosts the theme picker — a popover that
  // lists every available theme so the user can jump directly
  // instead of cycling blind. Settings + Diagnose now live in
  // CommonFooter, so the navbar only carries the app label and
  // the theme control.

  import { Check, ChevronDown, Monitor, Moon, Sun } from 'lucide-svelte';
  import { settingsStore, type ThemeMode } from '$lib/stores/settings.svelte';

  interface ThemeOption {
    id: ThemeMode;
    label: string;
    Icon: typeof Sun;
    blurb: string;
  }

  const OPTIONS: ThemeOption[] = [
    { id: 'dark', label: 'Dark', Icon: Moon, blurb: 'Default night palette.' },
    { id: 'light', label: 'Light', Icon: Sun, blurb: 'Bright palette for daytime.' },
    { id: 'system', label: 'System', Icon: Monitor, blurb: "Follow the OS preference." }
  ];

  const theme = $derived(settingsStore.values.theme);
  const active = $derived(OPTIONS.find((o) => o.id === theme) ?? OPTIONS[0]);

  let open = $state(false);
  let anchorEl: HTMLElement | undefined = $state();

  // Close on outside click. Registering on `open` means we only
  // pay the listener cost while the popover is up.
  $effect(() => {
    if (!open) return;
    const handler = (ev: MouseEvent) => {
      if (!anchorEl) return;
      if (!anchorEl.contains(ev.target as Node)) {
        open = false;
      }
    };
    // Defer to the next microtask so the click that opened the
    // popover doesn't immediately close it.
    const id = setTimeout(() => document.addEventListener('mousedown', handler), 0);
    return () => {
      clearTimeout(id);
      document.removeEventListener('mousedown', handler);
    };
  });

  function pick(mode: ThemeMode) {
    settingsStore.setTheme(mode);
    open = false;
  }
</script>

<header
  class="flex h-10 shrink-0 items-center gap-3 border-b border-line bg-surface-1 px-3 sm:px-4"
>
  <span class="select-none text-sm font-semibold tracking-tight text-fg">mmcp</span>

  <div class="ml-auto relative" bind:this={anchorEl}>
    <button
      type="button"
      class="inline-flex items-center gap-1 rounded-md border border-line bg-surface-0 px-2 py-0.5 text-[11px] text-fg hover:bg-surface-2"
      title="Theme"
      aria-label="Theme"
      aria-haspopup="listbox"
      aria-expanded={open}
      onclick={() => (open = !open)}
    >
      <active.Icon size={11} />
      <span>{active.label}</span>
      <ChevronDown size={11} class="text-fg-muted" />
    </button>

    {#if open}
      <ul
        class="absolute right-0 top-full z-30 mt-1 w-52 overflow-hidden rounded-md border border-line bg-surface-1 shadow-lg"
        role="listbox"
        aria-label="Theme options"
      >
        {#each OPTIONS as opt (opt.id)}
          {@const selected = opt.id === theme}
          <li>
            <button
              type="button"
              role="option"
              aria-selected={selected}
              class="flex w-full items-center gap-2 px-3 py-2 text-left text-xs transition-colors
                {selected
                ? 'bg-sky-500/15 text-selected-fg'
                : 'text-fg hover:bg-surface-2'}"
              onclick={() => pick(opt.id)}
            >
              <opt.Icon size={13} class="shrink-0" />
              <div class="min-w-0 flex-1">
                <div class="truncate">{opt.label}</div>
                <div class="truncate text-[10px] text-fg-subtle">{opt.blurb}</div>
              </div>
              {#if selected}
                <Check size={12} class="shrink-0 text-selected-fg" />
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</header>
