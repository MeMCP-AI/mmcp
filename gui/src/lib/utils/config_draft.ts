import type { ProjectConfig, UserConfig } from '$lib/types';
import { fromDraft, type DraftRemote } from '$lib/utils/remotes_draft';

/** Values the Settings panel's user form edits. Every key of `UserConfig`
 * the form does not list is carried from the loaded config untouched. */
export interface UserForm {
  name: string;
  email: string;
  gitFallback: 'unset' | 'enabled' | 'disabled';
  defaultGroup: string;
  syncServerUrl: string;
  remotes: DraftRemote[];
}

/** Values the Settings panel's project form edits. Every key of
 * `ProjectConfig` the form does not list is carried from the loaded
 * config untouched. */
export interface ProjectForm {
  slug: string;
  syncServerUrl: string;
  remotes: DraftRemote[];
  remoteOnly: boolean;
  noDefaultGlobal: boolean;
  autoDetectLanguages: boolean;
  /** Comma-separated extra groups. */
  groups: string;
  /** Comma-separated languages. */
  languages: string;
}

/** `null` for a blank string, the trimmed text otherwise. */
export function emptyToNull(text: string): string | null {
  const trimmed = text.trim();
  return trimmed.length > 0 ? trimmed : null;
}

/** Comma-separated text as a list of trimmed, non-empty entries. */
export function parseCsv(text: string): string[] {
  return text
    .split(',')
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0);
}

function gitFallbackValue(choice: UserForm['gitFallback']): boolean | null {
  if (choice === 'enabled') return true;
  if (choice === 'disabled') return false;
  return null;
}

/** The `UserConfig` a user-form save sends to the backend.
 * Starts from the loaded config and overrides only the fields the form edits, so every other key
 * (`limits`, `claude_md`, `projects`, and any key a later version adds to the type) survives the save. */
export function buildUserConfig(loaded: UserConfig | null, form: UserForm): UserConfig {
  const name = emptyToNull(form.name);
  const email = emptyToNull(form.email);
  const fallback = gitFallbackValue(form.gitFallback);
  const defaultGroup = emptyToNull(form.defaultGroup);
  const hasAuthor = name !== null || email !== null || fallback !== null;
  return {
    ...loaded,
    // Always sent as a complete object (never dropped for emptiness):
    // a partial reconstruction would silently lose `remotes`.
    sync: {
      server_url: emptyToNull(form.syncServerUrl),
      remotes: form.remotes.map(fromDraft)
    },
    author: hasAuthor ? { name, email, git_fallback: fallback } : null,
    defaults: defaultGroup !== null ? { group: defaultGroup } : null,
    limits: loaded?.limits ?? null
  };
}

/** The `ProjectConfig` a project-form save sends to the backend.
 * Starts from the loaded config and overrides only the fields the form edits, so `claude_md`, the
 * `memories` and `tags` subscriptions, and any key a later version adds to the type survive the save. */
export function buildProjectConfig(loaded: ProjectConfig, form: ProjectForm): ProjectConfig {
  return {
    ...loaded,
    project_slug: emptyToNull(form.slug) ?? undefined,
    // Always sent as a complete object; see the matching comment in `buildUserConfig`.
    sync: {
      server_url: emptyToNull(form.syncServerUrl),
      remotes: form.remotes.map(fromDraft)
    },
    project_remote_only: form.remoteOnly,
    subscriptions: {
      ...loaded.subscriptions,
      no_default_global: form.noDefaultGlobal,
      auto_detect_languages: form.autoDetectLanguages,
      languages: parseCsv(form.languages),
      groups: parseCsv(form.groups)
    }
  };
}
