//! The `Tool` wrapper that exposes one remote MCP tool to the model.

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

use crate::sven::mcp::{McpClient, McpToolInfo};
use crate::sven::tool::{Tool, ToolFuture};

/// The name under which the model sees a remote tool. The `mcp__` prefix
/// plus server and tool name keeps MCP tools from colliding with each
/// other or with sven's built-in tools.
pub fn mcp_tool_name(server: &str, tool: &str) -> String {
    format!("mcp__{}__{}", server, tool)
}

pub struct McpTool {
    server: String,
    tool: String,
    description: String,
    input_schema: Value,
    /// All tools of one server share a single connection. The agent runs
    /// tool calls one at a time, so the lock is never contended in
    /// practice; it exists because `Tool::execute` takes `&self`.
    ///
    /// Tokio's, not std's: the guard is held across the `.await`s of
    /// `call_tool`, and a std guard would make the future `!Send`.
    client: Arc<Mutex<McpClient>>,
}

impl McpTool {
    pub fn new(server: &str, info: &McpToolInfo, client: Arc<Mutex<McpClient>>) -> Self {
        Self {
            server: server.to_string(),
            tool: info.name.clone(),
            description: info.description.clone(),
            input_schema: info.input_schema.clone(),
            client,
        }
    }
}

impl Tool for McpTool {
    fn name(&self) -> String {
        mcp_tool_name(&self.server, &self.tool)
    }

    fn desc(&self) -> String {
        let description = if self.description.is_empty() {
            "(no description provided)".to_string()
        } else {
            self.description.clone()
        };
        format!(
            "MCP tool '{}' of server '{}': {}",
            self.tool, self.server, description
        )
    }

    /// The server's own `inputSchema`, passed through unchanged — it is
    /// already the JSON schema the backends expect.
    fn params(&self) -> Option<Value> {
        Some(self.input_schema.clone())
    }

    fn execute<'a>(&'a self, params: Value) -> ToolFuture<'a> {
        Box::pin(async move {
            // tokio's lock has no poisoning — a panic in another task
            // leaves the mutex usable
            let mut client = self.client.lock().await;
            client.call_tool(&self.tool, params).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sven::mcp::transport::{MockState, MockTransport};
    use serde_json::json;
    use std::collections::VecDeque;

    #[test]
    fn names_carry_the_mcp_prefix() {
        assert_eq!(mcp_tool_name("github", "create_issue"), "mcp__github__create_issue");
    }

    #[tokio::test]
    async fn exposes_the_remote_schema_and_calls_through() {
        let state = Arc::new(std::sync::Mutex::new(MockState {
            incoming: VecDeque::from(vec![
                json!({"jsonrpc": "2.0", "id": 1, "result": {"content": [{"type": "text", "text": "hello"}]}}),
            ]),
            ..Default::default()
        }));
        let client = McpClient::with_transport(
            "srv",
            Box::new(MockTransport {
                state: Arc::clone(&state),
            }),
        );
        let info = McpToolInfo {
            name: "echo".to_string(),
            description: "Echo a text".to_string(),
            input_schema: json!({"type": "object", "properties": {"text": {"type": "string"}}}),
        };
        let tool = McpTool::new("srv", &info, Arc::new(Mutex::new(client)));

        assert_eq!(tool.name(), "mcp__srv__echo");
        assert!(tool.desc().contains("Echo a text"), "{}", tool.desc());
        assert_eq!(
            tool.params(),
            Some(json!({"type": "object", "properties": {"text": {"type": "string"}}}))
        );
        assert_eq!(tool.execute(json!({"text": "hi"})).await.unwrap(), "hello");

        // the call reached the remote tool under its own name
        let sent = state.lock().unwrap().sent.clone();
        assert_eq!(sent[0]["method"], json!("tools/call"));
        assert_eq!(sent[0]["params"]["name"], json!("echo"));
        assert_eq!(sent[0]["params"]["arguments"], json!({"text": "hi"}));
    }
}