//! `SpawnAgentTool` — delegate one task to a fresh sub-agent.
//!
//! The tool captures the session's config (host, model, backend,
//! options, data dir, API key) and the live MCP connections at
//! registration time, so every sub-agent runs against the same server
//! the parent was configured for — reusing the config is the whole
//! point. A sub-agent starts with a clean context (only the system
//! prompt from the call and the task), gets every built-in tool plus
//! the session's MCP tools, but not `SpawnAgentTool` itself:
//! sub-agents cannot spawn agents, so a model cannot run away into
//! recursive spawning.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::sven::agent::{Agent, AgentConfig};
use crate::sven::config::SvenConfig;
use crate::sven::stats::{RunOutcome, RunStatus};
use crate::sven::tool::{Tool, ToolFuture, ToolOutput};
use crate::sven::tool_registry::ToolRegistry;
use crate::sven::tools::builtin_tools;
use crate::sven::tools::mcp_tool::McpTool;

#[derive(Deserialize, Debug, JsonSchema)]
struct SpawnAgentParams {
    /// The sub-agent's system prompt: its role, expertise and
    /// instructions. It replaces this agent's system prompt for the
    /// sub-agent. Empty reuses this agent's system prompt.
    system_prompt: String,
    /// The task for the sub-agent. Must be fully self-contained — the
    /// sub-agent sees neither this conversation nor any prior context,
    /// so include every detail it needs.
    user_prompt: String,
}

pub struct SpawnAgentTool {
    config: SvenConfig,
    api_key: Option<String>,
    mcp_tools: Vec<McpTool>,
}

impl SpawnAgentTool {
    /// Capture the session's config and MCP tools. Called once in
    /// `build_agent`, after the CLI overrides were applied — so a
    /// `--host`/`--model` flag reaches the sub-agents too.
    pub fn new(config: SvenConfig, api_key: Option<String>, mcp_tools: Vec<McpTool>) -> Self {
        Self { config, api_key, mcp_tools }
    }

    /// The registry a sub-agent gets: every built-in tool plus clones
    /// of the session's MCP tools (the connection lives behind the
    /// `Arc`, so parent and sub-agent share one client — and since the
    /// parent is blocked awaiting the spawn result while the sub-agent
    /// runs, the lock is never contended). `SpawnAgentTool` is
    /// deliberately absent, which is what keeps sub-agents from
    /// spawning further agents.
    fn sub_registry(&self) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        for tool in builtin_tools() {
            registry.register(tool);
        }
        for tool in &self.mcp_tools {
            registry.register(Box::new(tool.clone()));
        }
        registry
    }
}

/// Render a sub-agent's outcome as the tool result the spawning model
/// reads: the answer on success, the reason it failed otherwise. A
/// truncated or missing answer is noted, so the parent does not
/// mistake a cut-off text for the complete result.
fn render_outcome(outcome: &RunOutcome) -> Result<String, String> {
    match outcome.status {
        RunStatus::Finished => {
            if outcome.answer.trim().is_empty() {
                Ok("The sub-agent finished without producing an answer text \
                    (its entire response was thinking)."
                    .to_string())
            } else if outcome.answer_truncated {
                Ok(format!(
                    "{}\n[the sub-agent's answer was cut off by the output \
                     token cap — re-run it with a narrower task]",
                    outcome.answer
                ))
            } else {
                Ok(outcome.answer.clone())
            }
        }
        RunStatus::RoundCap => Err(
            "The sub-agent stopped after the maximum number of tool rounds \
             without a final answer — narrow the task or solve it directly."
                .to_string(),
        ),
        RunStatus::Error => Err(outcome
            .error
            .clone()
            .unwrap_or_else(|| "unknown error".to_string())),
    }
}

impl Tool for SpawnAgentTool {
    fn name(&self) -> String {
        "SpawnAgentTool".to_string()
    }

    fn desc(&self) -> String {
        "Spawn a sub-agent: a fresh agent with its own clean context that \
         runs one task to completion and returns its final answer as this \
         tool's result. Give it a system_prompt (its role and instructions) \
         and a fully self-contained user_prompt — the sub-agent sees \
         nothing of this conversation, so include every detail it needs. \
         It reuses this agent's config (model, host, options) and has the \
         same tools, except it cannot spawn further agents. Use it to \
         delegate self-contained subtasks whose back-and-forth would \
         bloat this context; the sub-agent's work is streamed to the \
         terminal while it runs."
            .to_string()
    }

