import type { Remote, RemoteAuth } from '$lib/types';

/** Editable draft of one `Remote`, plus a UI-only stable key for
 * `{#each}` keying. Carries every per-kind field regardless of the
 * current `kind` so switching kind in the editor doesn't lose an
 * already-typed value; `fromDraft` strips whatever the target kind
 * doesn't use before it reaches `save_project_config`/`save_user_config`. */
export interface DraftRemote {
  _key: string;
  kind: 'mmcp-server' | 'direct-git';
  name: string;
  url: string;
  default: boolean;
  include_in_push_all: boolean;
  auth: RemoteAuth;
  group: string;
}

let nextKey = 0;

/** Fresh UI-only key, unique within this session. Never sent to the backend. */
export function freshDraftKey(): string {
  nextKey += 1;
  return `draft-${nextKey}`;
}

/** Convert a loaded `Remote` into an editable draft. */
export function toDraft(remote: Remote): DraftRemote {
  return {
    _key: freshDraftKey(),
    kind: remote.kind,
    name: remote.name,
    url: remote.url,
    default: remote.default,
    include_in_push_all: remote.include_in_push_all,
    auth: remote.kind === 'direct-git' ? remote.auth : 'none',
    group: remote.kind === 'direct-git' ? (remote.group ?? '') : ''
  };
}

/** A new, empty draft of the given kind, for the "add remote" action. */
export function newDraft(kind: DraftRemote['kind']): DraftRemote {
  return {
    _key: freshDraftKey(),
    kind,
    name: '',
    url: '',
    default: false,
    include_in_push_all: true,
    auth: 'none',
    group: ''
  };
}

/** Strip the UI-only `_key` and whichever fields the draft's `kind`
 * doesn't use, producing the real `Remote` the save path sends back. */
export function fromDraft(draft: DraftRemote): Remote {
  const name = draft.name.trim();
  const url = draft.url.trim();
  if (draft.kind === 'mmcp-server') {
    return {
      kind: 'mmcp-server',
      name,
      url,
      default: draft.default,
      include_in_push_all: draft.include_in_push_all
    };
  }
  const group = draft.group.trim();
  return {
    kind: 'direct-git',
    name,
    url,
    auth: draft.auth,
    ...(group.length > 0 ? { group } : {}),
    default: draft.default,
    include_in_push_all: draft.include_in_push_all
  };
}
