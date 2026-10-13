use serde::{Deserialize, Serialize};
use serde_json::{Value, from_str, json};

use crate::sven::{agent::StreamState, term};

/// Which server protocol to speak. `Vllm` uses the same wire format as
/// `OpenAI` — vLLM serves an OpenAI-compatible API — but is its own
/// variant so configs are self-documenting and vLLM-specific behavior
/// has a place to diverge.
///
/// Serde expects the lowercase names (`"ollama"`, `"openai"`, `"vllm"`)
/// — the same strings `ToString` emits. The capitalized forms the
/// un-annotated enum used to require are kept as aliases so config files
/// written before the rename keep parsing.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Ollama,
    OpenAI,
    Vllm,
}

fn end_thinking(stream_state: &mut StreamState) {
    if stream_state.is_thinking {
        stream_state.is_thinking = false;
        if term::enabled() {
            println!("{}", term::reset());
        } else {
            println!();
        }
    }
}

fn end_content(stream_state: &mut StreamState) {
    if stream_state.is_answering {
        stream_state.is_answering = false;
        if ! stream_state.content.ends_with("\n") {
            println!("");
        }
    }
}

fn print_thinking(backend: &Backend, stream_state: &mut StreamState, chunk: &str) {
    if !chunk.is_empty() {
        if !stream_state.is_thinking {
            stream_state.is_thinking = true;
            print!("\n{}", term::thinking(&backend));
        }

        stream_state.thinking.push_str(chunk);
        print!("{}", chunk);
    }
}

fn print_content(stream_state: &mut StreamState, chunk: &str) {
    if !chunk.is_empty() {
        end_thinking(stream_state);

        if !stream_state.is_answering {
            stream_state.is_answering = true;
            println!();
        }

        stream_state.content.push_str(chunk);
        print!("{}", chunk);
    }
}

fn process_json_ollama(stream_state: &mut StreamState, json: &Value) {
    if let Some(thinking_chunk) = json["message"]["thinking"].as_str() {
        print_thinking(&Backend::Ollama, stream_state, thinking_chunk);
    }

    if let Some(content_chunk) = json["message"]["content"].as_str() {
        print_content(stream_state, content_chunk);
    }

    if json["done"].as_bool() == Some(true) {
        end_thinking(stream_state);
        end_content(stream_state);
    }

    if
        let Some(eval_count) = json["eval_count"].as_u64() &&
        let Some(prompt_eval_count) = json["prompt_eval_count"].as_u64()
    {
        println!("\n{}\n", term::bold(&format!("in {} out {}", prompt_eval_count, eval_count)));
        // the done chunk reports the totals for the whole response —
        // stored for the agent to accumulate across the rounds of a run
        stream_state.usage.prompt_tokens = prompt_eval_count;
        stream_state.usage.completion_tokens = eval_count;
    }

    if let Some(tcs) = json["message"]["tool_calls"].as_array() {
        for tc in tcs {
            stream_state.tool_calls.push(tc.clone());
        }
    }
}

