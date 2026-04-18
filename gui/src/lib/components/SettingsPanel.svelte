<script lang="ts">
  import KindBadge from './KindBadge.svelte';
  import {
    Database,
    FolderOpen,
    Info,
    LoaderCircle,
    Palette,
    User,
    Waypoints,
    X
  } from 'lucide-svelte';
  import type { KindStr, LoadedProjectConfig, ProjectConfig, UserConfig } from '$lib/types';
  import type { KindDisplay } from '$lib/stores/settings.svelte';

  interface Props {
    value: KindDisplay;
    onChange: (mode: KindDisplay) => void;
    onReset: () => void;
    onClose: () => void;
    referencePoint: string | null;
    onPickReferencePoint: () => void;
    onClearReferencePoint: () => void;
    userConfig: UserConfig | null;
    userPath: string | null;
    projectConfig: LoadedProjectConfig | null;
    onSaveUser: (cfg: UserConfig) => void;
    onSaveProject: (root: string, cfg: ProjectConfig) => void;
    saving: boolean;
    lastError: string | null;
  }

  let {
    value,
    onChange,
    onReset,
    onClose,
    referencePoint,
    onPickReferencePoint,
    onClearReferencePoint,
    userConfig,
    userPath,
    projectConfig,
    onSaveUser,
    onSaveProject,
    saving,
    lastError
  }: Props = $props();

  type TabId = 'appearance' | 'workspace' | 'user' | 'project' | 'storage' | 'about';
  let tab = $state<TabId>('appearance');

  const TABS: { id: TabId; label: string; Icon: typeof Palette }[] = [
    { id: 'appearance', label: 'Appearance', Icon: Palette },
    { id: 'workspace', label: 'Workspace', Icon: FolderOpen },
    { id: 'user', label: 'User', Icon: User },
    { id: 'project', label: 'Project', Icon: Waypoints },
    { id: 'storage', label: 'Storage', Icon: Database },
    { id: 'about', label: 'About', Icon: Info }
  ];

  const KIND_OPTIONS: { mode: KindDisplay; label: string }[] = [
    { mode: 'off', label: 'Off' },
    { mode: 'icon', label: 'Icon only' },
    { mode: 'text', label: 'Text only' },
    { mode: 'icon_and_text', label: 'Icon + text' }
  ];

  const SAMPLES: { kind: KindStr; slug: string }[] = [
    { kind: 'rule', slug: 'branch-policy' },
    { kind: 'snapshot', slug: 'repo-state-2026-04-18' },
    { kind: 'log', slug: 'incident-2026-03-05' },
    { kind: 'reference', slug: 'gitoxide-upstream' },
    { kind: 'scratch', slug: 'draft-notes' },
    { kind: 'feature', slug: 'fr-020-extract-mmcp-store' }
  ];

  const settingsPathHint =
    typeof navigator !== 'undefined' && navigator.userAgent.includes('Windows')
      ? '%APPDATA%\\mmcp-gui\\settings.json'
      : '~/.config/mmcp-gui/settings.json';

  let resetConfirm = $state(false);

  // Editable copies for the user / project forms. $derived picks up
  // store refreshes (e.g. after save → reload → prop changes).
  let draftUserName = $state('');
  let draftUserEmail = $state('');
  let draftUserGitFallback = $state<'unset' | 'enabled' | 'disabled'>('unset');
  let draftUserDefaultGroup = $state('');
  let draftUserSyncUrl = $state('');

  $effect(() => {
    const c = userConfig;
    if (!c) return;
    draftUserName = c.author?.name ?? '';
    draftUserEmail = c.author?.email ?? '';
    draftUserGitFallback =
      c.author?.git_fallback === true
        ? 'enabled'
        : c.author?.git_fallback === false
          ? 'disabled'
          : 'unset';
    draftUserDefaultGroup = c.defaults?.group ?? '';
    draftUserSyncUrl = c.sync?.server_url ?? '';
  });

  function emptyToNull(s: string): string | null {
    const t = s.trim();
    return t.length > 0 ? t : null;
  }

  function commitUser() {
    const fallback =
      draftUserGitFallback === 'enabled'
        ? true
        : draftUserGitFallback === 'disabled'
          ? false
          : null;
    const authorName = emptyToNull(draftUserName);
    const authorEmail = emptyToNull(draftUserEmail);
    const defaultGroup = emptyToNull(draftUserDefaultGroup);
    const syncUrl = emptyToNull(draftUserSyncUrl);
    const hasAuthor = authorName !== null || authorEmail !== null || fallback !== null;
    const hasDefaults = defaultGroup !== null;
    const hasSync = syncUrl !== null;
    const cfg: UserConfig = {
      sync: hasSync ? { server_url: syncUrl! } : null,
      author: hasAuthor
        ? { name: authorName, email: authorEmail, git_fallback: fallback }
        : null,
      defaults: hasDefaults ? { group: defaultGroup } : null
    };
    onSaveUser(cfg);
  }

  // Project form mirrors projectConfig.config when available.
  let draftProjectSlug = $state('');
  let draftProjectSyncUrl = $state('');
  let draftProjectNoDefault = $state(false);
  let draftProjectAdditional = $state('');
  let draftProjectLangUse = $state('');
  let draftProjectAutoDetect = $state(false);

  $effect(() => {
    const c = projectConfig?.config;
    if (!c) return;
    draftProjectSlug = c.project_slug ?? '';
    draftProjectSyncUrl = c.sync?.server_url ?? '';
    draftProjectNoDefault = c.groups?.no_default ?? false;
    draftProjectAdditional = (c.groups?.additional ?? []).join(', ');
    draftProjectLangUse = (c.languages?.use ?? []).join(', ');
    draftProjectAutoDetect = c.languages?.auto_detect ?? false;
  });

  function parseCsv(s: string): string[] {
    return s
      .split(',')
      .map((v) => v.trim())
      .filter((v) => v.length > 0);
  }

  function commitProject() {
    const current = projectConfig?.config;
    const root = projectConfig?.root;
    if (!current || !root) return;
    const cfg: ProjectConfig = {
      project_uuid: current.project_uuid,
      project_slug: emptyToNull(draftProjectSlug),
      sync: draftProjectSyncUrl.trim().length > 0
        ? { server_url: draftProjectSyncUrl.trim() }
        : null,
      groups: {
        no_default: draftProjectNoDefault,
        additional: parseCsv(draftProjectAdditional)
      },
      languages: {
        use: parseCsv(draftProjectLangUse),
        auto_detect: draftProjectAutoDetect
      }
    };
    onSaveProject(root, cfg);
  }
