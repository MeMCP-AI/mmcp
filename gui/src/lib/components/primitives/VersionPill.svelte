<script lang="ts">
  // `v1.2.3`-style frontmatter version pill. Lives in every memory
  // header; keeping it a primitive so a tweak to the shape or tint
  // only has to happen once.

  interface Props {
    version: string;
  }

  let { version }: Props = $props();
</script>

<span
  class="inline-flex items-center rounded-md bg-surface-2 px-1.5 py-0.5 text-[10px] font-semibold text-fg-muted"
  title="Schema version"
>
  v{version}
</span>
