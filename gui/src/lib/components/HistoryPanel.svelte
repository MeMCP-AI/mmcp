<script lang="ts">
  import KindBadge from './KindBadge.svelte';
  import { marked } from 'marked';
  import { GitCommit, LoaderCircle, Mail, User, X } from 'lucide-svelte';
  import { diffMemory, listMemoryHistory, loadMemoryAt } from '$lib/api/history';
  import type { CommitMeta, DiffResult, KindStr, MemoryFile } from '$lib/types';

  interface Props {
    groupId: string;
    slug: string;
    onClose: () => void;
  }

  let { groupId, slug, onClose }: Props = $props();

  let commits = $state<CommitMeta[] | null>(null);
  let loadingCommits = $state(false);
  let commitsError = $state<string | null>(null);

  let selectedId = $state<string | null>(null);
  let memoryAt = $state<MemoryFile | null>(null);
  let loadingMemory = $state(false);
  let memoryError = $state<string | null>(null);

  let detailTab = $state<'memory' | 'diff'>('memory');
  let diff = $state<DiffResult | null>(null);
  let loadingDiff = $state(false);
  let diffError = $state<string | null>(null);

  // Walking `git log` a second time for a brand-new memory is
  // cheap; we don't cache across tab swaps to keep the store
  // simple. The commit list itself is cached in `commits`.

  // Refetch the commit list whenever the viewer points at a new
  // memory. Auto-select the newest commit on first load.
  $effect(() => {
    const g = groupId;
    const s = slug;
    loadingCommits = true;
    commitsError = null;
    commits = null;
    selectedId = null;
    memoryAt = null;
    diff = null;
    void (async () => {
      try {
        const list = await listMemoryHistory(g, s);
        commits = list;
        if (list.length > 0) selectedId = list[0].id;
      } catch (err) {
        commitsError = formatErr(err);
      } finally {
        loadingCommits = false;
      }
    })();
  });

  // Read the memory at the currently selected commit.
  $effect(() => {
    const id = selectedId;
    if (!id) return;
    loadingMemory = true;
    memoryError = null;
    memoryAt = null;
    void (async () => {
      try {
        memoryAt = await loadMemoryAt(groupId, slug, id);
      } catch (err) {
        memoryError = formatErr(err);
      } finally {
        loadingMemory = false;
      }
    })();
  });

  // Compute the diff against the previous commit (the next entry
  // in the list since `commits` is newest-first). The initial
  // commit has no parent — send `null` and render an all-insert
  // diff labelled as "introduced".
  $effect(() => {
    const id = selectedId;
    const list = commits;
    if (!id || !list) return;
    if (detailTab !== 'diff') return;
    loadingDiff = true;
    diffError = null;
    diff = null;
    const idx = list.findIndex((c) => c.id === id);
    const parent = idx >= 0 && idx + 1 < list.length ? list[idx + 1].id : null;
    void (async () => {
      try {
        diff = await diffMemory(groupId, slug, parent, id);
      } catch (err) {
        diffError = formatErr(err);
      } finally {
        loadingDiff = false;
      }
    })();
  });

  marked.setOptions({ breaks: false, gfm: true });
  const previewHtml = $derived(memoryAt ? (marked.parse(memoryAt.body) as string) : '');

  function formatErr(err: unknown): string {
    if (err && typeof err === 'object' && 'message' in err) {
      return String((err as { message: unknown }).message);
    }
    return String(err);
  }

  // Compact relative-time helper. Grains larger than a day snap to
  // absolute ISO date so the list doesn't lie about month boundaries.
  function timeAgo(epochSeconds: number): string {
    const now = Math.floor(Date.now() / 1000);
    const delta = now - epochSeconds;
    if (delta < 0) return 'just now';
    if (delta < 60) return `${delta}s ago`;
    if (delta < 3600) return `${Math.floor(delta / 60)}m ago`;
    if (delta < 86400) return `${Math.floor(delta / 3600)}h ago`;
    if (delta < 86400 * 14) return `${Math.floor(delta / 86400)}d ago`;
    const d = new Date(epochSeconds * 1000);
    return d.toISOString().slice(0, 10);
  }

  function fullTimestamp(epochSeconds: number): string {
    const d = new Date(epochSeconds * 1000);
    return d.toLocaleString();
  }
