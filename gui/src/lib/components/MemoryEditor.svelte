<script lang="ts">
  import KindBadge from './KindBadge.svelte';
  import { untrack } from 'svelte';
  import { marked } from 'marked';
  import type { KindStr, MemoryFile } from '$lib/types';

  interface Props {
    initial: MemoryFile | null;
    mode: 'new' | 'edit';
    onSave: (memory: MemoryFile, slug: string) => void;
    onCancel: () => void;
  }

  let { initial, mode, onSave, onCancel }: Props = $props();

  // One-shot capture from the `initial` prop — the parent remounts
  // the editor on every `{#if editor}` toggle so later prop changes
  // aren't expected. `untrack` silences the
  // `state_referenced_locally` warning.
  let slug = $state(untrack(() => ''));
  let name = $state(untrack(() => initial?.frontmatter.name ?? ''));
  let description = $state(untrack(() => initial?.frontmatter.description ?? ''));
  let kind = $state<string>(untrack(() => initial?.frontmatter.kind ?? 'scratch'));
  let mandatory = $state(untrack(() => initial?.frontmatter.mandatory ?? false));
  let tags = $state(untrack(() => (initial?.frontmatter.tags ?? []).join(', ')));
  let body = $state(untrack(() => initial?.body ?? ''));

  // Body-only Edit/Preview toggle. Frontmatter form stays visible
  // above it so the user can tweak metadata while previewing the
  // rendered markdown without flipping a page-level tab.
  let bodyTab = $state<'edit' | 'preview'>('edit');

  marked.setOptions({ breaks: false, gfm: true });
  const previewHtml = $derived(marked.parse(body) as string);

  const tagsList = $derived(
    tags
      .split(',')
      .map((t) => t.trim())
      .filter((t) => t.length > 0)
  );

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
        tags: tagsList
      },
      body
    };
    onSave(memory, slug.trim());
  }

  const tabBtn = (active: boolean) =>
    'rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors ' +
    (active
      ? 'bg-sky-500/15 text-sky-100'
      : 'text-zinc-400 hover:bg-zinc-800/70 hover:text-zinc-200');
</script>

<section class="flex h-full min-h-0 flex-col overflow-hidden bg-zinc-950">
  <!-- Header: title only. Never scrolls. -->
  <header
    class="flex shrink-0 items-center gap-3 border-b border-zinc-800 bg-zinc-900/40 px-4 py-2 sm:px-6"
  >
    <h1 class="text-sm font-semibold text-zinc-100">
      {mode === 'new' ? 'New memory' : 'Edit memory'}
    </h1>
  </header>

  <!-- Scrollable body. Frontmatter form always visible; body section
       has its own Edit/Preview toggle so metadata stays in view. -->
  <div class="min-h-0 flex-1 overflow-y-auto">
    <div class="mx-auto max-w-4xl px-4 py-5 sm:px-6 sm:py-6">
      <!-- frontmatter form -->
      <div class="rounded-lg border border-zinc-800 bg-zinc-900 p-4 sm:p-5">
        <div
          class="grid grid-cols-[100px_1fr] items-center gap-3 text-sm sm:grid-cols-[120px_1fr]"
        >
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
          <input
            id="mandatory"
            type="checkbox"
            bind:checked={mandatory}
            class="justify-self-start"
          />

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

      <!-- Body editor with inline Edit/Preview tabs. -->
      <div class="mt-5 overflow-hidden rounded-md border border-zinc-800 bg-zinc-950">
        <div
          class="flex items-center justify-between border-b border-zinc-800 bg-zinc-900/40 px-3 py-1.5"
        >
          <div class="text-[11px] font-semibold uppercase tracking-wide text-zinc-500">
            Body (markdown)
          </div>
          <div class="flex items-center gap-1">
            <button
              type="button"
              class={tabBtn(bodyTab === 'edit')}
              onclick={() => (bodyTab = 'edit')}
            >
              Edit
            </button>
            <button
              type="button"
              class={tabBtn(bodyTab === 'preview')}
              onclick={() => (bodyTab = 'preview')}
            >
              Preview
            </button>
          </div>
        </div>

        {#if bodyTab === 'edit'}
          <textarea
            class="block min-h-[320px] w-full resize-y bg-zinc-950 p-3 font-mono text-sm text-zinc-100 outline-none"
            bind:value={body}
            spellcheck="false"
          ></textarea>
        {:else}
          <div class="min-h-[320px] p-4 sm:p-5">
            <div
              class="prose prose-invert prose-zinc prose-sm max-w-none prose-pre:bg-zinc-900 prose-pre:ring-1 prose-pre:ring-zinc-800 prose-headings:tracking-tight"
            >
              {#if body.trim()}
                {@html previewHtml}
              {:else}
                <p class="text-zinc-500 italic">(empty body)</p>
              {/if}
            </div>
            {#if tagsList.length > 0 || mandatory}
              <div class="mt-4 flex flex-wrap items-center gap-1.5 border-t border-zinc-800 pt-3">
                <KindBadge kind={kind as KindStr} mode="icon_and_text" />
                {#if mandatory}
                  <span
                    class="inline-flex items-center rounded-md bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase text-amber-300 ring-1 ring-inset ring-amber-500/30"
                  >
                    mandatory
                  </span>
                {/if}
                {#each tagsList as tag (tag)}
                  <span
                    class="inline-flex items-center rounded-md bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-300"
                  >
                    {tag}
                  </span>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
      </div>
    </div>
  </div>

  <!-- Footer: always visible regardless of scroll position. -->
  <footer
    class="flex shrink-0 flex-wrap items-center gap-3 border-t border-zinc-800 bg-zinc-900/40 px-4 py-3 sm:px-6"
  >
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
  </footer>
</section>
