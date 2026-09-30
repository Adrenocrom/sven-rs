use std::collections::BTreeMap;
use std::io::Write;

use reqwest::{Client, Response};
use serde_json::{Value, from_str, json};

use crate::sven::backend::Backend;
use crate::sven::chat_history::{ChatHistory, MessageResponse};
use crate::sven::config::ChatOptions;
use crate::sven::term;
use crate::sven::tool_registry::ToolRegistry;

/// Maximum number of chat rounds with tool calls before the agent gives
/// up — a model stuck in a tool loop would otherwise run forever.
const MAX_TOOL_ROUNDS: usize = 250;

/// Maximum characters of a tool result kept in the conversation. Larger
/// outputs are truncated so a single tool (e.g. a full man page) cannot
/// flood the model's context window.
const MAX_TOOL_OUTPUT: usize = 10_000;

#[derive(Default)]
pub struct StreamState {
    pub content: String,
    pub tool_calls: Vec<Value>,
    /// OpenAI streams each tool call as fragments keyed by `index`; they
    /// are merged here while the stream runs and moved into `tool_calls`
    /// by `Backend::finalize` once it ended. Ollama sends complete tool
    /// calls and never touches this.
    pub tool_call_fragments: BTreeMap<u64, Value>,
    pub is_thinking: bool,
    pub is_answering: bool,
}

/// Extract (name, arguments, id) from a tool-call payload. Local models
/// regularly emit malformed calls — missing name, non-string name, wrong
/// shape — so every access is checked and problems are reported as a
/// string instead of panicking.
///
/// `arguments` differs per backend: Ollama sends a JSON object, OpenAI a
/// JSON-encoded *string* (which may be empty). Both are normalized to a
/// Value here so `Tool::execute` always receives an object.
pub(crate) fn parse_tool_call(tool_call: &Value) -> Result<(String, Value, Option<String>), String> {
    let function = tool_call
        .get("function")
        .ok_or_else(|| "missing 'function' object".to_string())?;
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing or non-string 'name'".to_string())?;
    let arguments = match function.get("arguments") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(encoded)) => {
            if encoded.trim().is_empty() {
                json!({})
            } else {
                from_str::<Value>(encoded)
                    .map_err(|e| format!("invalid JSON in 'arguments': {}", e))?
            }
        }
        Some(arguments) => arguments.clone(),
    };
    let id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok((name.to_string(), arguments, id))
}

/// Cap `output` at `max` characters (not bytes — the cut must not split a
/// character), marking the cut so the model knows it was shortened.
fn truncate(output: &str, max: usize) -> String {
    if output.chars().count() <= max {
        return output.to_string();
    }
    let mut truncated: String = output.chars().take(max).collect();
    truncated.push_str("\n… [output truncated]");
    truncated
}

pub struct AgentConfig {
    pub host: String,
    pub model: String,
    pub system_prompt: String,
    pub options: ChatOptions,
    pub tool_registry: ToolRegistry,
    pub backend: Backend,
    /// Sent as `Authorization: Bearer …`; only the OpenAI backend uses it.
    pub api_key: Option<String>,
}

pub struct Agent {
    client: Client,
    config: AgentConfig,
    history: ChatHistory,
}

impl Agent {
    pub fn new(agent_config: AgentConfig) -> Agent {
        Agent {
            client: Client::new(),
            history: ChatHistory::new(&agent_config.system_prompt),
            config: agent_config,
        }
    }

    pub async fn run(&mut self, message: &str) {
        self.history.user(message);
        println!("");
        for _round in 0..MAX_TOOL_ROUNDS {
            let url = &self.config.backend.endpoint(&self.config.host);
            let mut builder = self.client.post(url).json(&self.request_body());
            if self.config.backend == Backend::OpenAI {
                if let Some(api_key) = &self.config.api_key {
                    builder = builder.bearer_auth(api_key);
                }
            }
            let mut response = match builder.send().await {
                Ok(r) => r,
                Err(e) => {
                    // drop the dangling user message so the next turn
                    // doesn't start with an unanswered prompt
                    self.history.pop_user();
                    eprintln!("error: {}", e);
                    return;
                }
            };

            let message: MessageResponse = self.handle_chunks(&mut response).await;
            self.history.assistant(&message);
            if message.tool_calls.is_empty() {
                return;
            }
            for tool_call in message.tool_calls {
                let (tool_name, tool_params, tool_call_id) = match parse_tool_call(&tool_call) {
                    Ok(parsed) => parsed,
                    Err(reason) => {
                        // a malformed call becomes a tool result describing
                        // the problem, so the model can correct itself
                        // instead of the agent crashing
                        let error = format!(
                            "Error: malformed tool call: {} in {}",
                            reason,
                            truncate(&tool_call.to_string(), 500)
                        );
                        println!("    {}", term::red(&error));
                        // keep the id when the fragment carried one, so
                        // the result can still be matched to its call
                        let id = tool_call.get("id").and_then(Value::as_str);
                        self.history.tool(&error, "malformed", id);
                        continue;
                    }
                };
                let result = self.process_tool_call(&tool_name, tool_params);
                self.history.tool(&result, &tool_name, tool_call_id.as_deref());
            }
        }

        let note = format!(
            "Stopped after the maximum of {} tool rounds without a final answer.",
            MAX_TOOL_ROUNDS
        );
        println!("{}", term::red(&note));
        self.history.assistant_note(&note);
    }

