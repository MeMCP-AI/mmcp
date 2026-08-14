<script lang="ts">
  // Feed-variant card. Two-row layout — header (name + description
  // + scope/group metadata) on top, chip strip (kind / FR / tags
  // / mandatory / version) on the bottom. Kind-coloured left border
  // drives the colour-at-a-glance effect the feed relies on.

  import FeatureBadge from '../FeatureBadge.svelte';
  import KindBadge from '../KindBadge.svelte';
  import MandatoryPill from './MandatoryPill.svelte';
  import ScopeIcon from './ScopeIcon.svelte';
  import TagChip from './TagChip.svelte';
  import VersionPill from './VersionPill.svelte';
  import { SCOPE_META } from '$lib/utils/scope';
  import type { KindStr } from '$lib/utils/memory_kind';
  import type { GroupEntry, MemoryFile } from '$lib/types';

  interface Props {
    slug: string;
    body: MemoryFile;
    group: GroupEntry;
    onOpen: () => void;
  }

  let { slug, body, group, onOpen }: Props = $props();

  const fm = $derived(body.frontmatter);

  const KIND_ACCENT: Record<KindStr, string> = {
    rule: 'border-l-kind-rule',
    snapshot: 'border-l-kind-snapshot',
    log: 'border-l-kind-log',
    reference: 'border-l-kind-reference',
    scratch: 'border-l-kind-scratch',
    feature: 'border-l-kind-feature',
    issue: 'border-l-kind-issue',
    milestone: 'border-l-kind-milestone'
  };
</script>

<button
  type="button"
  class="group flex w-full flex-col gap-2 rounded-lg border border-l-4 border-line bg-surface-1 p-4 text-left transition-colors hover:border-line-strong hover:bg-surface-2 {KIND_ACCENT[fm.kind]}"
  onclick={onOpen}
>
  <div class="flex flex-wrap items-start gap-2">
    <div class="min-w-0 flex-1">
      <h2 class="truncate text-sm font-semibold text-fg" title={fm.name}>
        {fm.name}
      </h2>
      <p class="mt-0.5 line-clamp-2 text-xs text-fg-muted">{fm.description}</p>
    </div>
    <div class="flex shrink-0 flex-col items-end gap-1 text-[10px] text-fg-subtle">
      <span class="inline-flex items-center gap-1">
        <ScopeIcon scope={group.scope} size={10} />
        {SCOPE_META[group.scope].label}
      </span>
      <span class="font-mono" title={group.slug}>
        {group.display_name ?? group.slug}
      </span>
    </div>
  </div>
  <div class="flex flex-wrap items-center gap-1.5">
    <KindBadge kind={fm.kind} mode="icon_and_text" />
    {#if fm.feature}
      <FeatureBadge status={fm.feature.status} number={fm.feature.number} />
    {/if}
    <code class="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[10px] text-fg-muted">
      {slug}
    </code>
    {#if fm.mandatory}
      <MandatoryPill />
    {/if}
    {#if fm.version}
      <VersionPill version={fm.version} />
    {/if}
    {#each fm.tags as tag (tag)}
      <TagChip {tag} hash />
    {/each}
  </div>
</button>