</script>

<section class="flex h-full min-h-0 flex-col overflow-hidden bg-zinc-950 text-zinc-100">
  <header
    class="flex shrink-0 items-center gap-2 border-b border-zinc-800 bg-zinc-900/40 px-4 py-2 sm:px-6"
  >
    <GitCommit size={14} class="text-zinc-400" />
    <h1 class="text-sm font-semibold text-zinc-100">History</h1>
    <code
      class="truncate rounded bg-zinc-800 px-1.5 py-0.5 text-xs text-zinc-300"
      title={slug}
    >
      {slug}
    </code>
    {#if commits}
      <span class="text-[11px] text-zinc-500">
        {commits.length} commit{commits.length === 1 ? '' : 's'}
      </span>
    {/if}
    <button
      type="button"
      class="ml-auto rounded-md p-1 text-zinc-400 hover:bg-zinc-800 hover:text-zinc-100"
      aria-label="Close history"
      title="Close history"
      onclick={onClose}
    >
      <X size={14} />
    </button>
  </header>

  <div class="flex min-h-0 flex-1 flex-col overflow-hidden md:flex-row">
    <!-- Commit list. Narrow on desktop, full-width on mobile with
         a small max-height so the detail still has room. -->
    <div
      class="flex shrink-0 flex-col overflow-hidden border-b border-zinc-800 md:w-72 md:border-b-0 md:border-r"
    >
      <div class="min-h-0 flex-1 overflow-y-auto">
        {#if loadingCommits}
          <div class="flex items-center gap-2 px-3 py-2 text-xs text-zinc-500">
            <LoaderCircle size={12} class="animate-spin" />
            Walking history…
          </div>
        {:else if commitsError}
          <div class="m-3 rounded-md border border-rose-900/60 bg-rose-950/40 p-2 text-xs text-rose-200">
            {commitsError}
          </div>
        {:else if commits && commits.length === 0}
          <div class="px-3 py-2 text-xs text-zinc-500">No commits touched this memory yet.</div>
        {:else if commits}
          <ul class="flex flex-col">
            {#each commits as commit, idx (commit.id)}
              {@const active = selectedId === commit.id}
              {@const latest = idx === 0}
              <li class="border-b border-zinc-900 last:border-b-0">
                <button
                  type="button"
                  class="flex w-full flex-col gap-0.5 px-3 py-2 text-left transition-colors
                    {active
                    ? 'bg-sky-500/15 text-sky-100'
                    : 'text-zinc-200 hover:bg-zinc-800/70'}"
                  onclick={() => (selectedId = commit.id)}
                  title={commit.subject}
                >
                  <div class="flex items-center gap-2">
                    <code
                      class="shrink-0 rounded bg-zinc-800 px-1 py-0.5 font-mono text-[10px] text-zinc-300"
                      title={commit.id}
                    >
                      {commit.short_id}
                    </code>
                    {#if latest}
                      <span
                        class="shrink-0 rounded-md bg-emerald-500/15 px-1 py-0.5 text-[9px] font-semibold uppercase tracking-wide text-emerald-300 ring-1 ring-inset ring-emerald-500/30"
                      >
                        head
                      </span>
                    {/if}
                    <span
                      class="ml-auto shrink-0 text-[10px] text-zinc-500"
                      title={fullTimestamp(commit.timestamp)}
                    >
                      {timeAgo(commit.timestamp)}
                    </span>
                  </div>
                  <span class="truncate text-xs" title={commit.subject}>{commit.subject}</span>
                  <span
                    class="truncate text-[10px] text-zinc-500"
                    title="{commit.author_name} <{commit.author_email}>"
                  >
                    {commit.author_name}
                  </span>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </div>

    <!-- Commit detail + memory / diff. -->
    <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
      {#if !selectedId}
        <div class="flex flex-1 items-center justify-center p-6 text-sm text-zinc-500">
          Pick a commit on the left.
        </div>
      {:else}
        {@const current = commits?.find((c) => c.id === selectedId)}
        <!-- Tab bar. Sits above the scroll container so switching
             views doesn't jump back to the top of the commit card. -->
        <div
          class="flex shrink-0 items-center gap-1 border-b border-zinc-800 bg-zinc-900/40 px-3 py-1.5"
        >
          <button
            type="button"
            class="rounded-md px-2.5 py-1 text-xs font-medium transition-colors
              {detailTab === 'memory'
              ? 'bg-sky-500/15 text-sky-100'
              : 'text-zinc-400 hover:bg-zinc-800/70 hover:text-zinc-200'}"
            onclick={() => (detailTab = 'memory')}
          >
            Memory
          </button>
          <button
            type="button"
            class="inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 text-xs font-medium transition-colors
              {detailTab === 'diff'
              ? 'bg-sky-500/15 text-sky-100'
              : 'text-zinc-400 hover:bg-zinc-800/70 hover:text-zinc-200'}"
            onclick={() => (detailTab = 'diff')}
          >
            Diff
            {#if diff && (diff.inserted > 0 || diff.deleted > 0)}
              <span class="text-[10px] text-emerald-300">+{diff.inserted}</span>
              <span class="text-[10px] text-rose-300">-{diff.deleted}</span>
            {/if}
          </button>
        </div>

        <div class="min-h-0 flex-1 overflow-y-auto">
          <div class="mx-auto max-w-4xl px-4 py-5 sm:px-6 sm:py-6">
            {#if current}
              <div class="rounded-lg border border-zinc-800 bg-zinc-900 p-4 sm:p-5">
                <div class="flex flex-wrap items-center gap-2 text-[11px] text-zinc-500">
                  <code
                    class="rounded bg-zinc-800 px-1.5 py-0.5 font-mono text-[11px] text-zinc-200"
                    title={current.id}
                  >
                    {current.short_id}
                  </code>
                  <span title={fullTimestamp(current.timestamp)}>
                    {timeAgo(current.timestamp)} — {fullTimestamp(current.timestamp)}
                  </span>
                </div>
                <h2 class="mt-2 text-base font-semibold text-zinc-50">{current.subject}</h2>
                {#if current.message.trim() !== current.subject.trim()}
                  <pre
                    class="mt-2 whitespace-pre-wrap font-mono text-xs text-zinc-300"
                  >{current.message}</pre>
                {/if}
                <div class="mt-3 flex flex-wrap gap-3 text-xs text-zinc-400">
                  <span class="inline-flex items-center gap-1">
                    <User size={12} /> {current.author_name}
                  </span>
                  <span class="inline-flex items-center gap-1">
                    <Mail size={12} /> {current.author_email}
                  </span>
                </div>
              </div>
            {/if}

            {#if detailTab === 'memory'}
              {#if loadingMemory}
                <div
                  class="mt-5 flex items-center justify-center gap-2 rounded-lg border border-zinc-800 bg-zinc-900 p-6 text-sm text-zinc-500"
                >
                  <LoaderCircle size={14} class="animate-spin" />
                  Reading memory at commit…
                </div>
              {:else if memoryError}
                <div
                  class="mt-5 rounded-md border border-rose-900/60 bg-rose-950/40 p-3 text-sm text-rose-200"
                >
                  {memoryError}
                </div>
              {:else if memoryAt}
                <div class="mt-5 rounded-lg border border-zinc-800 bg-zinc-900 p-4 sm:p-5">
                  <h3
                    class="text-base font-semibold text-zinc-50"
                    title={memoryAt.frontmatter.name}
                  >
                    {memoryAt.frontmatter.name}
                  </h3>
                  <p
                    class="mt-0.5 text-sm text-zinc-400"
                    title={memoryAt.frontmatter.description}
                  >
                    {memoryAt.frontmatter.description}
                  </p>
                  <div class="mt-3 flex flex-wrap items-center gap-1.5">
                    <KindBadge
                      kind={memoryAt.frontmatter.kind as KindStr}
                      mode="icon_and_text"
                    />
                    {#if memoryAt.frontmatter.mandatory}
                      <span
                        class="inline-flex items-center rounded-md bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase text-amber-300 ring-1 ring-inset ring-amber-500/30"
                      >
                        mandatory
                      </span>
                    {/if}
                    {#if memoryAt.frontmatter.version}
                      <span
                        class="inline-flex items-center rounded-md bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-300"
                        title="version"
                      >
                        v{memoryAt.frontmatter.version}
                      </span>
                    {/if}
                    {#each memoryAt.frontmatter.tags as tag (tag)}
                      <span
                        class="inline-flex items-center rounded-md bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-300"
                        title={tag}
                      >
                        {tag}
                      </span>
                    {/each}
                  </div>
                </div>

                <div class="mt-5 rounded-lg border border-zinc-800 bg-zinc-900/40 p-4 sm:p-5">
                  <div
                    class="prose prose-invert prose-zinc prose-sm max-w-none prose-pre:bg-zinc-950 prose-pre:ring-1 prose-pre:ring-zinc-800 prose-headings:tracking-tight"
                  >
                    {#if memoryAt.body.trim()}
                      {@html previewHtml}
                    {:else}
                      <p class="text-zinc-500 italic">(empty body)</p>
                    {/if}
                  </div>
                </div>
              {/if}
            {:else if loadingDiff}
              <div
                class="mt-5 flex items-center justify-center gap-2 rounded-lg border border-zinc-800 bg-zinc-900 p-6 text-sm text-zinc-500"
              >
                <LoaderCircle size={14} class="animate-spin" />
                Computing diff…
              </div>
            {:else if diffError}
              <div
                class="mt-5 rounded-md border border-rose-900/60 bg-rose-950/40 p-3 text-sm text-rose-200"
              >
                {diffError}
              </div>
            {:else if diff}
              <div
                class="mt-5 overflow-hidden rounded-lg border border-zinc-800 bg-zinc-950"
              >
                <div
                  class="flex flex-wrap items-center gap-2 border-b border-zinc-800 bg-zinc-900/40 px-3 py-1.5 text-[11px] text-zinc-400"
                >
                  <span>
                    {#if diff.from}
                      <code
                        class="rounded bg-zinc-800 px-1 py-0.5 text-[10px] text-zinc-300"
                        title={diff.from}
                      >
                        {diff.from.slice(0, 7)}
                      </code>
                      →
                    {:else}
                      <span
                        class="rounded-md bg-emerald-500/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase text-emerald-300 ring-1 ring-inset ring-emerald-500/30"
                      >
                        introduced
                      </span>
                      →
                    {/if}
                    <code
                      class="rounded bg-zinc-800 px-1 py-0.5 text-[10px] text-zinc-300"
                      title={diff.to}
                    >
                      {diff.to.slice(0, 7)}
                    </code>
                  </span>
                  <span class="ml-auto text-[10px]">
                    <span class="text-emerald-300">+{diff.inserted}</span>
                    <span class="text-rose-300">-{diff.deleted}</span>
                  </span>
                </div>
                {#if diff.lines.length === 0}
                  <div class="p-4 text-xs text-zinc-500 italic">
                    Commits are byte-identical at this path.
                  </div>
                {:else}
                  <table class="w-full border-collapse font-mono text-[11px] leading-5">
                    <tbody>
                      {#each diff.lines as line, idx (idx)}
                        {@const cls =
                          line.kind === 'insert'
                            ? 'bg-emerald-500/10 text-emerald-100'
                            : line.kind === 'delete'
                              ? 'bg-rose-500/10 text-rose-100'
                              : 'text-zinc-300'}
                        {@const sigil =
                          line.kind === 'insert' ? '+' : line.kind === 'delete' ? '-' : ' '}
                        <tr class={cls}>
                          <td
                            class="w-10 shrink-0 select-none border-r border-zinc-800/70 px-2 text-right text-[10px] text-zinc-600"
                          >
                            {line.old_lineno ?? ''}
                          </td>
                          <td
                            class="w-10 shrink-0 select-none border-r border-zinc-800/70 px-2 text-right text-[10px] text-zinc-600"
                          >
                            {line.new_lineno ?? ''}
                          </td>
                          <td
                            class="w-4 shrink-0 select-none px-1 text-center text-zinc-500"
                          >
                            {sigil}
                          </td>
                          <td class="whitespace-pre-wrap break-all px-2 py-0">{line.text}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                {/if}
              </div>
            {/if}
          </div>
        </div>
      {/if}
    </div>
  </div>
</section>
