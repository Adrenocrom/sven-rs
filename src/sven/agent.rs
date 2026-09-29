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
    pub is_thinking: bool,
    pub is_answering: bool,
}

//impl StreamState {
//    fn process_json(&mut self, json: &Value) {
//        if let Some(thinking_chunk) = json["message"]["thinking"].as_str() {
//            if !thinking_chunk.is_empty() {
//                if !self.is_thinking {
//                    self.is_thinking = true;
//                    print!("{}", term::thinking());
//                }
//                print!("{}", thinking_chunk);
//            }
//        } else if self.is_thinking {
//            self.is_thinking = false;
//            if term::enabled() {
//                println!("{}\n", term::reset());
//            } else {
//                println!();
//            }
//        }
//
//        if let Some(content_chunk) = json["message"]["content"].as_str() {
//            if !content_chunk.is_empty() {
//                self.is_answering = true;
//                self.content.push_str(content_chunk);
//                print!("{}", content_chunk);
//            }
//        } else if self.is_answering {
//            self.is_answering = false;
//            println!("\n");
//        }
//
//        if json["done"].as_bool() == Some(true) && self.is_answering {
//            print!("\n");
//        }
//
//        if
//            let Some(eval_count) = json["eval_count"].as_u64() &&
//            let Some(prompt_eval_count) = json["prompt_eval_count"].as_u64()
//        {
//            println!("\n{}", term::bold(&format!("in {} out {}", prompt_eval_count, eval_count)));
//            println!();
//        }
//
//        if let Some(tcs) = json["message"]["tool_calls"].as_array() {
//            for tc in tcs {
//                self.tool_calls.push(tc.clone());
//            }
//        }
//    }
//}

/// Extract (name, arguments) from a tool-call payload. Local models
/// regularly emit malformed calls — missing name, non-string name, wrong
/// shape — so every access is checked and problems are reported as a
/// string instead of panicking.
fn parse_tool_call(tool_call: &Value) -> Result<(String, Value), String> {
    let function = tool_call
        .get("function")
        .ok_or_else(|| "missing 'function' object".to_string())?;
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing or non-string 'name'".to_string())?;
    let arguments = function.get("arguments").cloned().unwrap_or(Value::Null);
    Ok((name.to_string(), arguments))
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
    pub backend: Backend
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
            let builder = self.client.post(url).json(&json!({
                "model": &self.config.model,
                "stream": true,
                "options": &self.config.options,
                "tools": self.config.tool_registry.tool_definitions(),
                "messages": self.history.get()
            }));
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
                let (tool_name, tool_params) = match parse_tool_call(&tool_call) {
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
                        self.history.tool(&error, "malformed", None);
                        continue;
                    }
                };
                let result = self.process_tool_call(&tool_name, tool_params);
                self.history.tool(&result, &tool_name, None);
            }
        }

        let note = format!(
            "Stopped after the maximum of {} tool rounds without a final answer.",
            MAX_TOOL_ROUNDS
        );
        println!("{}", term::red(&note));
        self.history.assistant_note(&note);
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

                if let Err(e) = self.config.backend.process_line(&mut state, &line) {
                    eprintln!("... couldn't decode JSON: {}", e);
                    eprintln!("... skipping line {:?}", line);
                }
            }

            let _ = std::io::stdout().flush();
        }

        if !buffer.is_empty() {
            let line = String::from_utf8_lossy(&buffer);
            let line = line.trim();
            if !line.is_empty() {
                match from_str::<Value>(line) {
                    Ok(json) => self.config.backend.process_json(&mut state, &json),
                    Err(e) => eprintln!("... couldn't decode JSON: {}", e),
                }
            }
        }

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
        let (name, args) = parse_tool_call(&json!({
            "function": {"name": "ReadTool", "arguments": {"path": "src/main.rs"}}
        }))
        .unwrap();
        assert_eq!(name, "ReadTool");
        assert_eq!(args["path"], "src/main.rs");
    }

    #[test]
    fn reports_malformed_tool_calls_instead_of_panicking() {
        assert!(parse_tool_call(&json!({})).is_err());
        assert!(parse_tool_call(&json!({"function": {}})).is_err());
        assert!(parse_tool_call(&json!({"function": {"name": 42}})).is_err());
        assert!(parse_tool_call(&json!({"function": {"arguments": {}}})).is_err());
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