    /// Build the chat request body for the configured backend. Ollama
    /// takes sampler options in an `options` envelope (`num_ctx` sets the
    /// context window); OpenAI-compatible servers take `temperature` and
    /// `max_tokens` (an output cap) at the top level and reject unknown
    /// fields like `options`.
    fn request_body(&self) -> Value {
        match &self.config.backend {
            Backend::Ollama => json!({
                "model": &self.config.model,
                "stream": true,
                "options": &self.config.options,
                "tools": self.config.tool_registry.tool_definitions(),
                "messages": self.history.get()
            }),
            Backend::OpenAI => {
                let mut body = json!({
                    "model": &self.config.model,
                    "stream": true,
                    "temperature": self.config.options.temperature,
                    "tools": self.config.tool_registry.tool_definitions(),
                    "messages": self.history.get()
                });
                if let Some(max_tokens) = self.config.options.max_tokens {
                    body["max_tokens"] = json!(max_tokens);
                }
                body
            }
        }
    }

    async fn handle_chunks(&self, response: &mut Response) -> MessageResponse {
        let mut state = StreamState::default();
        let mut buffer: Vec<u8> = Vec::new();

        loop {
            let bytes = match response.chunk().await {
                Ok(Some(b)) => b,
                Ok(None) => break,
                Err(e) => {
                    eprintln!("stream error: {}", e);
                    break;
                }
            };

            buffer.extend_from_slice(&bytes);
            while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line[..pos]);
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                if let Err(e) = self.config.backend.process_line(&mut state, line) {
                    eprintln!("... couldn't decode JSON: {}", e);
                    eprintln!("... skipping line {:?}", line);
                }
            }

            let _ = std::io::stdout().flush();
        }

        // a stream can end without a trailing newline; whatever is left
        // in the buffer is still a complete line
        if !buffer.is_empty() {
            let line = String::from_utf8_lossy(&buffer);
            let line = line.trim();
            if !line.is_empty() {
                if let Err(e) = self.config.backend.process_line(&mut state, line) {
                    eprintln!("... couldn't decode JSON: {}", e);
                    eprintln!("... skipping line {:?}", line);
                }
            }
        }

        // OpenAI tool calls are only complete now that every fragment
        // arrived; Ollama calls were already complete
        self.config.backend.finalize(&mut state);

        MessageResponse {
            content: state.content,
            tool_calls: state.tool_calls,
        }
    }

    /// Execute one tool call and return the result for the history,
    /// truncated to `MAX_TOOL_OUTPUT` characters.
    pub fn process_tool_call(&self, tool_name: &str, params: Value) -> String {
        let result = match self.config.tool_registry.get_tool(tool_name) {
            Some(tool) => {
                println!("\t🔧  {} {}\n", term::green(tool_name), params);
                match tool.execute(params) {
                    Ok(result) => result,
                    Err(err) => {
                        println!("    {}", term::red(&format!("ERROR: {}", err)));
                        err.to_string()
                    }
                }
            }
            None => format!("Error: Tool '{}' not found in registry", tool_name),
        };
        truncate(&result, MAX_TOOL_OUTPUT)
    }

    pub fn clear(&mut self) {
        self.history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_well_formed_tool_calls() {
        let (name, args, id) = parse_tool_call(&json!({
            "function": {"name": "ReadTool", "arguments": {"path": "src/main.rs"}}
        }))
        .unwrap();
        assert_eq!(name, "ReadTool");
        assert_eq!(args["path"], "src/main.rs");
        assert_eq!(id, None);
    }

    #[test]
    fn parses_openai_tool_calls_with_string_arguments() {
        let (name, args, id) = parse_tool_call(&json!({
            "id": "call_1",
            "type": "function",
            "function": {"name": "ReadTool", "arguments": "{\"path\": \"src/main.rs\"}"}
        }))
        .unwrap();
        assert_eq!(name, "ReadTool");
        assert_eq!(args["path"], "src/main.rs");
        assert_eq!(id.as_deref(), Some("call_1"));
    }

    #[test]
    fn empty_openai_arguments_decode_to_an_empty_object() {
        let (_, args, _) = parse_tool_call(&json!({
            "function": {"name": "TimeTool", "arguments": ""}
        }))
        .unwrap();
        assert_eq!(args, json!({}));
    }

    #[test]
    fn reports_malformed_tool_calls_instead_of_panicking() {
        assert!(parse_tool_call(&json!({})).is_err());
        assert!(parse_tool_call(&json!({"function": {}})).is_err());
        assert!(parse_tool_call(&json!({"function": {"name": 42}})).is_err());
        assert!(parse_tool_call(&json!({"function": {"arguments": {}}})).is_err());
        assert!(parse_tool_call(&json!({"function": {"name": "X", "arguments": "not json"}})).is_err());
    }

    #[test]
    fn truncates_long_outputs() {
        let long = "x".repeat(MAX_TOOL_OUTPUT + 100);
        let truncated = truncate(&long, MAX_TOOL_OUTPUT);
        assert!(truncated.chars().count() > MAX_TOOL_OUTPUT);
        assert!(truncated.ends_with("… [output truncated]"));
        assert_eq!(truncate("short", MAX_TOOL_OUTPUT), "short");
    }
}