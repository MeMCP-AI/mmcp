<script lang="ts">
  import KindBadge from './KindBadge.svelte';
  import {
    Database,
    FolderOpen,
    Info,
    LoaderCircle,
    Monitor,
    Moon,
    Palette,
    Sun,
    User,
    Waypoints,
    X
  } from '@lucide/svelte';
  import type { KindStr, LoadedProjectConfig, ProjectConfig, UserConfig } from '$lib/types';
  import type { KindDisplay, ThemeMode } from '$lib/stores/settings.svelte';
  import RemotesEditor from './RemotesEditor.svelte';
  import { fromDraft, toDraft, type DraftRemote } from '$lib/utils/remotes_draft';

  interface Props {
    value: KindDisplay;
    theme: ThemeMode;
    onChange: (mode: KindDisplay) => void;
    onThemeChange: (mode: ThemeMode) => void;
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
    theme,
    onChange,
    onThemeChange,
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

  const THEME_OPTIONS: { mode: ThemeMode; label: string; Icon: typeof Sun }[] = [
    { mode: 'dark', label: 'Dark', Icon: Moon },
    { mode: 'light', label: 'Light', Icon: Sun },
    { mode: 'system', label: 'System', Icon: Monitor }
  ];

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
  let draftUserSyncServerUrl = $state('');
  let draftUserRemotes = $state<DraftRemote[]>([]);

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
    draftUserSyncServerUrl = c.sync?.server_url ?? '';
    draftUserRemotes = (c.sync?.remotes ?? []).map(toDraft);
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
    const hasAuthor = authorName !== null || authorEmail !== null || fallback !== null;
    const hasDefaults = defaultGroup !== null;
    const cfg: UserConfig = {
      // Always sent as a complete object (never dropped for
      // emptiness): the exact save path FR-301's review fan-out
      // flagged, a partial reconstruction silently losing `remotes`.
      sync: {
        server_url: emptyToNull(draftUserSyncServerUrl),
        remotes: draftUserRemotes.map(fromDraft)
      },
      author: hasAuthor
        ? { name: authorName, email: authorEmail, git_fallback: fallback }
        : null,
      defaults: hasDefaults ? { group: defaultGroup } : null,
      // This form has no UI for limits; carry the loaded value through
      // unchanged so a save never silently erases an operator-set
      // `[limits]` section (mmcp review finding, repair round 4).
      limits: userConfig?.limits ?? null
    };
    onSaveUser(cfg);
  }

  // Project form mirrors projectConfig.config when available.
  let draftProjectSlug = $state('');
  let draftProjectSyncServerUrl = $state('');
  let draftProjectRemotes = $state<DraftRemote[]>([]);
  let draftProjectRemoteOnly = $state(false);
  let draftProjectNoDefault = $state(false);
  let draftProjectAdditional = $state('');
  let draftProjectLangUse = $state('');
  let draftProjectAutoDetect = $state(false);

  $effect(() => {
    const c = projectConfig?.config;
    if (!c) return;
    draftProjectSlug = c.project_slug ?? '';
    draftProjectSyncServerUrl = c.sync?.server_url ?? '';
    draftProjectRemotes = (c.sync?.remotes ?? []).map(toDraft);
    draftProjectRemoteOnly = c.project_remote_only;
    draftProjectNoDefault = c.subscriptions.no_default_global;
    draftProjectAdditional = c.subscriptions.groups.join(', ');
    draftProjectLangUse = c.subscriptions.languages.join(', ');
    draftProjectAutoDetect = c.subscriptions.auto_detect_languages;
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
      project_slug: emptyToNull(draftProjectSlug) ?? undefined,
      // Always sent as a complete object; see the matching comment
      // in `commitUser`.
      sync: {
        server_url: emptyToNull(draftProjectSyncServerUrl),
        remotes: draftProjectRemotes.map(fromDraft)
      },
      project_remote_only: draftProjectRemoteOnly,
      subscriptions: {
        no_default_global: draftProjectNoDefault,
        auto_detect_languages: draftProjectAutoDetect,
        languages: parseCsv(draftProjectLangUse),
        groups: parseCsv(draftProjectAdditional),
        memories: current.subscriptions.memories,
        tags: current.subscriptions.tags
      }
    };
    onSaveProject(root, cfg);
  }
</script>

