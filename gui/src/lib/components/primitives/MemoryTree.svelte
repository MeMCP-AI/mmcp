<script lang="ts">
  // Recursive slug folder-tree renderer, built on top of the
  // memoryTree utility's already-nested `nodes`. A folder node is a
  // collapsible row (Folder / FolderOpen icon, distinct from
  // MemoryRow's per-kind icons); a leaf node falls back to the
  // existing MemoryRow. Manual expand/collapse is local per-folder
  // state keyed by folder path, so it survives `nodes` changing
  // reference (e.g. the caller re-deriving the tree from a filtered
  // entry list) as long as this component instance stays mounted;
  // folders start collapsed.
  //
  // `forceExpand` overrides manual state and expands every folder
  // regardless: the caller sets it while a search/filter narrows
  // `nodes` down to matches only, since every folder that survives
  // that pruning holds at least one match and must not stay
  // collapsed and hide it.

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
    /** Recursion depth, drives indentation. Callers never set this; the self-import recursion below passes it down. */
    depth?: number;
  }

  let { nodes, onSelect, forceExpand = false, depth = 0 }: Props = $props();

  // Indentation per nesting level. Narrow enough that a deeply
  // nested slug still reads, wide enough to visually separate
  // sibling depths.
  const INDENT_STEP_REM = 0.9;

  // Absent path means collapsed, the default.
  let expandedByPath = $state<Record<string, boolean>>({});

  function isExpanded(path: string): boolean {
    return forceExpand || (expandedByPath[path] ?? false);
  }

  function toggle(path: string) {
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