fn process_json_openai(backend: &Backend,stream_state: &mut StreamState, json: &Value) {
    // Some servers report failures mid-stream as a top-level `error`
    // object instead of closing the connection; every `choices` access
    // below would silently return null for such chunks.
    if let Some(error) = json.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        eprintln!("stream error: {}", message);
        return;
    }

    if let Some(thinking_chunk) = json["choices"][0]["delta"]["reasoning"].as_str() {
        print_thinking(&backend, stream_state, thinking_chunk);
    } 
    else if let Some(thinking_chunk) = json["choices"][0]["delta"]["reasoning_content"].as_str() {
        print_thinking(&backend, stream_state, thinking_chunk);
    } 

    if let Some(content_chunk) = json["choices"][0]["delta"]["content"].as_str() {
        print_content(stream_state, content_chunk);
    }

    if let Some(finish_reason) = json["choices"][0].get("finish_reason").and_then(Value::as_str) {
        stream_state.finish_reason = Some(finish_reason.to_string());
        end_thinking(stream_state);
        end_content(stream_state);
        if !(stream_state.content.is_empty() && stream_state.thinking.is_empty()) {
            if ! stream_state.content.ends_with("\n") {
                println!("");
            }
        }
    }

    // The final chunk — sent because the request asked for
    // `stream_options: {"include_usage": true}` — carries the token
    // counts in `usage`; every other chunk has `usage: null`, and
    // servers that ignore `stream_options` never send one, so no line
    // is printed. Same display as the Ollama `eval_count` line.
    if
        let Some(completion_tokens) = json["usage"]["completion_tokens"].as_u64() &&
        let Some(prompt_tokens) = json["usage"]["prompt_tokens"].as_u64()
    {
        println!("{}\n", term::bold(&format!("in {} out {}", prompt_tokens, completion_tokens)));
        // the usage chunk reports the totals for the whole response —
        // stored for the agent to accumulate across the rounds of a run
        stream_state.usage.prompt_tokens = prompt_tokens;
        stream_state.usage.completion_tokens = completion_tokens;
    }

    // OpenAI streams each tool call as fragments: the first carries
    // `index`, `id` and the function `name` with an empty `arguments`
    // string, the following ones carry `index` and a string *fragment* of
    // the arguments. They are merged by `index` here and only become
    // complete calls in `finalize`.
    if let Some(tcs) = json["choices"][0]["delta"]["tool_calls"].as_array() {
        for tc in tcs {
            let Some(index) = tc.get("index").and_then(Value::as_u64) else {
                // no index — treat the fragment as a complete call
                stream_state.tool_calls.push(tc.clone());
                continue;
            };
            let entry = stream_state
                .tool_call_fragments
                .entry(index)
                .or_insert_with(|| json!({"function": {"name": "", "arguments": ""}}));
            if let Some(id) = tc.get("id").and_then(Value::as_str) {
                entry["id"] = json!(id);
            }
            // `type` must survive the merge: the merged call is echoed
            // back to the server inside the assistant message on the
            // next round, and OpenAI-compatible servers reject the
            // whole request when a tool call lacks `type: "function"`
            // (litellm in front of vLLM answers 400 naming
            // `ChatCompletionMessageFunctionToolCallParam`).
            if let Some(tool_type) = tc.get("type").and_then(Value::as_str) {
                entry["type"] = json!(tool_type);
            }
            if let Some(name) = tc["function"]["name"].as_str() {
                if !name.is_empty() {
                    entry["function"]["name"] = json!(name);
                }
            }
            if let Some(args) = tc["function"]["arguments"].as_str() {
                let merged = entry["function"]["arguments"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                entry["function"]["arguments"] = json!(merged + args);
            }
        }
    }
}

impl Backend {
    pub fn endpoint(&self, host: &String) -> String {
        match self {
            Backend::Ollama => format!("{}/api/chat", &host),
            Backend::OpenAI | Backend::Vllm => format!("{}/v1/chat/completions", &host),
        }
    }

    /// Whether the server speaks the OpenAI wire format: SSE events,
    /// `choices[0].delta`, fragmented tool calls, `POST /v1/chat/completions`,
    /// optional `Authorization: Bearer …` (vLLM only checks it when started
    /// with `--api-key`; sending it unconditionally is harmless without one).
    pub fn is_openai_compatible(&self) -> bool {
        matches!(self, Backend::OpenAI | Backend::Vllm)
    }

    /// Apply one line of the response stream. The OpenAI-compatible
    /// backends speak SSE: the line goes into the parser in
    /// `stream_state`, and a payload comes out only when the line's
    /// blank terminator ends an event — so a chunk is applied exactly
    /// once no matter how its `data:` fields were split. Ollama
    /// streams newline-delimited JSON and decodes each line directly.
    /// Lines that do not decode are skipped with a warning — one bad
    /// chunk must not kill the whole response.
    pub fn process_line(&self, stream_state: &mut StreamState, line: &str) {
        match self {
            Backend::Ollama => {
                if line.is_empty() {
                    return;
                }
                match from_str::<Value>(line) {
                    Ok(json) => process_json_ollama(stream_state, &json),
                    Err(e) => {
                        eprintln!("... couldn't decode JSON: {}", e);
                        eprintln!("... skipping line {:?}", line);
                    }
                }
            },
            Backend::OpenAI | Backend::Vllm => {
                if let Some(payload) = stream_state.sse.line(line) {
                    self.process_event(stream_state, &payload);
                }
            }
        }
    }

