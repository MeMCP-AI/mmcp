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
        tags: tagsList,
        // The editor doesn't expose cross-refs yet; preserve
        // whatever the source memory carried so a round-trip
        // edit doesn't drop the list on the floor.
        refs: initial?.frontmatter.refs ?? []
      },
      body
    };
    onSave(memory, slug.trim());
  }

  const tabBtn = (active: boolean) =>
    'rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors ' +
    (active
      ? 'bg-sky-500/15 text-selected-fg'
      : 'text-fg-muted hover:bg-surface-2/70 hover:text-fg');
</script>

<section class="flex h-full min-h-0 flex-col overflow-hidden bg-surface-0">
  <!-- Header: title only. Never scrolls. -->
  <header
    class="flex shrink-0 items-center gap-3 border-b border-line bg-surface-1/40 px-4 py-2 sm:px-6"
  >
    <h1 class="text-sm font-semibold text-fg">
      {mode === 'new' ? 'New memory' : 'Edit memory'}
    </h1>
  </header>

  <!-- Scrollable body. Frontmatter form always visible; body section
       has its own Edit/Preview toggle so metadata stays in view. -->
  <div class="min-h-0 flex-1 overflow-y-auto">
    <div class="mx-auto max-w-4xl px-4 py-5 sm:px-6 sm:py-6">
      <!-- frontmatter form -->
      <div class="rounded-lg border border-line bg-surface-1 p-4 sm:p-5">
        <div
          class="grid grid-cols-[100px_1fr] items-center gap-3 text-sm sm:grid-cols-[120px_1fr]"
        >
          <label for="slug" class="text-fg-muted">slug</label>
          <input
            id="slug"
            type="text"
            class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg disabled:opacity-50"
            bind:value={slug}
            disabled={mode === 'edit'}
            placeholder="kebab-case, e.g. rule-commit-format"
          />

          <label for="name" class="text-fg-muted">name</label>
          <input
            id="name"
            type="text"
            class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
            bind:value={name}
          />

          <label for="description" class="text-fg-muted">description</label>
          <input
            id="description"
            type="text"
            class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
            bind:value={description}
          />

          <label for="kind" class="text-fg-muted">kind</label>
          <select
            id="kind"
            class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
            bind:value={kind}
          >
            <option value="rule">rule</option>
            <option value="snapshot">snapshot</option>
            <option value="log">log</option>
            <option value="reference">reference</option>
            <option value="scratch">scratch</option>
            <option value="feature">feature</option>
          </select>

          <label for="mandatory" class="text-fg-muted">mandatory</label>
          <input
            id="mandatory"
            type="checkbox"
            bind:checked={mandatory}
            class="justify-self-start"
          />

          <label for="tags" class="self-start pt-1.5 text-fg-muted">tags</label>
          <div class="flex flex-col gap-2">
            <input
              id="tags"
              type="text"
              class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
              bind:value={tags}
              placeholder="comma-separated"
            />
            {#if tagsList.length > 0}
              <div class="flex flex-wrap gap-1.5">
                {#each tagsList as tag (tag)}
                  <span
                    class="inline-flex items-center rounded-md bg-surface-2 px-1.5 py-0.5 text-[10px] text-fg-muted"
                  >
                    {tag}
                  </span>
                {/each}
              </div>
            {:else}
              <span class="text-[10px] text-fg-subtle">No tags yet — comma-separate to add.</span>
            {/if}
          </div>
        </div>
      </div>

      <!-- Body editor with inline Edit/Preview tabs. -->
      <div class="mt-5 overflow-hidden rounded-md border border-line bg-surface-0">
        <div
          class="flex items-center justify-between border-b border-line bg-surface-1/40 px-3 py-1.5"
        >
          <div class="text-[11px] font-semibold uppercase tracking-wide text-fg-subtle">
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
            class="block min-h-[320px] w-full resize-y bg-surface-0 p-3 font-mono text-sm text-fg outline-none"
            bind:value={body}
            spellcheck="false"
          ></textarea>
        {:else}
          <div class="min-h-[320px] p-4 sm:p-5">
            <div
              class="prose prose-zinc prose-sm max-w-none prose-pre:bg-surface-1 prose-pre:ring-1 prose-pre:ring-line prose-headings:tracking-tight"
            >
              {#if body.trim()}
                {@html previewHtml}
              {:else}
                <p class="text-fg-subtle italic">(empty body)</p>
              {/if}
            </div>
            {#if tagsList.length > 0 || mandatory}
              <div class="mt-4 flex flex-wrap items-center gap-1.5 border-t border-line pt-3">
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
                    class="inline-flex items-center rounded-md bg-surface-2 px-1.5 py-0.5 text-[10px] text-fg-muted"
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

  <!-- Footer: always visible regardless of scroll position.
       Validation message (when present) stays flush-left; the
       action buttons are pushed to the right edge so the primary
       actions sit where the mouse expects them in modal-style
       editors. -->
  <footer
    class="flex shrink-0 flex-wrap items-center gap-3 border-t border-line bg-surface-1/40 px-4 py-3 sm:px-6"
  >
    {#if validation}
      <span class="text-xs text-rose-400">{validation}</span>
    {/if}
    <button
      type="button"
      class="ml-auto inline-flex items-center rounded-md border border-line-strong px-3 py-1.5 text-sm text-fg hover:bg-surface-2"
      onclick={onCancel}
    >
      Cancel
    </button>
    <button
      type="button"
      class="inline-flex items-center rounded-md bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-50"
      disabled={validation !== null}
      onclick={handleSave}
    >
      {mode === 'new' ? 'Create' : 'Save'}
    </button>
  </footer>
</section>
