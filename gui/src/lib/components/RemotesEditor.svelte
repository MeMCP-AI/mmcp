<script lang="ts">
  // Remotes-list editor shared by the Settings panel's User and
  // Project tabs: add, remove, reorder, and edit per-kind fields for
  // a `SyncConfig.remotes` array. Order is semantic (it feeds
  // resolution order), so reordering uses explicit up/down controls
  // rather than drag-and-drop.

  import { ArrowDown, ArrowUp, Plus, Trash2 } from '@lucide/svelte';
  import type { RemoteAuth } from '$lib/types';
  import { newDraft, type DraftRemote } from '$lib/utils/remotes_draft';

  interface Props {
    remotes: DraftRemote[];
    onChange: (remotes: DraftRemote[]) => void;
    disabled?: boolean;
  }

  let { remotes, onChange, disabled = false }: Props = $props();

  const AUTH_OPTIONS: { value: RemoteAuth; label: string }[] = [
    { value: 'none', label: 'None (ambient git/SSH)' },
    { value: 'ssh-agent', label: 'SSH agent' },
    { value: 'bearer', label: 'Bearer token (env-derived)' }
  ];

  function update(key: string, patch: Partial<DraftRemote>) {
    onChange(remotes.map((r) => (r._key === key ? { ...r, ...patch } : r)));
  }

  // Checking one remote's "default push target" clears it on every
  // other remote in this same list: a single file may declare at
  // most one default (the backend's `SyncConfig::validate` rejects
  // more than one), so the editor never lets the user create that
  // state to begin with.
  //
  // Unchecking the sole default is still allowed: the merged
  // effective set across levels may resolve a default from
  // elsewhere, or from being the only remote left.
  function setDefault(key: string, checked: boolean) {
    onChange(
      remotes.map((r) => ({
        ...r,
        default: r._key === key ? checked : checked ? false : r.default
      }))
    );
  }

  function remove(key: string) {
    onChange(remotes.filter((r) => r._key !== key));
  }

  function move(key: string, direction: -1 | 1) {
    const index = remotes.findIndex((r) => r._key === key);
    if (index === -1) return;
    const target = index + direction;
    if (target < 0 || target >= remotes.length) return;
    const next = remotes.slice();
    [next[index], next[target]] = [next[target], next[index]];
    onChange(next);
  }

  function add(kind: DraftRemote['kind']) {
    onChange([...remotes, newDraft(kind)]);
  }
</script>

<div class="flex flex-col gap-3">
  {#each remotes as remote, index (remote._key)}
    <div class="rounded-md border border-line-strong bg-surface-0 p-3">
      <div class="flex items-center gap-2">
        <select
          class="rounded-md border border-line-strong bg-surface-1 px-2 py-1 text-xs text-fg"
          value={remote.kind}
          {disabled}
          onchange={(e) =>
            update(remote._key, { kind: e.currentTarget.value as DraftRemote['kind'] })}
        >
          <option value="mmcp-server">mmcp-server</option>
          <option value="direct-git">direct-git</option>
        </select>
        <input
          type="text"
          class="flex-1 rounded-md border border-line-strong bg-surface-1 px-2 py-1 text-xs text-fg"
          placeholder="name"
          value={remote.name}
          {disabled}
          oninput={(e) => update(remote._key, { name: e.currentTarget.value })}
        />
        <button
          type="button"
          class="rounded-md p-1 text-fg-muted hover:bg-surface-2 hover:text-fg disabled:cursor-not-allowed disabled:opacity-40"
          title="Move up"
          disabled={disabled || index === 0}
          onclick={() => move(remote._key, -1)}
        >
          <ArrowUp size={12} />
        </button>
        <button
          type="button"
          class="rounded-md p-1 text-fg-muted hover:bg-surface-2 hover:text-fg disabled:cursor-not-allowed disabled:opacity-40"
          title="Move down"
          disabled={disabled || index === remotes.length - 1}
          onclick={() => move(remote._key, 1)}
        >
          <ArrowDown size={12} />
        </button>
        <button
          type="button"
          class="rounded-md p-1 text-rose-400 hover:bg-rose-500/10 disabled:cursor-not-allowed disabled:opacity-40"
          title="Remove"
          {disabled}
          onclick={() => remove(remote._key)}
        >
          <Trash2 size={12} />
        </button>
      </div>

      <div class="mt-2 grid grid-cols-[80px_1fr] items-center gap-2 text-xs">
        <span class="text-fg-muted">url</span>
        <input
          type="text"
          class="rounded-md border border-line-strong bg-surface-1 px-2 py-1 text-fg"
          placeholder={remote.kind === 'mmcp-server'
            ? 'https://mmcp.example.com'
            : 'ssh://git@example.com/repo.git'}
          value={remote.url}
          {disabled}
          oninput={(e) => update(remote._key, { url: e.currentTarget.value })}
        />

        {#if remote.kind === 'direct-git'}
          <span class="text-fg-muted">auth</span>
          <select
            class="rounded-md border border-line-strong bg-surface-1 px-2 py-1 text-fg"
            value={remote.auth}
            {disabled}
            onchange={(e) => update(remote._key, { auth: e.currentTarget.value as RemoteAuth })}
          >
            {#each AUTH_OPTIONS as opt (opt.value)}
              <option value={opt.value}>{opt.label}</option>
            {/each}
          </select>

          <span class="text-fg-muted">group</span>
          <input
            type="text"
            class="rounded-md border border-line-strong bg-surface-1 px-2 py-1 text-fg"
            placeholder="(defaults to this project)"
            value={remote.group}
            {disabled}
            oninput={(e) => update(remote._key, { group: e.currentTarget.value })}
          />
        {/if}
      </div>

      <div class="mt-2 flex flex-wrap gap-4 text-xs">
        <label class="flex items-center gap-1.5 text-fg">
          <input
            type="checkbox"
            checked={remote.default}
            {disabled}
            onchange={(e) => setDefault(remote._key, e.currentTarget.checked)}
          />
          default push target
        </label>
        <label class="flex items-center gap-1.5 text-fg">
          <input
            type="checkbox"
            checked={remote.include_in_push_all}
            {disabled}
            onchange={(e) =>
              update(remote._key, { include_in_push_all: e.currentTarget.checked })}
          />
          included in --all-remotes
        </label>
      </div>
    </div>
  {:else}
    <p class="text-xs text-fg-subtle">No remotes configured yet.</p>
  {/each}

  <div class="flex gap-2">
    <button
      type="button"
      class="inline-flex items-center gap-1.5 rounded-md border border-line-strong px-2 py-1 text-xs text-fg hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-50"
      {disabled}
      onclick={() => add('mmcp-server')}
    >
      <Plus size={12} /> Add mmcp-server remote
    </button>
    <button
      type="button"
      class="inline-flex items-center gap-1.5 rounded-md border border-line-strong px-2 py-1 text-xs text-fg hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-50"
      {disabled}
      onclick={() => add('direct-git')}
    >
      <Plus size={12} /> Add direct-git remote
    </button>
  </div>
</div>