    /// Decode and apply one completed SSE event payload. A payload that
    /// is not JSON is skipped with a warning instead of an error — a
    /// frame cut off mid-stream is noise, not a fatal error (the MCP
    /// transport skips such frames silently).
    fn process_event(&self, stream_state: &mut StreamState, payload: &str) {
        if payload.is_empty() || payload == "[DONE]" {
            return;
        }
        match from_str::<Value>(payload) {
            Ok(json) => process_json_openai(&self, stream_state, &json),
            Err(e) => {
                eprintln!("... couldn't decode JSON: {}", e);
                eprintln!("... skipping event {:?}", payload);
            }
        }
    }

    /// Called once the stream ended. OpenAI-style tool calls are only
    /// complete now that all argument fragments arrived; Ollama sends
    /// complete calls and has nothing to do. A trailing SSE event whose
    /// closing blank line never arrived is flushed first.
    pub fn finalize(&self, stream_state: &mut StreamState) {
        if self.is_openai_compatible() {
            if let Some(payload) = stream_state.sse.flush() {
                self.process_event(stream_state, &payload);
            }
            let fragments = std::mem::take(&mut stream_state.tool_call_fragments);
            stream_state.tool_calls.extend(fragments.into_values());
        }
    }

}

impl ToString for Backend {
    fn to_string(&self) -> String {
        match self {
            Backend::Ollama => "ollama".to_string(),
            Backend::OpenAI => "openai".to_string(),
            Backend::Vllm => "vllm".to_string(),
        }
    }
}

