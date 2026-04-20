import type {
  GroupInfo,
  HealthInfo,
  ManifestEnvelope,
  MemoryDescriptor,
  RemoteGroup
} from '$lib/types';
import { request } from './client';

export function serverHealth(): Promise<HealthInfo> {
  return request<HealthInfo>('/health', { auth: false });
}

export async function listManifest(): Promise<RemoteGroup[]> {
  const env = await request<ManifestEnvelope>('/sync/manifest');
  return env.groups;
}

// MCP tool dispatcher. Every tool call is wrapped in a typed envelope
// the server unwraps and dispatches in `crates/mmcp-server/src/mcp.rs`.
interface ToolEnvelope<Req> {
  tool: string;
  request: Req;
}
interface ToolResponse<Resp> {
  tool: string;
  response: Resp;
}

async function callTool<Req, Resp>(tool: string, req: Req): Promise<Resp> {
  const body: ToolEnvelope<Req> = { tool, request: req };
  const wrapped = await request<ToolResponse<Resp>>('/mcp/tool', {
    method: 'POST',
    body
  });
  return wrapped.response;
}

interface ListMemoriesReq {
  group: string | null;
  kinds: string[];
  only_mandatory: boolean | null;
}
interface ListMemoriesResp {
  memories: MemoryDescriptor[];
}

export function listMemories(group: string): Promise<MemoryDescriptor[]> {
  return callTool<ListMemoriesReq, ListMemoriesResp>('list_memories', {
    group,
    kinds: [],
    only_mandatory: null
  }).then((r) => r.memories);
}

interface GroupInfoReq {
  group: string;
}

export function groupInfo(group: string): Promise<GroupInfo> {
  return callTool<GroupInfoReq, GroupInfo>('group_info', { group });
}
