/// <reference types="bun-types" />
import { describe, expect, test } from 'bun:test';
import type { ProjectConfig, UserConfig } from '$lib/types';
import {
  buildProjectConfig,
  buildUserConfig,
  emptyToNull,
  parseCsv,
  type ProjectForm,
  type UserForm
} from './config_draft';

const PROJECT_UUID = '018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91';

const USER_FORM: UserForm = {
  name: 'Alice',
  email: '',
  gitFallback: 'unset',
  defaultGroup: '',
  syncServerUrl: '',
  remotes: []
};

const PROJECT_FORM: ProjectForm = {
  slug: 'team-acme',
  syncServerUrl: '',
  remotes: [],
  remoteOnly: false,
  noDefaultGlobal: false,
  autoDetectLanguages: false,
  groups: '',
  languages: ''
};

function loadedUser(): UserConfig {
  return {
    sync: { server_url: null, remotes: [] },
    author: null,
    defaults: null,
    limits: {
      max_auto_slug_length: 80,
      min_password_length: null,
      max_password_length: null,
      max_handle_length: null
    },
    claude_md: { project_file_suggestion: 'maybe', future_key: true },
    projects: {
      [PROJECT_UUID]: { claude_md: { project_file_suggestion: 'decline' } },
      '019d955d-4cce-77f2-a0b3-0b79ed394612': { claude_md: { project_file_suggestion: 7 } }
    }
  };
}

function loadedProject(): ProjectConfig {
  return {
    project_uuid: PROJECT_UUID,
    project_remote_only: false,
    subscriptions: {
      no_default_global: false,
      auto_detect_languages: false,
      languages: [],
      groups: [],
      memories: ['019d955d-4cce-77f2-a0b3-0b79ed394612:rule'],
      tags: ['git']
    },
    claude_md: { project_file_suggestion: 'maybe', future_key: true }
  };
}

describe('user config builder', () => {
  test('keeps the claude_md table and every projects entry the form does not edit', () => {
    const loaded = loadedUser();

    const built = buildUserConfig(loaded, USER_FORM);

    expect(built.claude_md).toEqual(loaded.claude_md);
    expect(built.projects).toEqual(loaded.projects);
  });

  test('keeps a raw invalid claude_md table and a key unknown to the form', () => {
    const loaded = { ...loadedUser(), future_top_level_key: { nested: 1 } } as UserConfig;

    const built = buildUserConfig(loaded, USER_FORM) as UserConfig & {
      future_top_level_key?: unknown;
    };

    expect(built.claude_md).toEqual({ project_file_suggestion: 'maybe', future_key: true });
    expect(built.future_top_level_key).toEqual({ nested: 1 });
  });

  test('keeps the limits section the form has no control for', () => {
    const built = buildUserConfig(loadedUser(), USER_FORM);

    expect(built.limits?.max_auto_slug_length).toBe(80);
  });

  test('applies the edited fields over the loaded ones', () => {
    const built = buildUserConfig(loadedUser(), {
      ...USER_FORM,
      email: ' alice@example.com ',
      gitFallback: 'enabled',
      defaultGroup: 'my-group',
      syncServerUrl: 'https://mmcp.example.com'
    });

    expect(built.author).toEqual({
      name: 'Alice',
      email: 'alice@example.com',
      git_fallback: true
    });
    expect(built.defaults).toEqual({ group: 'my-group' });
    expect(built.sync).toEqual({ server_url: 'https://mmcp.example.com', remotes: [] });
  });

  test('drops the author and defaults sections when the form blanks them', () => {
    const built = buildUserConfig(loadedUser(), { ...USER_FORM, name: '  ' });

    expect(built.author).toBeNull();
    expect(built.defaults).toBeNull();
  });

  test('builds from no loaded config', () => {
    const built = buildUserConfig(null, USER_FORM);

    expect(built.author?.name).toBe('Alice');
    expect(built.limits).toBeNull();
    expect(built.claude_md).toBeUndefined();
    expect(built.projects).toBeUndefined();
  });

  test('never touches the loaded config object', () => {
    const loaded = loadedUser();
    const before = JSON.stringify(loaded);

    buildUserConfig(loaded, { ...USER_FORM, name: 'Bob' });

    expect(JSON.stringify(loaded)).toBe(before);
  });
});

describe('project config builder', () => {
  test('keeps the claude_md table, raw and with its unknown key', () => {
    const loaded = loadedProject();

    const built = buildProjectConfig(loaded, PROJECT_FORM);

    expect(built.claude_md).toEqual(loaded.claude_md);
  });

  test('keeps a key unknown to the form', () => {
    const loaded = { ...loadedProject(), future_key: 'kept' } as ProjectConfig;

    const built = buildProjectConfig(loaded, PROJECT_FORM) as ProjectConfig & {
      future_key?: string;
    };

    expect(built.future_key).toBe('kept');
  });

  test('keeps the project uuid and the memories and tags subscriptions', () => {
    const loaded = loadedProject();

    const built = buildProjectConfig(loaded, PROJECT_FORM);

    expect(built.project_uuid).toBe(PROJECT_UUID);
    expect(built.subscriptions.memories).toEqual(loaded.subscriptions.memories);
    expect(built.subscriptions.tags).toEqual(loaded.subscriptions.tags);
  });

  test('applies the edited fields over the loaded ones', () => {
    const built = buildProjectConfig(loadedProject(), {
      ...PROJECT_FORM,
      remoteOnly: true,
      noDefaultGlobal: true,
      autoDetectLanguages: true,
      groups: 'team/a, team/b,',
      languages: 'rust'
    });

    expect(built.project_slug).toBe('team-acme');
    expect(built.project_remote_only).toBe(true);
    expect(built.subscriptions.no_default_global).toBe(true);
    expect(built.subscriptions.auto_detect_languages).toBe(true);
    expect(built.subscriptions.groups).toEqual(['team/a', 'team/b']);
    expect(built.subscriptions.languages).toEqual(['rust']);
  });

  test('leaves the slug out when the form blanks it', () => {
    const built = buildProjectConfig(
      { ...loadedProject(), project_slug: 'old' },
      { ...PROJECT_FORM, slug: ' ' }
    );

    expect(built.project_slug).toBeUndefined();
  });

  test('never touches the loaded config object', () => {
    const loaded = loadedProject();
    const before = JSON.stringify(loaded);

    buildProjectConfig(loaded, { ...PROJECT_FORM, groups: 'x' });

    expect(JSON.stringify(loaded)).toBe(before);
  });
});

describe('text helpers', () => {
  test('emptyToNull trims and nulls blanks', () => {
    expect(emptyToNull('  ')).toBeNull();
    expect(emptyToNull(' a ')).toBe('a');
  });

  test('parseCsv trims entries and drops empty ones', () => {
    expect(parseCsv(' a, b ,,c,')).toEqual(['a', 'b', 'c']);
    expect(parseCsv('')).toEqual([]);
  });
});
