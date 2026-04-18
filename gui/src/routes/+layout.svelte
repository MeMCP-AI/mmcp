<script lang="ts">
  import '../app.css';

  let { children } = $props();
</script>

<main class="h-full w-full">
  {@render children()}
</main>
