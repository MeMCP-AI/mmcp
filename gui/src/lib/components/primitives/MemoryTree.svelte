<script lang="ts">
  // Recursive slug folder-tree renderer over the memoryTree utility's already-nested `nodes`.
  // A folder node is a collapsible row (Folder / FolderOpen icon, distinct from MemoryRow's per-kind icons).
  // A leaf node falls back to the existing MemoryRow.
  // Manual expand/collapse is local per-folder state keyed by folder path.
  // It survives `nodes` changing reference, as long as this component instance stays mounted.
  // Folders start collapsed.
  //
  // `forceExpand` overrides manual state and expands every folder regardless.
  // The caller sets it while a filter narrows `nodes` down to matches only.
  // Every folder that survives that pruning holds at least one match.
  // None of them should stay collapsed and hide it.

  import { ChevronRight, Folder, FolderOpen } from '@lucide/svelte';
  import MemoryRow from './MemoryRow.svelte';
  import MemoryTree from './MemoryTree.svelte';
  import type { MemoryTreeNode } from '$lib/memoryTree';
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
    /**
     * Recursion depth, drives indentation.
     * Callers never set this; the self-import recursion below passes it down.
     */
    depth?: number;
  }

  let { nodes, onSelect, forceExpand = false, depth = 0 }: Props = $props();

  // Indentation per nesting level.
  // Narrow enough that a deeply nested slug still reads, wide enough to visually separate sibling depths.
  const INDENT_STEP_REM = 0.9;

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
    <div style={`padding-left: ${depth * INDENT_STEP_REM}rem`}>
      <MemoryRow
        slug={node.entry.slug}
        descriptor={node.entry.body}
        onSelect={() => onSelect(node.entry.slug)}
      />
    </div>
  {:else}
    <div>
      <button
        type="button"
        class="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-xs text-fg-muted transition-colors hover:bg-surface-2 hover:text-fg"
        style={`padding-left: ${depth * INDENT_STEP_REM}rem`}
        onclick={() => toggle(node.path)}
        aria-expanded={isExpanded(node.path)}
      >
        <ChevronRight size={12} class={isExpanded(node.path) ? 'rotate-90' : ''} />
        {#if isExpanded(node.path)}
          <FolderOpen size={13} />
        {:else}
          <Folder size={13} />
        {/if}
        <span class="truncate">{node.name}</span>
      </button>
      {#if isExpanded(node.path)}
        <div class="mt-1 flex flex-col gap-1.5">
          <MemoryTree nodes={node.children} {onSelect} {forceExpand} depth={depth + 1} />
        </div>
      {/if}
    </div>
  {/if}
{/each}