</script>

<section class="flex h-full min-h-0 flex-col overflow-hidden bg-zinc-950 text-zinc-100">
  <header
    class="flex shrink-0 items-center gap-3 border-b border-zinc-800 bg-zinc-900/40 px-4 py-2 sm:px-6"
  >
    <h1 class="text-sm font-semibold text-zinc-100">Settings</h1>
    {#if saving}
      <span class="inline-flex items-center gap-1.5 text-[11px] text-zinc-400">
        <LoaderCircle size={12} class="animate-spin" />
        Saving…
      </span>
    {/if}
    {#if lastError}
      <span class="truncate text-[11px] text-rose-400" title={lastError}>
        {lastError}
      </span>
    {/if}
    <button
      type="button"
      class="ml-auto rounded-md p-1 text-zinc-400 hover:bg-zinc-800 hover:text-zinc-100"
      aria-label="Close"
      title="Close"
      onclick={onClose}
    >
      <X size={14} />
    </button>
  </header>

  <div class="flex min-h-0 flex-1 flex-col overflow-hidden sm:flex-row">
    <nav
      class="flex shrink-0 gap-1 overflow-x-auto border-b border-zinc-800 bg-zinc-950/40 p-2 sm:w-48 sm:flex-col sm:gap-0 sm:overflow-x-visible sm:border-b-0 sm:border-r"
    >
      {#each TABS as t (t.id)}
        {@const active = tab === t.id}
        <button
          type="button"
          class="flex shrink-0 items-center gap-2 rounded-md px-3 py-1.5 text-sm transition-colors
            {active
            ? 'bg-sky-500/15 text-sky-100'
            : 'text-zinc-400 hover:bg-zinc-800/70 hover:text-zinc-200'}"
          onclick={() => (tab = t.id)}
        >
          <t.Icon size={14} />
          <span>{t.label}</span>
        </button>
      {/each}
    </nav>

    <div class="min-h-0 flex-1 overflow-y-auto p-5 sm:p-6">
      {#if tab === 'appearance'}
        <section class="flex flex-col gap-5">
          <div>
            <h3 class="text-sm font-semibold text-zinc-100">Memory list prefix</h3>
            <p class="mt-0.5 text-xs text-zinc-500">
              How the kind badge appears next to each slug in the memory list.
            </p>
            <div class="mt-3 flex flex-col gap-1.5">
              {#each KIND_OPTIONS as opt (opt.mode)}
                <label class="flex cursor-pointer items-center gap-2 text-sm text-zinc-200">
                  <input
                    type="radio"
                    name="kind-display"
                    value={opt.mode}
                    checked={value === opt.mode}
                    onchange={() => onChange(opt.mode)}
                  />
                  {opt.label}
                </label>
              {/each}
            </div>
          </div>

          <div>
            <h3 class="text-xs font-semibold uppercase tracking-wide text-zinc-400">Preview</h3>
            <div class="mt-2 rounded-lg border border-zinc-800 bg-zinc-950 p-3">
              <ul class="flex flex-col gap-1 font-mono text-sm">
                {#each SAMPLES as sample (sample.slug)}
                  <li class="flex items-center gap-2 text-zinc-200">
                    {#if value !== 'off'}
                      <KindBadge kind={sample.kind} mode={value} />
                    {/if}
                    <span>{sample.slug}</span>
                  </li>
                {/each}
              </ul>
            </div>
          </div>
        </section>
      {:else if tab === 'workspace'}
        <section class="flex flex-col gap-5">
          <div>
            <h3 class="text-sm font-semibold text-zinc-100">Reference point</h3>
            <p class="mt-0.5 text-xs text-zinc-500">
              Directory mmcp-gui anchors project-config discovery on. When set,
              the backend walks up from here to find a <code
                class="rounded bg-zinc-800 px-1 py-0.5 text-zinc-300">.mmcp.toml</code
              >. When cleared, it falls back to the launching shell's cwd.
            </p>
            <div
              class="mt-3 rounded-md border border-zinc-800 bg-zinc-950 px-3 py-2 font-mono text-xs text-zinc-300"
            >
              {referencePoint ?? '(unset — using process cwd)'}
            </div>
            <div class="mt-3 flex flex-wrap gap-2">
              <button
                type="button"
                class="inline-flex items-center gap-1.5 rounded-md bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-50"
                onclick={onPickReferencePoint}
                disabled={saving}
              >
                <FolderOpen size={12} />
                Choose folder…
              </button>
              <button
                type="button"
                class="inline-flex items-center rounded-md border border-zinc-700 px-3 py-1.5 text-sm text-zinc-200 hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-50"
                onclick={onClearReferencePoint}
                disabled={!referencePoint || saving}
              >
                Clear
              </button>
            </div>
          </div>

          <div>
            <h3 class="text-xs font-semibold uppercase tracking-wide text-zinc-400">
              Project detection
            </h3>
            <div class="mt-2 rounded-lg border border-zinc-800 bg-zinc-950 p-3 text-sm">
              {#if projectConfig?.root}
                <div class="text-zinc-200">
                  Found <code class="rounded bg-zinc-800 px-1 py-0.5 text-zinc-300"
                    >.mmcp.toml</code
                  >
                  at:
                </div>
                <div class="mt-1 break-all font-mono text-xs text-zinc-400">
                  {projectConfig.root}
                </div>
              {:else}
                <span class="text-zinc-500">
                  No <code class="rounded bg-zinc-800 px-1 py-0.5 text-zinc-300"
                    >.mmcp.toml</code
                  >
                  found under the reference point. Run
                  <code class="rounded bg-zinc-800 px-1 py-0.5 text-zinc-300">mmcp init project</code>
                  inside the folder to create one.
                </span>
              {/if}
            </div>
          </div>
        </section>
      {:else if tab === 'user'}
        <section class="flex flex-col gap-5">
          <div>
            <h3 class="text-sm font-semibold text-zinc-100">User config</h3>
            <p class="mt-0.5 text-xs text-zinc-500">
              Defaults that apply across every project. Stored at
              <code class="rounded bg-zinc-800 px-1 py-0.5 text-zinc-300">
                {userPath ?? '~/.mmcp/config.toml'}
              </code>.
            </p>
          </div>

          {#if !userConfig}
            <div class="text-xs text-zinc-500">Loading…</div>
          {:else}
            <div class="grid grid-cols-[140px_1fr] items-center gap-3 text-sm">
              <label for="u-name" class="text-zinc-400">author name</label>
              <input
                id="u-name"
                type="text"
                class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
                bind:value={draftUserName}
                placeholder="(unset — uses mmcp fallback)"
              />

              <label for="u-email" class="text-zinc-400">author email</label>
              <input
                id="u-email"
                type="text"
                class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
                bind:value={draftUserEmail}
                placeholder="(unset — uses mmcp fallback)"
              />

              <label for="u-git" class="text-zinc-400">git fallback</label>
              <select
                id="u-git"
                class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
                bind:value={draftUserGitFallback}
              >
                <option value="unset">Unset (warn)</option>
                <option value="enabled">Enabled (read global git config)</option>
                <option value="disabled">Disabled (never read git config)</option>
              </select>

              <label for="u-group" class="text-zinc-400">default group</label>
              <input
                id="u-group"
                type="text"
                class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
                bind:value={draftUserDefaultGroup}
                placeholder="slug or UUID"
              />

              <label for="u-sync" class="text-zinc-400">default sync URL</label>
              <input
                id="u-sync"
                type="text"
                class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
                bind:value={draftUserSyncUrl}
                placeholder="https://…"
              />
            </div>

            <div>
              <button
                type="button"
                class="inline-flex items-center rounded-md bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-50"
                onclick={commitUser}
                disabled={saving}
              >
                Save user config
              </button>
            </div>
          {/if}
        </section>
      {:else if tab === 'project'}
        <section class="flex flex-col gap-5">
          <div>
            <h3 class="text-sm font-semibold text-zinc-100">Project config</h3>
            <p class="mt-0.5 text-xs text-zinc-500">
              The <code class="rounded bg-zinc-800 px-1 py-0.5 text-zinc-300">.mmcp.toml</code>
              resolved at the current reference point. Sync changes take effect
              after you pick the folder again from the Workspace tab.
            </p>
          </div>

          {#if !projectConfig?.config}
            <div class="rounded-md border border-zinc-800 bg-zinc-950 p-3 text-xs text-zinc-500">
              No project config found under the reference point. Set a folder
              on the Workspace tab and run <code
                class="rounded bg-zinc-800 px-1 py-0.5 text-zinc-300">mmcp init project</code
              >
              if the folder is a new project.
            </div>
          {:else}
            {@const cfg = projectConfig.config}
            <div class="grid grid-cols-[140px_1fr] items-center gap-3 text-sm">
              <span class="text-zinc-400">project UUID</span>
              <code
                class="truncate rounded bg-zinc-950 px-2 py-1.5 font-mono text-xs text-zinc-400 ring-1 ring-inset ring-zinc-800"
                title={cfg.project_uuid}
              >
                {cfg.project_uuid}
              </code>

              <label for="p-slug" class="text-zinc-400">project slug</label>
              <input
                id="p-slug"
                type="text"
                class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
                bind:value={draftProjectSlug}
              />

              <label for="p-sync" class="text-zinc-400">sync server URL</label>
              <input
                id="p-sync"
                type="text"
                class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
                bind:value={draftProjectSyncUrl}
                placeholder="(empty = local-only)"
              />

              <label class="self-start pt-1.5 text-zinc-400">groups</label>
              <div class="flex flex-col gap-2">
                <label class="flex items-center gap-2 text-zinc-200">
                  <input type="checkbox" bind:checked={draftProjectNoDefault} />
                  skip the default <code
                    class="rounded bg-zinc-800 px-1 py-0.5 text-[11px] text-zinc-300">global</code
                  > group
                </label>
                <input
                  type="text"
                  class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
                  bind:value={draftProjectAdditional}
                  placeholder="additional groups — comma-separated"
                />
              </div>

              <label class="self-start pt-1.5 text-zinc-400">languages</label>
              <div class="flex flex-col gap-2">
                <label class="flex items-center gap-2 text-zinc-200">
                  <input type="checkbox" bind:checked={draftProjectAutoDetect} />
                  auto-detect languages from marker files
                </label>
                <input
                  type="text"
                  class="rounded-md border border-zinc-700 bg-zinc-950 px-2 py-1.5 text-zinc-100"
                  bind:value={draftProjectLangUse}
                  placeholder="explicit languages — comma-separated"
                />
              </div>
            </div>

            <div>
              <button
                type="button"
                class="inline-flex items-center rounded-md bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-50"
                onclick={commitProject}
                disabled={saving}
              >
                Save project config
              </button>
            </div>
          {/if}
        </section>
      {:else if tab === 'storage'}
        <section class="flex flex-col gap-5">
          <div>
            <h3 class="text-sm font-semibold text-zinc-100">Settings file</h3>
            <p class="mt-0.5 text-xs text-zinc-500">
              Persists across restarts. Written by the Tauri backend on every change.
            </p>
            <div
              class="mt-3 rounded-md border border-zinc-800 bg-zinc-950 px-3 py-2 font-mono text-xs text-zinc-300"
            >
              {settingsPathHint}
            </div>
          </div>

          <div>
            <h3 class="text-sm font-semibold text-zinc-100">Reset to defaults</h3>
            <p class="mt-0.5 text-xs text-zinc-500">
              Clears every preference on this device. Memories and groups are untouched.
            </p>
            {#if !resetConfirm}
              <button
                type="button"
                class="mt-3 inline-flex items-center rounded-md border border-zinc-700 px-3 py-1.5 text-sm text-zinc-200 hover:bg-zinc-800"
                onclick={() => (resetConfirm = true)}
              >
                Reset settings…
              </button>
            {:else}
              <div class="mt-3 flex flex-wrap items-center gap-2">
                <span class="text-xs text-zinc-400">Confirm reset?</span>
                <button
                  type="button"
                  class="inline-flex items-center rounded-md bg-rose-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-rose-500"
                  onclick={() => {
                    onReset();
                    resetConfirm = false;
                  }}
                >
                  Reset
                </button>
                <button
                  type="button"
                  class="inline-flex items-center rounded-md border border-zinc-700 px-3 py-1.5 text-sm text-zinc-200 hover:bg-zinc-800"
                  onclick={() => (resetConfirm = false)}
                >
                  Cancel
                </button>
              </div>
            {/if}
          </div>
        </section>
      {:else if tab === 'about'}
        <section class="flex flex-col gap-5">
          <div>
            <h3 class="text-sm font-semibold text-zinc-100">mmcp-gui</h3>
            <p class="mt-0.5 text-xs text-zinc-500">
              Desktop visual client for mmcp memories. Reads, writes, and syncs through
              <code class="rounded bg-zinc-800 px-1 py-0.5 text-zinc-300">mmcp-store</code> directly —
              no HTTP, no MCP round-trip.
            </p>
          </div>

          <dl class="grid grid-cols-[120px_1fr] gap-y-2 text-sm">
            <dt class="text-zinc-500">Version</dt>
            <dd class="text-zinc-200">0.1.0</dd>
            <dt class="text-zinc-500">Shell</dt>
            <dd class="text-zinc-200">Tauri 2</dd>
            <dt class="text-zinc-500">Frontend</dt>
            <dd class="text-zinc-200">SvelteKit 2 · Svelte 5 runes · Tailwind v4</dd>
            <dt class="text-zinc-500">Icons</dt>
            <dd class="text-zinc-200">Lucide</dd>
            <dt class="text-zinc-500">Package manager</dt>
            <dd class="text-zinc-200">bun</dd>
          </dl>

          <p class="text-xs text-zinc-500">
            See <code class="rounded bg-zinc-800 px-1 py-0.5 text-zinc-300">gui/README.md</code>
            for setup and build instructions.
          </p>
        </section>
      {/if}
    </div>
  </div>
</section>
