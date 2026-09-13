<script lang="ts">
  // Recursive slug folder-tree renderer over the memory_tree utility's already-nested `nodes`.
  // A folder node is a collapsible row: a Folder / FolderOpen icon, plus its master
  // memory's kind icon when `buildMemoryTree` attached one.
  // A leaf node falls back to the existing MemoryRow.
  // The chevron toggles expansion; the rest of the folder row selects its master memory when
  // one is present, and otherwise also toggles expansion.
  // Manual expand/collapse is local per-folder state keyed by folder path.
  // It survives `nodes` changing reference, as long as this component instance stays mounted.
  // Folders start collapsed.
  //
  // `forceExpand` overrides manual state and expands every folder regardless.
  // The caller sets it while a filter narrows `nodes` down to matches only.
  // Every folder that survives that pruning holds at least one match.
  // None of them should stay collapsed and hide it.

  import { ChevronRight, Folder, FolderOpen } from '@lucide/svelte';
  import KindBadge from '../KindBadge.svelte';
  import MemoryRow from './MemoryRow.svelte';
  import MemoryTree from './MemoryTree.svelte';
  import type { MemoryTreeNode } from '$lib/utils/memory_tree';
  import type { MemoryFile } from '$lib/types';

  export interface MemoryTreeEntry {
    slug: string;
    /** Frontmatter-only descriptor, mirrors `MemoryRow`'s own `descriptor` prop. */
    body: MemoryFile | undefined;
  }

  interface Props {
    nodes: MemoryTreeNode<MemoryTreeEntry>[];
    onSelect: (slug: string) => void;
    forceExpand?: boolean;
  }

  let { nodes, onSelect, forceExpand = false }: Props = $props();

  // Absent path means collapsed, the default.
  let expandedByPath = $state<Record<string, boolean>>({});

  function isExpanded(path: string): boolean {
    return forceExpand || (expandedByPath[path] ?? false);
  }

  function toggle(path: string) {
    // A click during forceExpand must not write manual state.
    // isExpanded would read true from forceExpand regardless, hiding a false write until the filter clears.
    // That stale write then collapses a folder the user never manually closed.
    if (forceExpand) return;
    expandedByPath[path] = !isExpanded(path);
  }
</script>

{#each nodes as node (node.type === 'leaf' ? `leaf:${node.slug}` : `folder:${node.path}`)}
  {#if node.type === 'leaf'}
    <MemoryRow
      slug={node.entry.slug}
      descriptor={node.entry.body}
      onSelect={() => onSelect(node.entry.slug)}
    />
  {:else}
    <div>
      <div
        class="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-xs text-fg-muted transition-colors hover:bg-surface-2 hover:text-fg"
      >
        <button
          type="button"
          class="shrink-0"
          onclick={() => toggle(node.path)}
          aria-expanded={isExpanded(node.path)}
          aria-label={isExpanded(node.path) ? 'Collapse folder' : 'Expand folder'}
        >
          <ChevronRight size={12} class={isExpanded(node.path) ? 'rotate-90' : ''} />
        </button>
        <button
          type="button"
          class="flex min-w-0 flex-1 items-center gap-1.5 text-left"
          onclick={() => (node.master ? onSelect(node.master.slug) : toggle(node.path))}
        >
          {#if isExpanded(node.path)}
            <FolderOpen size={13} />
          {:else}
            <Folder size={13} />
          {/if}
          {#if node.master?.body}
            <KindBadge kind={node.master.body.frontmatter.kind} mode="icon" />
          {/if}
          <span class="truncate">{node.name}</span>
        </button>
      </div>
      {#if isExpanded(node.path)}
        <!-- `pl-4` is the whole indentation mechanism. -->
        <!-- Each recursion nests one more of these, so depth accumulates structurally. -->
        <div class="mt-1 flex flex-col gap-1.5 pl-4">
          <MemoryTree nodes={node.children} {onSelect} {forceExpand} />
        </div>
      {/if}
    </div>
  {/if}
{/each}