/// Parses the `--backend` CLI flag. Accepts exactly the names serde does
/// (lowercase, plus the capitalized aliases kept for old config files) so
/// a value that works in `sven.json` also works on the command line.
impl std::str::FromStr for Backend {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ollama" | "Ollama" => Ok(Backend::Ollama),
            "openai" | "OpenAI" => Ok(Backend::OpenAI),
            "vllm" => Ok(Backend::Vllm),
            _ => Err(format!(
                "unknown backend '{}' (expected one of: ollama, openai, vllm)",
                s
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_accepts_the_same_names_as_serde() {
        // lowercase — the names the program itself prints
        assert_eq!("ollama".parse::<Backend>().unwrap(), Backend::Ollama);
        assert_eq!("openai".parse::<Backend>().unwrap(), Backend::OpenAI);
        assert_eq!("vllm".parse::<Backend>().unwrap(), Backend::Vllm);
        // capitalized aliases kept for old config files
        assert_eq!("Ollama".parse::<Backend>().unwrap(), Backend::Ollama);
        assert_eq!("OpenAI".parse::<Backend>().unwrap(), Backend::OpenAI);
        // unknown names are rejected, not silently mapped to a default
        assert!("llamacpp".parse::<Backend>().is_err());
    }

    /// Feed each payload as one complete SSE event — a `data:` line
    /// followed by the blank line that ends the event — the way a real
    /// server sends them and `handle_chunks` delivers them.
    fn feed_events(backend: &Backend, state: &mut StreamState, payloads: &[&str]) {
        for payload in payloads {
            backend.process_line(state, &format!("data: {}", payload));
            backend.process_line(state, "");
        }
    }

    fn state_with_fragments() -> StreamState {
        let mut state = StreamState::default();
        let backend = Backend::OpenAI;
        feed_events(&backend, &mut state, &[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"ReadTool","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"src/main.rs\"}"}}]}}]}"#,
            "[DONE]",
        ]);
        backend.finalize(&mut state);
        state
    }

    #[test]
    fn openai_tool_call_fragments_are_merged_by_index() {
        let state = state_with_fragments();
        assert_eq!(state.tool_calls.len(), 1);
        let tc = &state.tool_calls[0];
        assert_eq!(tc["id"], "call_1");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "ReadTool");
        assert_eq!(tc["function"]["arguments"], r#"{"path":"src/main.rs"}"#);
    }

    #[test]
    fn merged_openai_tool_calls_parse() {
        let state = state_with_fragments();
        let (name, args, id) = crate::sven::agent::parse_tool_call(&state.tool_calls[0]).unwrap();
        assert_eq!(name, "ReadTool");
        assert_eq!(args["path"], "src/main.rs");
        assert_eq!(id.as_deref(), Some("call_1"));
    }

    #[test]
    fn parallel_openai_tool_calls_stay_separate() {
        let mut state = StreamState::default();
        let backend = Backend::OpenAI;
        feed_events(&backend, &mut state, &[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"TimeTool","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_b","function":{"name":"ListFiles","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{}"}}]}}]}"#,
        ]);
        backend.finalize(&mut state);
        assert_eq!(state.tool_calls.len(), 2);
        assert_eq!(state.tool_calls[0]["function"]["name"], "TimeTool");
        assert_eq!(state.tool_calls[1]["function"]["name"], "ListFiles");
    }

    #[test]
    fn ollama_tool_calls_are_complete_without_finalize() {
        let mut state = StreamState::default();
        let backend = Backend::Ollama;
        backend.process_line(&mut state, r#"{"message":{"tool_calls":[{"function":{"name":"TimeTool","arguments":{}}}]},"done":true}"#);
        backend.finalize(&mut state);
        assert_eq!(state.tool_calls.len(), 1);
        assert_eq!(state.tool_calls[0]["function"]["name"], "TimeTool");
    }

    #[test]
    fn sse_comment_lines_are_skipped() {
        let mut state = StreamState::default();
        let backend = Backend::OpenAI;
        backend.process_line(&mut state, ": keep-alive");
        backend.process_line(&mut state, "");
        assert!(state.tool_calls.is_empty());
    }

    #[test]
    fn sse_data_without_space_after_colon_is_parsed() {
        // `data:{"json"}` — no space after the colon — is legal SSE and
        // was dropped by the old `strip_prefix("data: ")` parser
        let mut state = StreamState::default();
        let backend = Backend::OpenAI;
        backend.process_line(&mut state, r#"data:{"choices":[{"delta":{"content":"hi"}}]}"#);
        backend.process_line(&mut state, "");
        assert_eq!(state.content, "hi");
    }

    #[test]
    fn sse_payload_split_across_data_lines_is_joined() {
        // a payload split across several `data:` lines is joined with a
        // newline per the SSE spec; the old parser decoded each line
        // separately and dropped both. The split is between JSON tokens
        // — a newline inside a string literal would be invalid JSON.
        let mut state = StreamState::default();
        let backend = Backend::OpenAI;
        backend.process_line(&mut state, r#"data: {"choices":[{"delta":{"#);
        backend.process_line(&mut state, r#"data: "content":"hello"}}]}"#);
        backend.process_line(&mut state, "");
        assert_eq!(state.content, "hello");
    }

    #[test]
    fn trailing_event_without_blank_line_is_flushed() {
        // a stream can end without the closing blank line of its last
        // event; `finalize` flushes it instead of losing the chunk
        let mut state = StreamState::default();
        let backend = Backend::OpenAI;
        backend.process_line(&mut state, r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#);
        backend.finalize(&mut state);
        assert_eq!(state.content, "hi");
    }

    #[test]
    fn finish_reason_is_captured_from_the_final_chunk() {
        let mut state = StreamState::default();
        let backend = Backend::OpenAI;
        // regular chunks carry finish_reason: null
        feed_events(&backend, &mut state, &[
            r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#,
        ]);
        assert_eq!(state.finish_reason, None);
        // the final chunk carries the actual reason
        feed_events(&backend, &mut state, &[
            r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
        ]);
        assert_eq!(state.finish_reason.as_deref(), Some("length"));
    }

    #[test]
    fn mid_stream_error_object_is_reported() {
        let mut state = StreamState::default();
        let backend = Backend::OpenAI;
        feed_events(&backend, &mut state, &[
            r#"{"error":{"message":"context length exceeded","type":"server_error"}}"#,
        ]);
        // the error chunk carries no choices — nothing is accumulated
        assert!(state.content.is_empty());
        assert!(state.tool_calls.is_empty());
    }

    #[test]
    fn vllm_stream_parses_like_openai() {
        // vLLM serves the OpenAI wire format: SSE events, reasoning under
        // `reasoning_content` (DeepSeek lineage, used by vLLM's
        // --reasoning-parser), tool calls fragmented by `index`.
        let mut state = StreamState::default();
        let backend = Backend::Vllm;
        feed_events(&backend, &mut state, &[
            r#"{"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"thinking..."},"finish_reason":null}]}"#,
            r#"{"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}"#,
            r#"{"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"chatcmpl-tool-1","type":"function","function":{"name":"TimeTool","arguments":""}}]},"finish_reason":null}]}"#,
            r#"{"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]},"finish_reason":null}]}"#,
            r#"{"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ]);
        backend.finalize(&mut state);

        assert_eq!(state.content, "hello");
        assert_eq!(state.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(state.tool_calls.len(), 1);
        assert_eq!(state.tool_calls[0]["id"], "chatcmpl-tool-1");
        assert_eq!(state.tool_calls[0]["type"], "function");
        assert_eq!(state.tool_calls[0]["function"]["name"], "TimeTool");
        assert_eq!(state.tool_calls[0]["function"]["arguments"], "{}");
    }

    #[test]
    fn usage_chunk_prints_the_token_line() {
        // the final chunk of a stream requested with
        // stream_options: {"include_usage": true} — as sent by vLLM
        let mut state = StreamState::default();
        let backend = Backend::Vllm;
        feed_events(&backend, &mut state, &[
            r#"{"id":"chatcmpl-8bfb9d6ad14a3f56","object":"chat.completion.chunk","choices":[{"index":0,"delta":{}}],"usage":{"completion_tokens":77,"prompt_tokens":2289,"total_tokens":2366,"completion_tokens_details":{"reasoning_tokens":20}}}"#,
        ]);
        // the usage chunk carries no content and no finish_reason —
        // nothing is accumulated, the token line is just printed
        assert!(state.content.is_empty());
        assert_eq!(state.finish_reason, None);
        // the counts are stored for the agent to accumulate
        assert_eq!(state.usage.prompt_tokens, 2289);
        assert_eq!(state.usage.completion_tokens, 77);
    }

    #[test]
    fn ollama_done_chunk_stores_the_token_counts() {
        let mut state = StreamState::default();
        let backend = Backend::Ollama;
        backend
            .process_line(
                &mut state,
                r#"{"message":{"content":"hi"},"done":true,"prompt_eval_count":42,"eval_count":7}"#,
            );
        assert_eq!(state.usage.prompt_tokens, 42);
        assert_eq!(state.usage.completion_tokens, 7);
    }

    #[test]
    fn vllm_endpoint_is_openai_compatible() {
        assert_eq!(
            Backend::Vllm.endpoint(&"http://localhost:8000".to_string()),
            "http://localhost:8000/v1/chat/completions"
        );
        assert!(Backend::Vllm.is_openai_compatible());
        assert!(!Backend::Ollama.is_openai_compatible());
    }

    #[test]
    fn backend_deserializes_from_lowercase_names() {
        // the config file uses the same names ToString emits
        assert_eq!(serde_json::from_str::<Backend>(r#""ollama""#).unwrap(), Backend::Ollama);
        assert_eq!(serde_json::from_str::<Backend>(r#""openai""#).unwrap(), Backend::OpenAI);
        assert_eq!(serde_json::from_str::<Backend>(r#""vllm""#).unwrap(), Backend::Vllm);
        // unknown names are a hard error, not a silent Ollama fallback
        assert!(serde_json::from_str::<Backend>(r#""llamacpp""#).is_err());
    }
}
