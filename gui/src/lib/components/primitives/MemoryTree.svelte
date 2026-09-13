<script lang="ts">
  // Recursive slug folder-tree renderer over the memory_tree utility's already-nested `nodes`.
  // A folder row shows a Folder or FolderOpen icon.
  // A folder carrying a master memory also shows that memory's kind icon on its label.
  // A leaf row renders through MemoryRow.
  // The chevron button only toggles expansion and carries `aria-expanded`.
  // The label button opens the master memory when one exists, or toggles expansion otherwise.
  // Manual expand and collapse state is local per folder, keyed by folder path.
  // It survives `nodes` changing reference as long as this component instance stays mounted.
  // Folders start collapsed.
  // `forceExpand` overrides manual state and expands every folder.
  // A filtered result set passes it so every surviving match stays visible.
  // A short, always-open list passes it too.
  // `preventBlurOnMouseDown` makes every row's mousedown call `preventDefault`.
  // A caller passes it when its own blur handler would otherwise close this tree before a click lands.

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
    /** Guards every row's mousedown against a caller's own blur-triggered close. */
    preventBlurOnMouseDown?: boolean;
  }

  let {
    nodes,
    onSelect,
    forceExpand = false,
    preventBlurOnMouseDown = false
  }: Props = $props();

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

  function guardMouseDown(e: MouseEvent) {
    if (preventBlurOnMouseDown) e.preventDefault();
  }
</script>

{#each nodes as node (node.type === 'leaf' ? `leaf:${node.slug}` : `folder:${node.path}`)}
  {#if node.type === 'leaf'}
    <MemoryRow
      slug={node.entry.slug}
      descriptor={node.entry.body}
      onSelect={() => onSelect(node.entry.slug)}
      onMouseDown={guardMouseDown}
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
          onmousedown={guardMouseDown}
          aria-expanded={isExpanded(node.path)}
          aria-label={isExpanded(node.path) ? 'Collapse folder' : 'Expand folder'}
        >
          <ChevronRight size={12} class={isExpanded(node.path) ? 'rotate-90' : ''} />
        </button>
        <button
          type="button"
          class="flex min-w-0 flex-1 items-center gap-1.5 text-left"
          onclick={() => (node.master ? onSelect(node.master.slug) : toggle(node.path))}
          onmousedown={guardMouseDown}
          aria-expanded={node.master ? undefined : isExpanded(node.path)}
          aria-label={node.master
            ? `${node.name}: open ${node.master.body?.frontmatter.name ?? node.master.slug}`
            : undefined}
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
          <MemoryTree nodes={node.children} {onSelect} {forceExpand} {preventBlurOnMouseDown} />
        </div>
      {/if}
    </div>
  {/if}
{/each}
