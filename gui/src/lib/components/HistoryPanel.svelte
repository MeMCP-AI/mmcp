<script lang="ts">
  import KindBadge from './KindBadge.svelte';
  import { marked } from 'marked';
  import {
    AlignJustify,
    Columns,
    GitCommit,
    LoaderCircle,
    Mail,
    Pilcrow,
    User,
    X
  } from 'lucide-svelte';
  import { fly } from 'svelte/transition';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { diffMemory, listMemoryHistory, loadMemoryAt } from '$lib/api/history';
  import { settingsStore, type DiffViewMode } from '$lib/stores/settings.svelte';
  import type { CommitMeta, DiffResult, DiffRow, DiffSpan, KindStr, MemoryFile } from '$lib/types';

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

  const diffView = $derived(settingsStore.values.diff_view);

  const DIFF_VIEW_BUTTONS: { mode: DiffViewMode; label: string; Icon: typeof Columns }[] = [
    { mode: 'unified', label: 'Unified', Icon: AlignJustify },
    { mode: 'inline_word', label: 'Word', Icon: Pilcrow },
    { mode: 'side_by_side', label: 'Split', Icon: Columns }
  ];

  // Pair adjacent delete-run + insert-run into replace rows for
  // side-by-side rendering. Leftovers (mismatched counts) stay as
  // standalone delete/insert rows rendered with a blank counterpart
  // column so line numbers stay aligned.
  interface PairEqual {
    kind: 'equal';
    old_lineno: number;
    new_lineno: number;
    text: string;
  }
  interface PairDelete {
    kind: 'delete';
    old_lineno: number;
    text: string;
    spans: DiffSpan[];
  }
  interface PairInsert {
    kind: 'insert';
    new_lineno: number;
    text: string;
    spans: DiffSpan[];
  }
  interface PairReplace {
    kind: 'replace';
    old_lineno: number;
    old_text: string;
    old_spans: DiffSpan[];
    new_lineno: number;
    new_text: string;
    new_spans: DiffSpan[];
  }
  type PairedRow = PairEqual | PairDelete | PairInsert | PairReplace;

  function pairRows(rows: DiffRow[]): PairedRow[] {
    const out: PairedRow[] = [];
    let i = 0;
    while (i < rows.length) {
      const row = rows[i];
      if (row.kind === 'equal') {
        out.push({
          kind: 'equal',
          old_lineno: row.old_lineno,
          new_lineno: row.new_lineno,
          text: row.text
        });
        i++;
        continue;
      }
      const deletes: Extract<DiffRow, { kind: 'delete' }>[] = [];
      while (i < rows.length && rows[i].kind === 'delete') {
        deletes.push(rows[i] as Extract<DiffRow, { kind: 'delete' }>);
        i++;
      }
      const inserts: Extract<DiffRow, { kind: 'insert' }>[] = [];
      while (i < rows.length && rows[i].kind === 'insert') {
        inserts.push(rows[i] as Extract<DiffRow, { kind: 'insert' }>);
        i++;
      }
      const pairs = Math.min(deletes.length, inserts.length);
      for (let k = 0; k < pairs; k++) {
        out.push({
          kind: 'replace',
          old_lineno: deletes[k].old_lineno,
          old_text: deletes[k].text,
          old_spans: deletes[k].spans,
          new_lineno: inserts[k].new_lineno,
          new_text: inserts[k].text,
          new_spans: inserts[k].spans
        });
      }
      for (let k = pairs; k < deletes.length; k++) {
        out.push({
          kind: 'delete',
          old_lineno: deletes[k].old_lineno,
          text: deletes[k].text,
          spans: deletes[k].spans
        });
      }
      for (let k = pairs; k < inserts.length; k++) {
        out.push({
          kind: 'insert',
          new_lineno: inserts[k].new_lineno,
          text: inserts[k].text,
          spans: inserts[k].spans
        });
      }
    }
    return out;
  }

  const pairedRows = $derived(diff ? pairRows(diff.rows) : []);

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

  /// Silent refresh on mirror:changed. Refetches the commit list
  /// without touching `loadingCommits` so the UI doesn't flash a
  /// spinner; new commits animate in via the keyed `{#each}` +
  /// `transition:fly`.
  $effect(() => {
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;
    void (async () => {
      const off = await listen<{ group_id: string | null }>(
        'mirror:changed',
        async (e) => {
          const gid = e.payload?.group_id ?? null;
          if (gid !== null && gid !== groupId) return;
          try {
            const fresh = await listMemoryHistory(groupId, slug);
            commits = fresh;
          } catch {
            // Silent — user can refresh manually by toggling
            // history off/on if the list gets stuck.
          }
        }
      );
      if (cancelled) off();
      else unlisten = off;
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
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
              <li
                class="border-b border-zinc-900 last:border-b-0"
                transition:fly={{ y: -12, duration: 220 }}
              >
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
                <div class="flex flex-wrap items-center gap-1 border-b border-zinc-800 bg-zinc-900/20 px-2 py-1">
                  {#each DIFF_VIEW_BUTTONS as btn (btn.mode)}
                    {@const active = diffView === btn.mode}
                    <button
                      type="button"
                      class="inline-flex items-center gap-1.5 rounded-md px-2 py-0.5 text-[11px] font-medium transition-colors
                        {active
                        ? 'bg-sky-500/15 text-sky-100'
                        : 'text-zinc-400 hover:bg-zinc-800/70 hover:text-zinc-200'}"
                      onclick={() => settingsStore.setDiffView(btn.mode)}
                      title={btn.label + ' view'}
                    >
                      <btn.Icon size={11} />
                      <span>{btn.label}</span>
                    </button>
                  {/each}
                </div>
                {#if diff.rows.length === 0}
                  <div class="p-4 text-xs text-zinc-500 italic">
                    Commits are byte-identical at this path.
                  </div>
                {:else if diffView === 'unified'}
                  <table class="w-full border-collapse font-mono text-[11px] leading-5">
                    <tbody>
                      {#each diff.rows as row, idx (idx)}
                        {@const cls =
                          row.kind === 'insert'
                            ? 'bg-emerald-500/10 text-emerald-100'
                            : row.kind === 'delete'
                              ? 'bg-rose-500/10 text-rose-100'
                              : 'text-zinc-300'}
                        {@const sigil =
                          row.kind === 'insert' ? '+' : row.kind === 'delete' ? '-' : ' '}
                        <tr class={cls}>
                          <td
                            class="w-10 shrink-0 select-none border-r border-zinc-800/70 px-2 text-right text-[10px] text-zinc-600"
                          >
                            {row.kind === 'insert' ? '' : row.old_lineno}
                          </td>
                          <td
                            class="w-10 shrink-0 select-none border-r border-zinc-800/70 px-2 text-right text-[10px] text-zinc-600"
                          >
                            {row.kind === 'delete' ? '' : row.new_lineno}
                          </td>
                          <td
                            class="w-4 shrink-0 select-none px-1 text-center text-zinc-500"
                          >
                            {sigil}
                          </td>
                          <td class="whitespace-pre-wrap break-all px-2 py-0">{row.text}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                {:else if diffView === 'inline_word'}
                  <!-- Same row order as unified but inline word spans paint only the
                       fragments that actually diverged, so the user can spot the exact
                       words that changed inside a mostly-similar line. -->
                  <table class="w-full border-collapse font-mono text-[11px] leading-5">
                    <tbody>
                      {#each diff.rows as row, idx (idx)}
                        {@const cls =
                          row.kind === 'insert'
                            ? 'bg-emerald-500/5'
                            : row.kind === 'delete'
                              ? 'bg-rose-500/5'
                              : ''}
                        {@const sigil =
                          row.kind === 'insert' ? '+' : row.kind === 'delete' ? '-' : ' '}
                        <tr class={cls}>
                          <td
                            class="w-10 shrink-0 select-none border-r border-zinc-800/70 px-2 text-right text-[10px] text-zinc-600"
                          >
                            {row.kind === 'insert' ? '' : row.old_lineno}
                          </td>
                          <td
                            class="w-10 shrink-0 select-none border-r border-zinc-800/70 px-2 text-right text-[10px] text-zinc-600"
                          >
                            {row.kind === 'delete' ? '' : row.new_lineno}
                          </td>
                          <td
                            class="w-4 shrink-0 select-none px-1 text-center text-zinc-500"
                          >
                            {sigil}
                          </td>
                          <td class="whitespace-pre-wrap break-all px-2 py-0 text-zinc-300">
                            {#if row.kind === 'equal'}
                              {row.text}
                            {:else}
                              {#each row.spans as span, sidx (sidx)}
                                {#if row.kind === 'insert'}
                                  <span
                                    class={span.emphasized
                                      ? 'bg-emerald-500/40 text-emerald-50 rounded-sm px-0.5'
                                      : 'text-emerald-200/80'}>{span.text}</span
                                  >
                                {:else}
                                  <span
                                    class={span.emphasized
                                      ? 'bg-rose-500/40 text-rose-50 rounded-sm px-0.5 line-through decoration-rose-300/70'
                                      : 'text-rose-200/80'}>{span.text}</span
                                  >
                                {/if}
                              {/each}
                              {#if row.spans.length === 0}{row.text}{/if}
                            {/if}
                          </td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                {:else}
                  <!-- Side-by-side: two mirrored columns. Adjacent delete+insert runs
                       pair up so a modified line shows on the same row; leftovers get
                       an empty counterpart cell so line numbers stay honest. -->
                  <table class="w-full border-collapse font-mono text-[11px] leading-5">
                    <colgroup>
                      <col class="w-10" />
                      <col />
                      <col class="w-10" />
                      <col />
                    </colgroup>
                    <tbody>
                      {#each pairedRows as row, idx (idx)}
                        {#if row.kind === 'equal'}
                          <tr class="text-zinc-300">
                            <td
                              class="select-none border-r border-zinc-800/70 px-2 text-right text-[10px] text-zinc-600"
                            >
                              {row.old_lineno}
                            </td>
                            <td class="whitespace-pre-wrap break-all border-r border-zinc-800 px-2 py-0">
                              {row.text}
                            </td>
                            <td
                              class="select-none border-r border-zinc-800/70 px-2 text-right text-[10px] text-zinc-600"
                            >
                              {row.new_lineno}
                            </td>
                            <td class="whitespace-pre-wrap break-all px-2 py-0">{row.text}</td>
                          </tr>
                        {:else if row.kind === 'replace'}
                          <tr>
                            <td
                              class="select-none border-r border-zinc-800/70 bg-rose-500/5 px-2 text-right text-[10px] text-zinc-600"
                            >
                              {row.old_lineno}
                            </td>
                            <td
                              class="whitespace-pre-wrap break-all border-r border-zinc-800 bg-rose-500/10 px-2 py-0 text-rose-100"
                            >
                              {#each row.old_spans as span, sidx (sidx)}
                                <span
                                  class={span.emphasized
                                    ? 'bg-rose-500/40 text-rose-50 rounded-sm px-0.5'
                                    : ''}>{span.text}</span
                                >
                              {/each}
                              {#if row.old_spans.length === 0}{row.old_text}{/if}
                            </td>
                            <td
                              class="select-none border-r border-zinc-800/70 bg-emerald-500/5 px-2 text-right text-[10px] text-zinc-600"
                            >
                              {row.new_lineno}
                            </td>
                            <td
                              class="whitespace-pre-wrap break-all bg-emerald-500/10 px-2 py-0 text-emerald-100"
                            >
                              {#each row.new_spans as span, sidx (sidx)}
                                <span
                                  class={span.emphasized
                                    ? 'bg-emerald-500/40 text-emerald-50 rounded-sm px-0.5'
                                    : ''}>{span.text}</span
                                >
                              {/each}
                              {#if row.new_spans.length === 0}{row.new_text}{/if}
                            </td>
                          </tr>
                        {:else if row.kind === 'delete'}
                          <tr>
                            <td
                              class="select-none border-r border-zinc-800/70 bg-rose-500/5 px-2 text-right text-[10px] text-zinc-600"
                            >
                              {row.old_lineno}
                            </td>
                            <td
                              class="whitespace-pre-wrap break-all border-r border-zinc-800 bg-rose-500/10 px-2 py-0 text-rose-100"
                            >
                              {row.text}
                            </td>
                            <td
                              class="select-none border-r border-zinc-800/70 px-2 text-right text-[10px] text-zinc-600"
                            ></td>
                            <td class="px-2 py-0"></td>
                          </tr>
                        {:else}
                          <tr>
                            <td
                              class="select-none border-r border-zinc-800/70 px-2 text-right text-[10px] text-zinc-600"
                            ></td>
                            <td class="border-r border-zinc-800 px-2 py-0"></td>
                            <td
                              class="select-none border-r border-zinc-800/70 bg-emerald-500/5 px-2 text-right text-[10px] text-zinc-600"
                            >
                              {row.new_lineno}
                            </td>
                            <td
                              class="whitespace-pre-wrap break-all bg-emerald-500/10 px-2 py-0 text-emerald-100"
                            >
                              {row.text}
                            </td>
                          </tr>
                        {/if}
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
