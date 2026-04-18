<script lang="ts">
  import Modal from './Modal.svelte';
  import KindBadge from './KindBadge.svelte';
  import type { KindStr } from '$lib/types';
  import type { KindDisplay } from '$lib/stores/settings.svelte';

  interface Props {
    value: KindDisplay;
    onChange: (mode: KindDisplay) => void;
    onClose: () => void;
  }

  let { value, onChange, onClose }: Props = $props();

  const OPTIONS: { mode: KindDisplay; label: string }[] = [
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
</script>

<Modal title="Settings" onClose={onClose} widthClass="max-w-xl">
  <div class="flex flex-col gap-5 p-5">
    <section>
      <h3 class="text-xs font-semibold uppercase tracking-wide text-zinc-400">
        Memory list prefix
      </h3>
      <div class="mt-2 flex flex-col gap-1.5">
        {#each OPTIONS as opt (opt.mode)}
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
    </section>

    <section>
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
    </section>

    <footer class="text-[11px] text-zinc-500">
      Settings persist to the platform's app-config directory
      (<code>~/.config/mmcp-gui</code> on Linux,
      <code>%APPDATA%\mmcp-gui</code> on Windows). Delete that directory to reset.
    </footer>
  </div>
</Modal>
