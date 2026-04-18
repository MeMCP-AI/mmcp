<script lang="ts">
  import '../app.css';

  let { children } = $props();
</script>

<div class="flex h-full w-full flex-col">
  {@render children()}
</div>
