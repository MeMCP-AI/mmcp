# claude.ai MCP Servers Issue Log

## 2026-04-16 — Mass disconnection of cloud MCP servers

**Affected servers**:
- `claude.ai Gmail` (authenticate, complete_authentication)
- `claude.ai Google Calendar` (authenticate, complete_authentication)
- `claude.ai Hugging Face Gradio` (all 14 tools: dynamic_space, gr1_z_image_turbo_generate, hf_doc_fetch, hf_doc_search, hf_hub_query, hf_whoami, hub_repo_details, hub_repo_search, paper_search, space_search)
- `claude.ai Virtual World AI` (authenticate, complete_authentication)
- `claude.ai Vwait` (authenticate, complete_authentication)

**Error**: System notification: "The following deferred tools are no longer available (their MCP server disconnected)."

**Context**: Mid-session disconnection. No explicit trigger — likely the user's PC went to sleep and the websocket connections dropped.

**Impact**: None for this project (mmcp doesn't use these servers). HF Gradio instructions no longer apply.

**Status**: Not reconnected. Would require session restart or MCP server re-initialization.