<section class="flex h-full min-h-0 flex-col overflow-hidden bg-surface-0 text-fg">
  <header
    class="flex shrink-0 items-center gap-3 border-b border-line bg-surface-1/40 px-4 py-2 sm:px-6"
  >
    <h1 class="text-sm font-semibold text-fg">Settings</h1>
    {#if saving}
      <span class="inline-flex items-center gap-1.5 text-[11px] text-fg-muted">
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
      class="ml-auto rounded-md p-1 text-fg-muted hover:bg-surface-2 hover:text-fg"
      aria-label="Close"
      title="Close"
      onclick={onClose}
    >
      <X size={14} />
    </button>
  </header>

  <div class="flex min-h-0 flex-1 flex-col overflow-hidden sm:flex-row">
    <nav
      class="flex shrink-0 gap-1 overflow-x-auto border-b border-line bg-surface-0/40 p-2 sm:w-48 sm:flex-col sm:gap-0 sm:overflow-x-visible sm:border-b-0 sm:border-r"
    >
      {#each TABS as t (t.id)}
        {@const active = tab === t.id}
        <button
          type="button"
          class="flex shrink-0 items-center gap-2 rounded-md px-3 py-1.5 text-sm transition-colors
            {active
            ? 'bg-sky-500/15 text-selected-fg'
            : 'text-fg-muted hover:bg-surface-2/70 hover:text-fg'}"
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
            <h3 class="text-sm font-semibold text-fg">Theme</h3>
            <p class="mt-0.5 text-xs text-fg-subtle">
              Follow the operating system, or pin to a specific colour scheme.
            </p>
            <div class="mt-3 grid grid-cols-3 gap-2">
              {#each THEME_OPTIONS as opt (opt.mode)}
                {@const active = theme === opt.mode}
                <button
                  type="button"
                  class="flex items-center justify-center gap-1.5 rounded-md border px-2 py-1.5 text-xs font-medium transition-colors
                    {active
                    ? 'border-sky-500/50 bg-sky-500/15 text-selected-fg'
                    : 'border-line-strong text-fg-muted hover:bg-surface-2'}"
                  onclick={() => onThemeChange(opt.mode)}
                  aria-pressed={active}
                  title={`${opt.label} theme`}
                >
                  <opt.Icon size={13} />
                  {opt.label}
                </button>
              {/each}
            </div>
          </div>

          <div>
            <h3 class="text-sm font-semibold text-fg">Memory list prefix</h3>
            <p class="mt-0.5 text-xs text-fg-subtle">
              How the kind badge appears next to each slug in the memory list.
            </p>
            <div class="mt-3 flex flex-col gap-1.5">
              {#each KIND_OPTIONS as opt (opt.mode)}
                <label class="flex cursor-pointer items-center gap-2 text-sm text-fg">
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
            <h3 class="text-xs font-semibold uppercase tracking-wide text-fg-muted">Preview</h3>
            <div class="mt-2 rounded-lg border border-line bg-surface-0 p-3">
              <ul class="flex flex-col gap-1 font-mono text-sm">
                {#each SAMPLES as sample (sample.slug)}
                  <li class="flex items-center gap-2 text-fg">
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
            <h3 class="text-sm font-semibold text-fg">Reference point</h3>
            <p class="mt-0.5 text-xs text-fg-subtle">
              Directory mmcp-gui anchors project-config discovery on. When set,
              the backend walks up from here to find a <code
                class="rounded bg-surface-2 px-1 py-0.5 text-fg-muted">.mmcp.toml</code
              >. When cleared, it falls back to the launching shell's cwd.
            </p>
            <div
              class="mt-3 rounded-md border border-line bg-surface-0 px-3 py-2 font-mono text-xs text-fg-muted"
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
                class="inline-flex items-center rounded-md border border-line-strong px-3 py-1.5 text-sm text-fg hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-50"
                onclick={onClearReferencePoint}
                disabled={!referencePoint || saving}
              >
                Clear
              </button>
            </div>
          </div>

          <div>
            <h3 class="text-xs font-semibold uppercase tracking-wide text-fg-muted">
              Project detection
            </h3>
            <div class="mt-2 rounded-lg border border-line bg-surface-0 p-3 text-sm">
              {#if projectConfig?.root}
                <div class="text-fg">
                  Found <code class="rounded bg-surface-2 px-1 py-0.5 text-fg-muted"
                    >.mmcp.toml</code
                  >
                  at:
                </div>
                <div class="mt-1 break-all font-mono text-xs text-fg-muted">
                  {projectConfig.root}
                </div>
              {:else}
                <span class="text-fg-subtle">
                  No <code class="rounded bg-surface-2 px-1 py-0.5 text-fg-muted"
                    >.mmcp.toml</code
                  >
                  found under the reference point. Run
                  <code class="rounded bg-surface-2 px-1 py-0.5 text-fg-muted">mmcp init project</code>
                  inside the folder to create one.
                </span>
              {/if}
            </div>
          </div>
        </section>
      {:else if tab === 'user'}
        <section class="flex flex-col gap-5">
          <div>
            <h3 class="text-sm font-semibold text-fg">User config</h3>
            <p class="mt-0.5 text-xs text-fg-subtle">
              Defaults that apply across every project. Stored at
              <code class="rounded bg-surface-2 px-1 py-0.5 text-fg-muted">
                {userPath ?? '~/.mmcp/config.toml'}
              </code>.
            </p>
          </div>

          {#if !userConfig}
            <div class="text-xs text-fg-subtle">Loading…</div>
          {:else}
            <div class="grid grid-cols-[140px_1fr] items-center gap-3 text-sm">
              <label for="u-name" class="text-fg-muted">author name</label>
              <input
                id="u-name"
                type="text"
                class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
                bind:value={draftUserName}
                placeholder="(unset — uses mmcp fallback)"
              />

              <label for="u-email" class="text-fg-muted">author email</label>
              <input
                id="u-email"
                type="text"
                class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
                bind:value={draftUserEmail}
                placeholder="(unset — uses mmcp fallback)"
              />

              <label for="u-git" class="text-fg-muted">git fallback</label>
              <select
                id="u-git"
                class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
                bind:value={draftUserGitFallback}
              >
                <option value="unset">Unset (warn)</option>
                <option value="enabled">Enabled (read global git config)</option>
                <option value="disabled">Disabled (never read git config)</option>
              </select>

              <label for="u-group" class="text-fg-muted">default group</label>
              <input
                id="u-group"
                type="text"
                class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
                bind:value={draftUserDefaultGroup}
                placeholder="slug or UUID"
              />

              <label for="u-sync" class="text-fg-muted">legacy sync URL</label>
              <input
                id="u-sync"
                type="text"
                class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
                bind:value={draftUserSyncServerUrl}
                placeholder="https://… (optional single-remote shorthand)"
              />
            </div>

            <div>
              <h3 class="text-sm font-semibold text-fg">Default sync remotes</h3>
              <p class="mt-0.5 text-xs text-fg-subtle">
                Inherited by every project unless a project sets its own <code
                  class="rounded bg-surface-2 px-1 py-0.5 text-[11px] text-fg-muted"
                  >project_remote_only</code
                >.
              </p>
              <div class="mt-3">
                <RemotesEditor
                  remotes={draftUserRemotes}
                  onChange={(r) => (draftUserRemotes = r)}
                  disabled={saving}
                />
              </div>
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
            <h3 class="text-sm font-semibold text-fg">Project config</h3>
            <p class="mt-0.5 text-xs text-fg-subtle">
              The <code class="rounded bg-surface-2 px-1 py-0.5 text-fg-muted">.mmcp.toml</code>
              resolved at the current reference point. Sync changes take effect
              after you pick the folder again from the Workspace tab.
            </p>
          </div>

          {#if !projectConfig?.config}
            <div class="rounded-md border border-line bg-surface-0 p-3 text-xs text-fg-subtle">
              No project config found under the reference point. Set a folder
              on the Workspace tab and run <code
                class="rounded bg-surface-2 px-1 py-0.5 text-fg-muted">mmcp init project</code
              >
              if the folder is a new project.
            </div>
          {:else}
            {@const cfg = projectConfig.config}
            <div class="grid grid-cols-[140px_1fr] items-center gap-3 text-sm">
              <span class="text-fg-muted">project UUID</span>
              <code
                class="truncate rounded bg-surface-0 px-2 py-1.5 font-mono text-xs text-fg-muted ring-1 ring-inset ring-line"
                title={cfg.project_uuid}
              >
                {cfg.project_uuid}
              </code>

              <label for="p-slug" class="text-fg-muted">project slug</label>
              <input
                id="p-slug"
                type="text"
                class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
                bind:value={draftProjectSlug}
              />

              <label for="p-sync" class="text-fg-muted">legacy sync URL</label>
              <input
                id="p-sync"
                type="text"
                class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
                bind:value={draftProjectSyncServerUrl}
                placeholder="(empty = local-only single-remote shorthand)"
              />

              <span class="self-start pt-1.5 text-fg-muted">remotes</span>
              <div class="flex flex-col gap-2">
                <label class="flex items-center gap-2 text-fg">
                  <input type="checkbox" bind:checked={draftProjectRemoteOnly} />
                  use only this project's own remotes — do not inherit the user's
                </label>
              </div>

              <span class="self-start pt-1.5 text-fg-muted">groups</span>
              <div class="flex flex-col gap-2">
                <label class="flex items-center gap-2 text-fg">
                  <input type="checkbox" bind:checked={draftProjectNoDefault} />
                  skip the default <code
                    class="rounded bg-surface-2 px-1 py-0.5 text-[11px] text-fg-muted">global</code
                  > group
                </label>
                <input
                  type="text"
                  class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
                  bind:value={draftProjectAdditional}
                  placeholder="additional groups — comma-separated"
                />
              </div>

              <span class="self-start pt-1.5 text-fg-muted">languages</span>
              <div class="flex flex-col gap-2">
                <label class="flex items-center gap-2 text-fg">
                  <input type="checkbox" bind:checked={draftProjectAutoDetect} />
                  auto-detect languages from marker files
                </label>
                <input
                  type="text"
                  class="rounded-md border border-line-strong bg-surface-0 px-2 py-1.5 text-fg"
                  bind:value={draftProjectLangUse}
                  placeholder="explicit languages — comma-separated"
                />
              </div>
            </div>

            <div>
              <h3 class="text-sm font-semibold text-fg">Project sync remotes</h3>
              <p class="mt-0.5 text-xs text-fg-subtle">
                A <code class="rounded bg-surface-2 px-1 py-0.5 text-[11px] text-fg-muted"
                  >direct-git</code
                > entry with no group defaults to this project's own group.
              </p>
              <div class="mt-3">
                <RemotesEditor
                  remotes={draftProjectRemotes}
                  onChange={(r) => (draftProjectRemotes = r)}
                  disabled={saving}
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
            <h3 class="text-sm font-semibold text-fg">Settings file</h3>
            <p class="mt-0.5 text-xs text-fg-subtle">
              Persists across restarts. Written by the Tauri backend on every change.
            </p>
            <div
              class="mt-3 rounded-md border border-line bg-surface-0 px-3 py-2 font-mono text-xs text-fg-muted"
            >
              {settingsPathHint}
            </div>
          </div>

          <div>
            <h3 class="text-sm font-semibold text-fg">Reset to defaults</h3>
            <p class="mt-0.5 text-xs text-fg-subtle">
              Clears every preference on this device. Memories and groups are untouched.
            </p>
            {#if !resetConfirm}
              <button
                type="button"
                class="mt-3 inline-flex items-center rounded-md border border-line-strong px-3 py-1.5 text-sm text-fg hover:bg-surface-2"
                onclick={() => (resetConfirm = true)}
              >
                Reset settings…
              </button>
            {:else}
              <div class="mt-3 flex flex-wrap items-center gap-2">
                <span class="text-xs text-fg-muted">Confirm reset?</span>
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
                  class="inline-flex items-center rounded-md border border-line-strong px-3 py-1.5 text-sm text-fg hover:bg-surface-2"
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
            <h3 class="text-sm font-semibold text-fg">mmcp-gui</h3>
            <p class="mt-0.5 text-xs text-fg-subtle">
              Desktop visual client for mmcp memories. Reads, writes, and syncs through
              <code class="rounded bg-surface-2 px-1 py-0.5 text-fg-muted">mmcp-store</code> directly —
              no HTTP, no MCP round-trip.
            </p>
          </div>

          <dl class="grid grid-cols-[120px_1fr] gap-y-2 text-sm">
            <dt class="text-fg-subtle">Version</dt>
            <dd class="text-fg">0.1.0</dd>
            <dt class="text-fg-subtle">Shell</dt>
            <dd class="text-fg">Tauri 2</dd>
            <dt class="text-fg-subtle">Frontend</dt>
            <dd class="text-fg">SvelteKit 2 · Svelte 5 runes · Tailwind v4</dd>
            <dt class="text-fg-subtle">Icons</dt>
            <dd class="text-fg">Lucide</dd>
            <dt class="text-fg-subtle">Package manager</dt>
            <dd class="text-fg">bun</dd>
          </dl>

          <p class="text-xs text-fg-subtle">
            See <code class="rounded bg-surface-2 px-1 py-0.5 text-fg-muted">gui/README.md</code>
            for setup and build instructions.
          </p>
        </section>
      {/if}
    </div>
  </div>
</section>
