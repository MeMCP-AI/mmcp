//! `mmcp serve` implementation: MCP stdio server.
//!
//! Line-delimited JSON envelopes on stdin and stdout. Each request
//! carries a `ToolName` and a `request` value, each response echoes
//! the tool name and includes either the successful response or a
//! structured error. The full rmcp bidirectional protocol layering
//! lands in a future commit; this transport is stable enough to be
//! driven from a simple test harness today.

use std::io::BufRead;

use anyhow::{Context, Result};
use mmcp_proto::{
    ListMemoriesResponse, ProtoError, SearchMemoriesResponse, ToolName,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
#[allow(dead_code)] // NOTE: `request` is captured for forward compatibility; it will be read once the stdio handlers inspect arguments.
struct ToolEnvelope {
    tool: ToolName,
    #[serde(default)]
    request: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
enum ToolReply {
    Ok { tool: ToolName, response: Value },
    Error { tool: ToolName, error: ProtoError },
}

/// Run the stdio MCP server loop until stdin closes.
pub async fn run() -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let raw = line.context("reading MCP envelope from stdin")?;
        if raw.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<ToolEnvelope>(&raw) {
            Ok(envelope) => dispatch(envelope).await,
            Err(err) => ToolReply::Error {
                tool: ToolName::ListMemories,
                error: ProtoError::InvalidRequest(err.to_string()),
            },
        };
        write_line(&mut out, &reply)?;
    }
    Ok(())
}

fn write_line<W: std::io::Write>(out: &mut W, reply: &ToolReply) -> Result<()> {
    let text = serde_json::to_string(reply)?;
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

async fn dispatch(envelope: ToolEnvelope) -> ToolReply {
    // The local stdio server currently returns empty responses for
    // every tool it understands; eventually each arm will read
    // `envelope.request` and forward it to the local cache or the
    // server, but for now the tool name alone is enough to produce
    // the stub reply.
    let tool = envelope.tool;
    match tool {
        ToolName::ListMemories => {
            let value = serde_json::to_value(ListMemoriesResponse { memories: Vec::new() })
                .unwrap_or(Value::Null);
            ToolReply::Ok {
                tool,
                response: value,
            }
        }
        ToolName::SearchMemories => {
            let value = serde_json::to_value(SearchMemoriesResponse { hits: Vec::new() })
                .unwrap_or(Value::Null);
            ToolReply::Ok {
                tool,
                response: value,
            }
        }
        other => ToolReply::Error {
            tool: other,
            error: ProtoError::Internal(format!(
                "{} is not yet wired in the local stdio server",
                other.as_str()
            )),
        },
    }
}
