use std::collections::BTreeMap;
use std::io::Write;

use reqwest::{Client, Response};
use serde_json::{Value, from_str, json};

use crate::sven::backend::Backend;
use crate::sven::chat_history::{ChatHistory, MessageResponse, TokenUsage};
use crate::sven::config::ChatOptions;
use crate::sven::stats::{RunOutcome, RunStatus, StatsStore};
use crate::sven::term;
use crate::sven::tool::Tool;
use crate::sven::tool_registry::ToolRegistry;
use crate::sven::tools::notify_tool::NotifySendTool;

/// Maximum number of chat rounds with tool calls before the agent gives
/// up — a model stuck in a tool loop would otherwise run forever.
const MAX_TOOL_ROUNDS: usize = 250;

/// Maximum characters of a tool result kept in the conversation. Larger
/// outputs are truncated so a single tool (e.g. a full man page) cannot
/// flood the model's context window.
const MAX_TOOL_OUTPUT: usize = 10_000;

/// A run longer than this is notified with critical urgency: most
/// daemons (GNOME Shell, Notify OSD) keep critical notifications on
/// screen until dismissed instead of fading them out after a few
/// seconds.
const NOTIFY_CRITICAL_AFTER: std::time::Duration = std::time::Duration::from_secs(3 * 60);

#[derive(Default)]
pub struct StreamState {
    pub content: String,
    pub thinking: String,
    pub tool_calls: Vec<Value>,
    /// OpenAI streams each tool call as fragments keyed by `index`; they
    /// are merged here while the stream runs and moved into `tool_calls`
    /// by `Backend::finalize` once it ended. Ollama sends complete tool
    /// calls and never touches this.
    pub tool_call_fragments: BTreeMap<u64, Value>,
    pub is_thinking: bool,
    pub is_answering: bool,
    /// `finish_reason` of the final OpenAI chunk (`stop`, `length`,
    /// `tool_calls`, …). None while the stream is still running, and
    /// always None for Ollama, which signals the end via `done` instead.
    pub finish_reason: Option<String>,
    /// Token counts reported by the server for this response. Ollama
    /// sends them on the done chunk, OpenAI-compatible servers in a
    /// final `usage` chunk; servers that report nothing leave it at 0.
    pub usage: TokenUsage,
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

/// Human-readable token count: plain below 1,000, `1.5K` up to a million,
/// `1.2M` above. One decimal, with a trailing `.0` dropped (`2K`, not
/// `2.0K`) — the summary line stays short even for long tool-heavy runs.
pub(crate) fn format_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        let k = tokens as f64 / 1_000.0;
        let k = format!("{k:.1}");
        // 999_999 would otherwise round to "1000K"
        if k == "1000.0" {
            "1M".to_string()
        } else {
            format!("{}K", k.trim_end_matches(".0"))
        }
    } else {
        let m = tokens as f64 / 1_000_000.0;
        let m = format!("{m:.1}");
        format!("{}M", m.trim_end_matches(".0"))
    }
}

/// Human-readable duration for the run summary: one decimal below a
/// minute, whole minutes/seconds above (`1.5s`, `2m 3s`, `1h 4m`). The
/// seconds are rounded before the split so 59m59.9s carries into the
/// next minute instead of printing "59m 60s".
pub(crate) fn format_duration(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds < 60.0 {
        format!("{seconds:.1}s")
    } else {
        let total = seconds.round() as u64;
        let minutes = total / 60;
        let rest = total % 60;
        if minutes < 60 {
            format!("{minutes}m {rest}s")
        } else {
            format!("{}h {}m", minutes / 60, minutes % 60)
        }
    }
}

pub struct AgentConfig {
    pub host: String,
    pub model: String,
    pub system_prompt: String,
    pub options: ChatOptions,
    pub tool_registry: ToolRegistry,
    pub backend: Backend,
    /// Where `statistics.json` lives; the agent owns the store so every
    /// exit path of `run` records the run.
    pub data_dir: String,
    /// Sent as `Authorization: Bearer …`; only the OpenAI backend uses it.
    pub api_key: Option<String>,
}

pub struct Agent {
    client: Client,
    config: AgentConfig,
    history: ChatHistory,
    stats: StatsStore,
}

impl Agent {
    pub fn new(agent_config: AgentConfig) -> Agent {
        let stats = StatsStore::load(&agent_config.data_dir);
        Agent {
            client: Client::new(),
            history: ChatHistory::new(&agent_config.system_prompt),
            config: agent_config,
            stats,
        }
    }

    /// One user prompt → final answer. Wraps the round loop to accumulate
    /// the token usage of every round and measure the wall time until the
    /// run finishes; the summary is printed on every exit path (final
    /// answer, request error, round cap) and the run is recorded in
    /// `statistics.json`.
    pub async fn run(&mut self, message: &str) {
        let start = std::time::Instant::now();
        let mut usage = TokenUsage::default();
        let outcome = self.run_rounds(message, &mut usage).await;
        let elapsed = start.elapsed();
        let summary = format!(
            "run finished in {} — in {} out {} tokens ({} total)",
            format_duration(elapsed),
            format_tokens(usage.prompt_tokens),
            format_tokens(usage.completion_tokens),
            format_tokens(usage.total())
        );
        println!("{}", term::bold(&summary));
        self.stats.record_run(&usage, &outcome, elapsed);
        self.notify_finished(elapsed, &summary).await;
    }

    /// Print the overall statistics accumulated in `statistics.json`.
    pub fn print_stats(&self) {
        println!("{}", self.stats.summary());
    }

