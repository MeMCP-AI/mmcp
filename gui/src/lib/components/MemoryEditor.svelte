<script lang="ts">
  import { marked } from 'marked';
  import type { MemoryFile } from '$lib/types';

  interface Props {
    initial: MemoryFile | null;
    mode: 'new' | 'edit';
    onSave: (memory: MemoryFile, slug: string) => void;
    onCancel: () => void;
  }

  let { initial, mode, onSave, onCancel }: Props = $props();

  let slug = $state(initial?.frontmatter && 'slug' in initial.frontmatter ? '' : '');
  let name = $state(initial?.frontmatter.name ?? '');
  let description = $state(initial?.frontmatter.description ?? '');
  let kind = $state<string>(initial?.frontmatter.kind ?? 'scratch');
  let mandatory = $state(initial?.frontmatter.mandatory ?? false);
  let tags = $state((initial?.frontmatter.tags ?? []).join(', '));
  let body = $state(initial?.body ?? '');

  marked.setOptions({ breaks: false, gfm: true });
  const previewHtml = $derived(marked.parse(body) as string);

  const validation = $derived.by(() => {
    if (!slug.trim() && mode === 'new') return 'slug is required';
    if (!name.trim()) return 'name is required';
    if (!description.trim()) return 'description is required';
    return null;
  });

  function handleSave() {
    if (validation) return;
    const memory: MemoryFile = {
      frontmatter: {
        id: initial?.frontmatter.id ?? null,
        name: name.trim(),
        description: description.trim(),
        kind: kind as MemoryFile['frontmatter']['kind'],
        mandatory,
        version: initial?.frontmatter.version ?? null,
        tags: tags
          .split(',')
          .map((t) => t.trim())
          .filter((t) => t.length > 0)
      },
      body
    };
    onSave(memory, slug.trim());
  }
</script>

<section class="flex h-full flex-col bg-zinc-950">
  <div class="flex-1 overflow-y-auto">
    <div class="mx-auto flex max-w-5xl flex-col gap-5 p-6">
      <header>
        <h1 class="text-lg font-semibold text-zinc-50">
          {mode === 'new' ? 'New memory' : 'Edit memory'}
        </h1>
      </header>

      <!-- frontmatter form -->
      <div class="rounded-lg border border-zinc-800 bg-zinc-900 p-5">
        <div class="grid grid-cols-[120px_1fr] items-center gap-3 text-sm">
          <label for="slug" class="text-zinc-400">slug</label>
          <input
            id="slug"
            type="text"
            class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100 disabled:opacity-50"
            bind:value={slug}
            disabled={mode === 'edit'}
            placeholder="kebab-case, e.g. rule-commit-format"
          />

          <label for="name" class="text-zinc-400">name</label>
          <input
            id="name"
            type="text"
            class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
            bind:value={name}
          />

          <label for="description" class="text-zinc-400">description</label>
          <input
            id="description"
            type="text"
            class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
            bind:value={description}
          />

          <label for="kind" class="text-zinc-400">kind</label>
          <select
            id="kind"
            class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
            bind:value={kind}
          >
            <option value="rule">rule</option>
            <option value="snapshot">snapshot</option>
            <option value="log">log</option>
            <option value="reference">reference</option>
            <option value="scratch">scratch</option>
            <option value="feature">feature</option>
          </select>

          <label for="mandatory" class="text-zinc-400">mandatory</label>
          <input id="mandatory" type="checkbox" bind:checked={mandatory} class="justify-self-start" />

          <label for="tags" class="text-zinc-400">tags</label>
          <input
            id="tags"
            type="text"
            class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
            bind:value={tags}
            placeholder="comma-separated"
          />
        </div>
      </div>

      <!-- body split -->
      <div>
        <div class="mb-2 text-[11px] font-semibold uppercase tracking-wide text-zinc-500">
          Body (markdown)
        </div>
        <div class="grid min-h-[420px] grid-cols-2 gap-3">
          <textarea
            class="h-[420px] w-full resize-none rounded-md border border-zinc-800 bg-zinc-950 p-3 font-mono text-sm text-zinc-100 outline-none focus:border-zinc-600"
            bind:value={body}
            spellcheck="false"
          ></textarea>
          <div
            class="h-[420px] overflow-y-auto rounded-md border border-zinc-800 bg-zinc-900/40 p-3"
          >
            <div class="prose prose-invert prose-zinc prose-sm max-w-none">
              {@html previewHtml}
            </div>
          </div>
        </div>
      </div>

      <!-- actions -->
      <div class="flex items-center gap-3">
        <button
          type="button"
          class="inline-flex items-center rounded-md bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-50"
          disabled={validation !== null}
          onclick={handleSave}
        >
          {mode === 'new' ? 'Create' : 'Save'}
        </button>
        <button
          type="button"
          class="inline-flex items-center rounded-md border border-zinc-700 px-3 py-1.5 text-sm text-zinc-200 hover:bg-zinc-800"
          onclick={onCancel}
        >
          Cancel
        </button>
        {#if validation}
          <span class="text-xs text-rose-400">{validation}</span>
        {/if}
      </div>
    </div>
  </div>
</section>
