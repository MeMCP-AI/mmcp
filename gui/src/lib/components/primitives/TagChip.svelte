<script lang="ts">
  // Frontmatter `tags` chip. Renders either the bare tag or a
  // `#prefixed` version for layouts that want a hashtag vibe
  // (feed cards, viewer chips).

  interface Props {
    tag: string;
    hash?: boolean;
  }

  let { tag, hash = false }: Props = $props();
</script>

<span
  class="inline-flex items-center rounded-md bg-surface-2 px-1.5 py-0.5 text-[10px] text-fg-muted"
  title={tag}
>
  {hash ? `#${tag}` : tag}
</span>