    /// Tell the user the run is over by reusing `NotifySendTool` — the
    /// same tool the model can call, so the argv layout, urgency
    /// validation and error handling are not duplicated here. The tool
    /// is a stateless unit struct, so calling it directly is equivalent
    /// to going through the registry. A failure (e.g. no notification
    /// daemon) is only reported on stderr: the run itself succeeded, and
    /// the summary was already printed to the terminal.
    async fn notify_finished(&self, elapsed: std::time::Duration, summary: &str) {
        let urgency = if elapsed > NOTIFY_CRITICAL_AFTER {
            "critical"
        } else {
            "normal"
        };
        if let Err(e) = NotifySendTool
            .execute(json!({
                "summary": "Sven: run finished",
                "body": summary,
                "urgency": urgency,
                "app_name": "sven"
            }))
            .await
        {
            eprintln!("could not send notification: {}", e);
        }
    }

    async fn run_rounds(&mut self, message: &str, usage: &mut TokenUsage) -> RunOutcome {
        self.history.user(message);
        let mut rounds = 0u64;
        let mut tool_calls = 0u64;
        for _round in 0..MAX_TOOL_ROUNDS {
            let url = &self.config.backend.endpoint(&self.config.host);
            let mut builder = self.client.post(url).json(&self.request_body());
            if self.config.backend.is_openai_compatible() {
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
                    return RunOutcome { rounds, tool_calls, status: RunStatus::Error };
                }
            };

            // `send()` returns Ok for 4xx/5xx too, and an error body is
            // not an SSE/NDJSON stream — without this check a wrong model
            // name or API key would make the agent silently do nothing.
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                self.history.pop_user();
                eprintln!("error: {} — {}", status, truncate(&body, 500));
                return RunOutcome { rounds, tool_calls, status: RunStatus::Error };
            }

            let message: MessageResponse = self.handle_chunks(&mut response).await;
            usage.add(&message.usage);
            self.history.assistant(&message);
            rounds += 1;
            if message.tool_calls.is_empty() {
                // `length` means the model hit the `max_tokens` cap —
                // common with reasoning models, whose thinking counts
                // against the cap. The answer is cut off mid-sentence
                // (or mid-thought, with no content at all), so the
                // truncation is reported instead of silently returning
                // to the prompt.
                if message.finish_reason.as_deref() == Some("length") {
                    println!(
                        "{}",
                        term::red(
                            "Response cut off by max_tokens (finish_reason: length). \
                             Raise `options.max_tokens` in the config."
                        )
                    );
                }
                return RunOutcome { rounds, tool_calls, status: RunStatus::Finished };
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
                tool_calls += 1;
                let result = self.process_tool_call(&tool_name, tool_params).await;
                self.history.tool(&result, &tool_name, tool_call_id.as_deref());
            }
        }

        let note = format!(
            "Stopped after the maximum of {} tool rounds without a final answer.",
            MAX_TOOL_ROUNDS
        );
        println!("{}", term::red(&note));
        self.history.assistant_note(&note);
        RunOutcome { rounds, tool_calls, status: RunStatus::RoundCap }
    }

    /// Build the chat request body for the configured backend. Ollama
    /// takes sampler options in an `options` envelope (`num_ctx` sets the
    /// context window); OpenAI-compatible servers (OpenAI, vLLM) take
    /// `temperature` and `max_tokens` (an output cap) at the top level
    /// and reject unknown fields like `options`.
    fn request_body(&self) -> Value {
        match &self.config.backend {
            Backend::Ollama => json!({
                "model": &self.config.model,
                "stream": true,
                "options": &self.config.options,
                "tools": self.config.tool_registry.tool_definitions(),
                "messages": self.history.get()
            }),
            Backend::OpenAI | Backend::Vllm => {
                let mut body = json!({
                    "model": &self.config.model,
                    "stream": true,
                    "temperature": self.config.options.temperature,
                    "tools": self.config.tool_registry.tool_definitions(),
                    "messages": self.history.get(),
                    "stream_options": {
                        "include_usage": true
                    }
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
            finish_reason: state.finish_reason,
            usage: state.usage,
        }
    }

    /// Execute one tool call and return the result for the history,
    /// truncated to `MAX_TOOL_OUTPUT` characters.
    pub async fn process_tool_call(&self, tool_name: &str, params: Value) -> String {
        let result = match self.config.tool_registry.get_tool(tool_name) {
            Some(tool) => {
                println!("\t🔧  {} {}", term::tool_color(&self.config.backend,tool_name), params);
                match tool.execute(params).await {
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

    #[test]
    fn formats_durations_readably() {
        assert_eq!(format_duration(std::time::Duration::from_millis(1500)), "1.5s");
        assert_eq!(format_duration(std::time::Duration::from_secs(59)), "59.0s");
        assert_eq!(format_duration(std::time::Duration::from_secs(123)), "2m 3s");
        assert_eq!(format_duration(std::time::Duration::from_secs(3840)), "1h 4m");
        // 59m59.9s carries into the next minute instead of "59m 60s"
        assert_eq!(
            format_duration(std::time::Duration::from_millis(3_599_900)),
            "1h 0m"
        );
    }

    #[test]
    fn formats_token_counts_readably() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1K");
        assert_eq!(format_tokens(1_523), "1.5K");
        assert_eq!(format_tokens(999_999), "1M");
        assert_eq!(format_tokens(1_000_000), "1M");
        assert_eq!(format_tokens(1_234_567), "1.2M");
    }

    #[test]
    fn token_usage_accumulates() {
        let mut total = TokenUsage::default();
        total.add(&TokenUsage { prompt_tokens: 100, completion_tokens: 20 });
        total.add(&TokenUsage { prompt_tokens: 50, completion_tokens: 5 });
        assert_eq!(total.prompt_tokens, 150);
        assert_eq!(total.completion_tokens, 25);
        assert_eq!(total.total(), 175);
    }
}