    fn params(&self) -> Option<Value> {
        let mut schema = schemars::schema_for!(SpawnAgentParams);
        schema.remove("$schema");
        Some(serde_json::json!(schema))
    }

    fn execute<'a>(&'a self, params: Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: SpawnAgentParams = serde_json::from_value(params)?;
            if args.user_prompt.trim().is_empty() {
                return Err("user_prompt must not be empty".into());
            }
            // An empty system_prompt reuses the session's — the config
            // is reused wholesale, and a caller that only wants a task
            // done gets the same persona the parent runs under.
            let system_prompt = if args.system_prompt.trim().is_empty() {
                self.config.system_prompt.clone()
            } else {
                args.system_prompt
            };
            let mut agent = Agent::new(AgentConfig {
                host: self.config.host.clone(),
                model: self.config.model.clone(),
                system_prompt,
                options: self.config.options.clone(),
                tool_registry: self.sub_registry(),
                backend: self.config.backend.clone(),
                data_dir: self.config.data_dir.clone(),
                api_key: self.api_key.clone(),
                max_tool_calls: self.config.max_tool_calls,
                subagent: true,
            });
            let outcome = agent.run(&args.user_prompt).await;
            render_outcome(&outcome).map(ToolOutput::text).map_err(Into::into)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sven::mcp::transport::{MockState, MockTransport};
    use crate::sven::mcp::{McpClient, McpToolInfo};
    use serde_json::json;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    fn tool_with_host(host: &str, mcp_tools: Vec<McpTool>) -> SpawnAgentTool {
        SpawnAgentTool::new(
            SvenConfig {
                host: host.to_string(),
                ..SvenConfig::default()
            },
            None,
            mcp_tools,
        )
    }

