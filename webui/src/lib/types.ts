// Hand-written TS mirrors of the mmcp-server JSON wire types. The
// shapes here overlap with `gui/src/lib/types.ts` — `KindStr`,
// `MemoryFrontmatter`, `MemoryFile`, `Issue`, etc. are verbatim
// copies. The server-perspective DTOs (LoginOk, RemoteGroup, …) are
// webui-specific because they mirror the HTTP wire format, not the
// Tauri invoke layer the gui consumes.

export type KindStr =
  | 'rule'
  | 'snapshot'
  | 'log'
  | 'reference'
  | 'scratch'
  | 'feature';

// ── Common memory DTOs (shared shape with gui) ──────────────────

export interface MemoryFrontmatter {
  id: string | null;
  name: string;
  description: string;
  kind: KindStr;
  mandatory: boolean;
  version: string | null;
  tags: string[];
}

export interface MemoryFile {
  frontmatter: MemoryFrontmatter;
  body: string;
}

// ── mmcp-server HTTP wire types ─────────────────────────────────

/** Response of `POST /auth/login`. */
export interface LoginOk {
  token: string;
  user_id: string;
  /** Unix seconds. */
  expires_at: number;
}

/** One entry in `GET /sync/manifest`. */
export interface RemoteGroup {
  group_id: string;
  slug: string;
  head_commit: string;
}

/** Wraps `GET /sync/manifest`. */
export interface ManifestEnvelope {
  groups: RemoteGroup[];
}

/** Summary of one memory as served by the `list_memories` tool. */
export interface MemoryDescriptor {
  id: string;
  group: string;
  slug: string;
  name: string;
  description: string;
  kind: KindStr;
  mandatory: boolean;
  latest_version: string | null;
}

/** Response of `GET /health`. */
export interface HealthInfo {
  status: string;
  name: string;
  version: string;
}

/** `group_info` tool response. */
export interface GroupInfo {
  id: string;
  slug: string;
  owner: string;
  display_name: string | null;
  memory_count: number;
  effective_role: string;
}
