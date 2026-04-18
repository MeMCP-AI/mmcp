<script lang="ts">
  import type { GroupEntry } from '$lib/types';

  interface Props {
    groups: GroupEntry[];
    selectedId: string | null;
    onSelect: (groupId: string) => void;
  }

  let { groups, selectedId, onSelect }: Props = $props();
</script>

<aside
  class="flex h-full min-h-0 flex-col overflow-hidden border-r border-zinc-800 bg-zinc-900/50"
>
  <div
    class="flex h-9 shrink-0 items-center px-3 text-xs font-semibold uppercase tracking-wide text-zinc-400"
  >
    Groups
  </div>
  <div class="min-h-0 flex-1 overflow-y-auto">
    {#if groups.length === 0}
      <div class="px-3 py-2 text-xs text-zinc-500">
        No groups in the local mirror. Run <code class="text-zinc-300">mmcp init project</code>
        in a shell to create one.
      </div>
    {:else}
      <ul class="flex flex-col">
        {#each groups as group (group.group_id)}
          {@const selected = selectedId === group.group_id}
          {@const label = group.display_name ?? group.slug}
          {@const hint =
            group.display_name && group.display_name !== group.slug
              ? `${group.display_name} (${group.slug})`
              : label}
          <li>
            <button
              type="button"
              class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm transition-colors
                {selected
                ? 'bg-sky-500/15 text-sky-100'
                : 'text-zinc-200 hover:bg-zinc-800/70'}"
              onclick={() => onSelect(group.group_id)}
              title={hint}
            >
              <span class="truncate" title={label}>
                {label}
              </span>
              {#if group.display_name && group.display_name !== group.slug}
                <span class="truncate text-[11px] text-zinc-500" title={group.slug}>
                  {group.slug}
                </span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</aside>
