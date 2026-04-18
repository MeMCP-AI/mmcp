<script lang="ts">
  import Modal from './Modal.svelte';
  import KindBadge from './KindBadge.svelte';
  import { Database, Info, Palette } from 'lucide-svelte';
  import type { KindStr } from '$lib/types';
  import type { KindDisplay } from '$lib/stores/settings.svelte';

  interface Props {
    value: KindDisplay;
    onChange: (mode: KindDisplay) => void;
    onReset: () => void;
    onClose: () => void;
  }

  let { value, onChange, onReset, onClose }: Props = $props();

  type TabId = 'appearance' | 'storage' | 'about';
  let tab = $state<TabId>('appearance');

  const TABS: { id: TabId; label: string; Icon: typeof Palette }[] = [
    { id: 'appearance', label: 'Appearance', Icon: Palette },
    { id: 'storage', label: 'Storage', Icon: Database },
    { id: 'about', label: 'About', Icon: Info }
  ];

  const KIND_OPTIONS: { mode: KindDisplay; label: string }[] = [
    { mode: 'off', label: 'Off' },
    { mode: 'icon', label: 'Icon only' },
    { mode: 'text', label: 'Text only' },
    { mode: 'icon_and_text', label: 'Icon + text' }
  ];

  const SAMPLES: { kind: KindStr; slug: string }[] = [
    { kind: 'rule', slug: 'branch-policy' },
    { kind: 'snapshot', slug: 'repo-state-2026-04-18' },
    { kind: 'log', slug: 'incident-2026-03-05' },
    { kind: 'reference', slug: 'gitoxide-upstream' },
    { kind: 'scratch', slug: 'draft-notes' },
    { kind: 'feature', slug: 'fr-020-extract-mmcp-store' }
  ];

  const settingsPathHint =
    typeof navigator !== 'undefined' && navigator.userAgent.includes('Windows')
      ? '%APPDATA%\\mmcp-gui\\settings.json'
      : '~/.config/mmcp-gui/settings.json';

  let resetConfirm = $state(false);
</script>

