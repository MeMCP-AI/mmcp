<script lang="ts">
  // Scope-specific icon (and tint class) that every scope-aware
  // layout reaches for. Wrapping lets callers pass just the
  // `scope` enum instead of re-mapping to icon imports each time.

  import { FolderGit2, Globe, Layers } from '@lucide/svelte';
  import type { GroupScope } from '$lib/types';

  interface Props {
    scope: GroupScope;
    size?: number;
    /** Extra class to merge (e.g. opacity / tint overrides). */
    extraClass?: string;
  }

  let { scope, size = 12, extraClass = '' }: Props = $props();

  const Icon = $derived(
    scope === 'project' ? FolderGit2 : scope === 'shared' ? Layers : Globe
  );
</script>

<Icon {size} class={extraClass} />
