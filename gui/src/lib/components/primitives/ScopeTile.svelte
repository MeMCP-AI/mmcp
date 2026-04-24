<script lang="ts">
  // Home-dashboard scope tile. A big button that advertises how
  // many groups live in a scope and opens the scope view.

  import ScopeIcon from './ScopeIcon.svelte';
  import { SCOPE_META } from '$lib/utils/scope';
  import type { GroupScope } from '$lib/types';

  interface Props {
    scope: GroupScope;
    groupCount: number;
    onOpen: () => void;
  }

  let { scope, groupCount, onOpen }: Props = $props();

  const meta = $derived(SCOPE_META[scope]);
</script>

<button
  type="button"
  class="rounded-lg border border-line p-4 text-left transition-colors hover:border-line-strong hover:bg-surface-1 {meta.tileTint} ring-1 ring-inset"
  onclick={onOpen}
>
  <div class="flex items-center justify-between">
    <div class="inline-flex items-center gap-2 text-sm font-semibold text-fg">
      <ScopeIcon {scope} size={14} />
      {meta.label}
    </div>
    <span class="text-[11px] text-fg-muted">
      {groupCount} group{groupCount === 1 ? '' : 's'}
    </span>
  </div>
  <p class="mt-1 text-xs text-fg-muted">{meta.description}</p>
</button>