<Modal title="Settings" onClose={onClose} widthClass="max-w-3xl">
  <div class="flex min-h-[420px] flex-col sm:flex-row">
    <!-- Vertical tab rail -->
    <nav
      class="flex shrink-0 gap-1 overflow-x-auto border-b border-zinc-800 bg-zinc-950/40 p-2 sm:w-44 sm:flex-col sm:gap-0 sm:overflow-x-visible sm:border-b-0 sm:border-r"
    >
      {#each TABS as t (t.id)}
        {@const active = tab === t.id}
        <button
          type="button"
          class="flex shrink-0 items-center gap-2 rounded-md px-3 py-1.5 text-sm transition-colors
            {active
            ? 'bg-sky-500/15 text-sky-100'
            : 'text-zinc-400 hover:bg-zinc-800/70 hover:text-zinc-200'}"
          onclick={() => (tab = t.id)}
        >
          <t.Icon size={14} />
          <span>{t.label}</span>
        </button>
      {/each}
    </nav>

    <!-- Tab content -->
    <div class="min-h-0 flex-1 overflow-y-auto p-5">
      {#if tab === 'appearance'}
        <section class="flex flex-col gap-5">
          <div>
            <h3 class="text-sm font-semibold text-zinc-100">Memory list prefix</h3>
            <p class="mt-0.5 text-xs text-zinc-500">
              How the kind badge appears next to each slug in the memory list.
            </p>
            <div class="mt-3 flex flex-col gap-1.5">
              {#each KIND_OPTIONS as opt (opt.mode)}
                <label class="flex cursor-pointer items-center gap-2 text-sm text-zinc-200">
                  <input
                    type="radio"
                    name="kind-display"
                    value={opt.mode}
                    checked={value === opt.mode}
                    onchange={() => onChange(opt.mode)}
                  />
                  {opt.label}
                </label>
              {/each}
            </div>
          </div>

          <div>
            <h3 class="text-xs font-semibold uppercase tracking-wide text-zinc-400">Preview</h3>
            <div class="mt-2 rounded-lg border border-zinc-800 bg-zinc-950 p-3">
              <ul class="flex flex-col gap-1 font-mono text-sm">
                {#each SAMPLES as sample (sample.slug)}
                  <li class="flex items-center gap-2 text-zinc-200">
                    {#if value !== 'off'}
                      <KindBadge kind={sample.kind} mode={value} />
                    {/if}
                    <span>{sample.slug}</span>
                  </li>
                {/each}
              </ul>
            </div>
          </div>
        </section>
      {:else if tab === 'storage'}
        <section class="flex flex-col gap-5">
          <div>
            <h3 class="text-sm font-semibold text-zinc-100">Settings file</h3>
            <p class="mt-0.5 text-xs text-zinc-500">
              Persists across restarts. Written by the Tauri backend on every change.
            </p>
            <div class="mt-3 rounded-md border border-zinc-800 bg-zinc-950 px-3 py-2 font-mono text-xs text-zinc-300">
              {settingsPathHint}
            </div>
          </div>

          <div>
            <h3 class="text-sm font-semibold text-zinc-100">Reset to defaults</h3>
            <p class="mt-0.5 text-xs text-zinc-500">
              Clears every preference on this device. Memories and groups are untouched.
            </p>
            {#if !resetConfirm}
              <button
                type="button"
                class="mt-3 inline-flex items-center rounded-md border border-zinc-700 px-3 py-1.5 text-sm text-zinc-200 hover:bg-zinc-800"
                onclick={() => (resetConfirm = true)}
              >
                Reset settings…
              </button>
            {:else}
              <div class="mt-3 flex flex-wrap items-center gap-2">
                <span class="text-xs text-zinc-400">Confirm reset?</span>
                <button
                  type="button"
                  class="inline-flex items-center rounded-md bg-rose-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-rose-500"
                  onclick={() => {
                    onReset();
                    resetConfirm = false;
                  }}
                >
                  Reset
                </button>
                <button
                  type="button"
                  class="inline-flex items-center rounded-md border border-zinc-700 px-3 py-1.5 text-sm text-zinc-200 hover:bg-zinc-800"
                  onclick={() => (resetConfirm = false)}
                >
                  Cancel
                </button>
              </div>
            {/if}
          </div>
        </section>
      {:else if tab === 'about'}
        <section class="flex flex-col gap-5">
          <div>
            <h3 class="text-sm font-semibold text-zinc-100">mmcp-gui</h3>
            <p class="mt-0.5 text-xs text-zinc-500">
              Desktop visual client for mmcp memories. Reads, writes, and syncs through
              <code class="rounded bg-zinc-800 px-1 py-0.5 text-zinc-300">mmcp-store</code> directly —
              no HTTP, no MCP round-trip.
            </p>
          </div>

          <dl class="grid grid-cols-[120px_1fr] gap-y-2 text-sm">
            <dt class="text-zinc-500">Version</dt>
            <dd class="text-zinc-200">0.1.0</dd>
            <dt class="text-zinc-500">Shell</dt>
            <dd class="text-zinc-200">Tauri 2</dd>
            <dt class="text-zinc-500">Frontend</dt>
            <dd class="text-zinc-200">SvelteKit 2 · Svelte 5 runes · Tailwind v4</dd>
            <dt class="text-zinc-500">Icons</dt>
            <dd class="text-zinc-200">Lucide</dd>
            <dt class="text-zinc-500">Package manager</dt>
            <dd class="text-zinc-200">bun</dd>
          </dl>

          <p class="text-xs text-zinc-500">
            See <code class="rounded bg-zinc-800 px-1 py-0.5 text-zinc-300">gui/README.md</code>
            for setup and build instructions.
          </p>
        </section>
      {/if}
    </div>
  </div>
</Modal>