    fn registry_names(registry: &ToolRegistry) -> Vec<String> {
        registry
            .tool_definitions()
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn params_schema_lists_both_prompts_as_required() {
        let tool = tool_with_host("http://localhost:11434", Vec::new());
        let schema = tool.params().unwrap();
        let properties = schema["properties"].as_object().unwrap();
        assert!(properties.contains_key("system_prompt"));
        assert!(properties.contains_key("user_prompt"));
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("system_prompt")));
        assert!(required.contains(&json!("user_prompt")));
    }

    #[test]
    fn sub_agents_get_the_builtin_tools_but_cannot_spawn_agents() {
        let tool = tool_with_host("http://localhost:11434", Vec::new());
        let names = registry_names(&tool.sub_registry());
        // the built-in tools are there …
        assert!(names.contains(&"ReadTool".to_string()));
        assert!(names.contains(&"TimeTool".to_string()));
        // … but the spawn tool is not — the recursion guard
        assert!(
            !names.contains(&"SpawnAgentTool".to_string()),
            "{:?}",
            names
        );
    }

    #[test]
    fn sub_agents_share_the_sessions_mcp_tools() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let client = McpClient::with_transport(
            "srv",
            Box::new(MockTransport { state: Arc::clone(&state) }),
        );
        let info = McpToolInfo {
            name: "echo".to_string(),
            description: "Echo a text".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
        };
        let mcp_tool = McpTool::new("srv", &info, Arc::new(tokio::sync::Mutex::new(client)));
        let tool = tool_with_host("http://localhost:11434", vec![mcp_tool]);
        let names = registry_names(&tool.sub_registry());
        assert!(names.contains(&"mcp__srv__echo".to_string()), "{:?}", names);
    }

    #[test]
    fn renders_finished_answers_verbatim() {
        let outcome = RunOutcome {
            rounds: 3,
            tool_calls: 2,
            status: RunStatus::Finished,
            answer: "42".to_string(),
            answer_truncated: false,
            error: None,
        };
        assert_eq!(render_outcome(&outcome).unwrap(), "42");
    }

    #[test]
    fn notes_truncated_and_missing_answers() {
        let truncated = RunOutcome {
            rounds: 1,
            tool_calls: 0,
            status: RunStatus::Finished,
            answer: "partial…".to_string(),
            answer_truncated: true,
            error: None,
        };
        let rendered = render_outcome(&truncated).unwrap();
        assert!(rendered.starts_with("partial…"), "{}", rendered);
        assert!(rendered.contains("cut off"), "{}", rendered);

        let thinking_only = RunOutcome {
            answer: String::new(),
            ..truncated
        };
        assert!(render_outcome(&thinking_only).unwrap().contains("without producing"));
    }

    #[test]
    fn errors_and_round_caps_become_tool_errors() {
        let error = RunOutcome::error(0, 0, "connection refused".to_string());
        assert_eq!(render_outcome(&error).unwrap_err(), "connection refused");

        let round_cap = RunOutcome {
            rounds: 250,
            tool_calls: 250,
            status: RunStatus::RoundCap,
            answer: String::new(),
            answer_truncated: false,
            error: None,
        };
        assert!(render_outcome(&round_cap)
            .unwrap_err()
            .contains("maximum number of tool rounds"));
    }

    /// A one-shot HTTP server: accepts one request, captures its body,
    /// answers with `raw` (a full HTTP response, status line included).
    /// Returns the host URL and the captured request body.
    fn fake_server(raw: String) -> (String, Arc<Mutex<Option<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured_clone = Arc::clone(&captured);
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let request = read_http_request(&mut stream);
            *captured_clone.lock().unwrap() = Some(request);
            let _ = stream.write_all(raw.as_bytes());
        });
        (format!("http://{}", addr), captured)
    }

    /// Read one HTTP request: the headers, then `Content-Length` bytes
    /// of body. Byte-wise until the header terminator — requests are
    /// small, and this needs no buffering subtleties.
    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut buf: Vec<u8> = Vec::new();
        let mut byte = [0u8; 1];
        while !buf.ends_with(b"\r\n\r\n") {
            match stream.read(&mut byte) {
                Ok(0) | Err(_) => return String::new(),
                Ok(_) => buf.push(byte[0]),
            }
        }
        let headers = String::from_utf8_lossy(&buf).to_ascii_lowercase();
        let content_length = headers
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            let _ = stream.read_exact(&mut body);
        }
        String::from_utf8_lossy(&body).to_string()
    }

    /// A one-shot fake Ollama server answering with one NDJSON line.
    fn fake_ollama(line: &str) -> (String, Arc<Mutex<Option<String>>>) {
        let body = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}\n",
            line.len() + 1,
            line
        );
        fake_server(body)
    }

    fn temp_data_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sven-spawn-{}-{}", std::process::id(), label));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn spawns_a_sub_agent_against_the_configured_host() {
        let (host, captured) = fake_ollama(
            r#"{"message":{"content":"sub-agent answer"},"done":true,"prompt_eval_count":10,"eval_count":5}"#,
        );
        let dir = temp_data_dir("e2e");
        let tool = SpawnAgentTool::new(
            SvenConfig {
                host,
                data_dir: dir.to_string_lossy().into_owned(),
                ..SvenConfig::default()
            },
            None,
            Vec::new(),
        );

        let output = tool
            .execute(json!({
                "system_prompt": "You are a code reviewer.",
                "user_prompt": "find the bug"
            }))
            .await
            .unwrap();

        assert_eq!(output.text, "sub-agent answer");
        assert!(output.attachments.is_empty());

        // the request went to the configured host and carried both
        // prompts — the system one with the date the agent appends
        let request = captured.lock().unwrap().clone().unwrap();
        assert!(request.contains("You are a code reviewer."), "{}", request);
        assert!(request.contains("find the bug"), "{}", request);

        // the sub-agent's run was recorded in the shared data dir
        let stats = std::fs::read_to_string(dir.join("statistics.json")).unwrap();
        assert!(stats.contains("\"runs\": 1"), "{}", stats);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_failed_sub_agent_run_is_a_tool_error() {
        let (host, _captured) = fake_server(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_string(),
        );
        let tool = tool_with_host(&host, Vec::new());
        let error = tool
            .execute(json!({
                "system_prompt": "s",
                "user_prompt": "do something"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("500"), "{}", error);
    }

    #[tokio::test]
    async fn empty_system_prompt_reuses_the_configured_one() {
        let (host, captured) = fake_ollama(
            r#"{"message":{"content":"ok"},"done":true,"prompt_eval_count":1,"eval_count":1}"#,
        );
        let tool = SpawnAgentTool::new(
            SvenConfig {
                host,
                system_prompt: "You are the session persona.".to_string(),
                ..SvenConfig::default()
            },
            None,
            Vec::new(),
        );
        tool.execute(json!({"system_prompt": "", "user_prompt": "task"}))
            .await
            .unwrap();
        let request = captured.lock().unwrap().clone().unwrap();
        assert!(
            request.contains("You are the session persona."),
            "{}",
            request
        );
    }

    #[tokio::test]
    async fn empty_user_prompt_is_rejected_before_any_request() {
        let tool = tool_with_host("http://localhost:11434", Vec::new());
        let error = tool
            .execute(json!({"system_prompt": "s", "user_prompt": "   "}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("must not be empty"), "{}", error);
    }
}
