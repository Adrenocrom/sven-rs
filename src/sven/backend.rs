use serde::Deserialize;
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
#[derive(Deserialize, Clone, Debug, PartialEq)]
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

fn print_thinking(backend: &Backend, stream_state: &mut StreamState, chunk: &str) {
    if !chunk.is_empty() {
        if !stream_state.is_thinking {
            stream_state.is_thinking = true;
            print!("\n{}", term::thinking(&backend));
        }
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
    else {
        if stream_state.is_answering {
            stream_state.is_answering = false;
            println!("");
        }
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
    }

    if
        let Some(eval_count) = json["eval_count"].as_u64() &&
        let Some(prompt_eval_count) = json["prompt_eval_count"].as_u64()
    {
        println!("\n{}\n", term::bold(&format!("in {} out {}", prompt_eval_count, eval_count)));
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
        println!("");
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

    pub fn process_line(&self, stream_state: &mut StreamState, line: &str) -> Result<(), serde_json::Error> {
        match self {
            Backend::Ollama => {
                //println!("\x1b[33m {}", &line);
                let json = from_str::<Value>(&line)?;
                process_json_ollama(stream_state, &json);
                Ok(())
            },
            Backend::OpenAI | Backend::Vllm => {
                //println!("\x1b[33m {}", &line);
                if line.eq("data: [DONE]") {
                    return Ok(());
                }
                let Some(payload) = line.strip_prefix("data: ") else {
                    // SSE comments (": keep-alive") and empty lines are
                    // skipped; anything else is not a data event
                    return Ok(());
                };
                let json = from_str::<Value>(payload)?;
                process_json_openai(&self, stream_state, &json);
                Ok(())
            }
        }
    }

    /// Called once the stream ended. OpenAI-style tool calls are only
    /// complete now that all argument fragments arrived; Ollama sends
    /// complete calls and has nothing to do.
    pub fn finalize(&self, stream_state: &mut StreamState) {
        if self.is_openai_compatible() {
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

    fn state_with_fragments() -> StreamState {
        let mut state = StreamState::default();
        let backend = Backend::OpenAI;
        backend.process_line(&mut state, r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"ReadTool","arguments":""}}]}}]}"#).unwrap();
        backend.process_line(&mut state, r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\""}}]}}]}"#).unwrap();
        backend.process_line(&mut state, r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"src/main.rs\"}"}}]}}]}"#).unwrap();
        backend.process_line(&mut state, "data: [DONE]").unwrap();
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
        backend.process_line(&mut state, r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"TimeTool","arguments":""}}]}}]}"#).unwrap();
        backend.process_line(&mut state, r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_b","function":{"name":"ListFiles","arguments":""}}]}}]}"#).unwrap();
        backend.process_line(&mut state, r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]}}]}"#).unwrap();
        backend.process_line(&mut state, r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{}"}}]}}]}"#).unwrap();
        backend.finalize(&mut state);
        assert_eq!(state.tool_calls.len(), 2);
        assert_eq!(state.tool_calls[0]["function"]["name"], "TimeTool");
        assert_eq!(state.tool_calls[1]["function"]["name"], "ListFiles");
    }

    #[test]
    fn ollama_tool_calls_are_complete_without_finalize() {
        let mut state = StreamState::default();
        let backend = Backend::Ollama;
        backend.process_line(&mut state, r#"{"message":{"tool_calls":[{"function":{"name":"TimeTool","arguments":{}}}]},"done":true}"#).unwrap();
        backend.finalize(&mut state);
        assert_eq!(state.tool_calls.len(), 1);
        assert_eq!(state.tool_calls[0]["function"]["name"], "TimeTool");
    }

    #[test]
    fn sse_comment_lines_are_skipped() {
        let mut state = StreamState::default();
        let backend = Backend::OpenAI;
        backend.process_line(&mut state, ": keep-alive").unwrap();
        backend.process_line(&mut state, "").unwrap();
        assert!(state.tool_calls.is_empty());
    }

    #[test]
    fn finish_reason_is_captured_from_the_final_chunk() {
        let mut state = StreamState::default();
        let backend = Backend::OpenAI;
        // regular chunks carry finish_reason: null
        backend
            .process_line(
                &mut state,
                r#"data: {"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#,
            )
            .unwrap();
        assert_eq!(state.finish_reason, None);
        // the final chunk carries the actual reason
        backend
            .process_line(
                &mut state,
                r#"data: {"choices":[{"delta":{},"finish_reason":"length"}]}"#,
            )
            .unwrap();
        assert_eq!(state.finish_reason.as_deref(), Some("length"));
    }

    #[test]
    fn mid_stream_error_object_is_reported() {
        let mut state = StreamState::default();
        let backend = Backend::OpenAI;
        backend
            .process_line(
                &mut state,
                r#"data: {"error":{"message":"context length exceeded","type":"server_error"}}"#,
            )
            .unwrap();
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
        backend
            .process_line(&mut state, r#"data: {"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"thinking..."},"finish_reason":null}]}"#)
            .unwrap();
        backend
            .process_line(&mut state, r#"data: {"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}"#)
            .unwrap();
        backend
            .process_line(&mut state, r#"data: {"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"chatcmpl-tool-1","type":"function","function":{"name":"TimeTool","arguments":""}}]},"finish_reason":null}]}"#)
            .unwrap();
        backend
            .process_line(&mut state, r#"data: {"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]},"finish_reason":null}]}"#)
            .unwrap();
        backend
            .process_line(&mut state, r#"data: {"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#)
            .unwrap();
        backend.process_line(&mut state, "data: [DONE]").unwrap();
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
