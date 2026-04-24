<script lang="ts">
  // Theme picker popover. A single trigger button shows the
  // active mode; clicking opens a list of every theme so the
  // user jumps directly to the one they want instead of cycling.
  // Lifted into its own primitive so any "main bar" can drop it
  // in without re-implementing the popover semantics.

  import { Check, CircleDot, Contrast, Monitor, Moon, Sun } from 'lucide-svelte';
  import { settingsStore, type ThemeMode } from '$lib/stores/settings.svelte';

  interface ThemeOption {
    id: ThemeMode;
    label: string;
    Icon: typeof Sun;
    blurb: string;
  }

  const OPTIONS: ThemeOption[] = [
    { id: 'dark', label: 'Dark', Icon: Moon, blurb: 'Default night palette.' },
    { id: 'oled', label: 'OLED', Icon: CircleDot, blurb: 'Pure black, saves OLED pixels.' },
    { id: 'dim-dark', label: 'Dim Dark', Icon: Contrast, blurb: 'Soft dark, lower contrast.' },
    { id: 'dim', label: 'Dim Light', Icon: Contrast, blurb: 'Low-contrast soft daylight.' },
    { id: 'light', label: 'Light', Icon: Sun, blurb: 'Bright palette for daytime.' },
    { id: 'system', label: 'System', Icon: Monitor, blurb: 'Follow the OS preference.' }
  ];

  const theme = $derived(settingsStore.values.theme);
  const active = $derived(OPTIONS.find((o) => o.id === theme) ?? OPTIONS[0]);

  let open = $state(false);
  let anchorEl: HTMLElement | undefined = $state();

  // Outside-click closer, registered only while open so we don't
  // pay the listener cost otherwise. The setTimeout defers
  // registration past the click that opens the popover.
  $effect(() => {
    if (!open) return;
    const handler = (ev: MouseEvent) => {
      if (!anchorEl) return;
      if (!anchorEl.contains(ev.target as Node)) open = false;
    };
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

<div class="relative" bind:this={anchorEl}>
  <button
    type="button"
    class="inline-flex items-center rounded-sm p-1 text-fg-muted hover:text-fg focus:outline-none focus-visible:text-fg"
    title={`Theme: ${active.label}`}
    aria-label={`Theme: ${active.label}`}
    aria-haspopup="listbox"
    aria-expanded={open}
    onclick={() => (open = !open)}
  >
    <active.Icon size={14} />
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
              {selected ? 'bg-sky-500/15 text-selected-fg' : 'text-fg hover:bg-surface-2'}"
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
