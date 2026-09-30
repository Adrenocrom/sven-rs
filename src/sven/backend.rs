use serde::Deserialize;
use serde_json::{Value, from_str, json};

use crate::sven::{agent::StreamState, term};

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub enum Backend {
    Ollama,
    OpenAI, // API key lives here
}

fn process_json_ollama(stream_state: &mut StreamState, json: &Value) {
    if let Some(thinking_chunk) = json["message"]["thinking"].as_str() {
        if !thinking_chunk.is_empty() {
            if !stream_state.is_thinking {
                stream_state.is_thinking = true;
                print!("{}", term::thinking());
            }
            print!("{}", thinking_chunk);
        }
    } else if stream_state.is_thinking {
        stream_state.is_thinking = false;
        if term::enabled() {
            println!("{}\n", term::reset());
        } else {
            println!();
        }
    }

    if let Some(content_chunk) = json["message"]["content"].as_str() {
        if !content_chunk.is_empty() {
            stream_state.is_answering = true;
            stream_state.content.push_str(content_chunk);
            print!("{}", content_chunk);
        }
    } else if stream_state.is_answering {
        stream_state.is_answering = false;
        println!("\n");
    }

    if json["done"].as_bool() == Some(true) && stream_state.is_answering {
        print!("\n");
    }

    if
        let Some(eval_count) = json["eval_count"].as_u64() &&
        let Some(prompt_eval_count) = json["prompt_eval_count"].as_u64()
    {
        println!("\n{}", term::bold(&format!("in {} out {}", prompt_eval_count, eval_count)));
        println!();
    }

    if let Some(tcs) = json["message"]["tool_calls"].as_array() {
        for tc in tcs {
            stream_state.tool_calls.push(tc.clone());
        }
    }
}

fn process_json_openai(stream_state: &mut StreamState, json: &Value) {
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
        if !thinking_chunk.is_empty() {
            if !stream_state.is_thinking {
                stream_state.is_thinking = true;
                print!("{}", term::thinking());
            }
            print!("{}", thinking_chunk);
        }
    } 
    else if let Some(thinking_chunk) = json["choices"][0]["delta"]["reasoning_content"].as_str() {
        if !thinking_chunk.is_empty() {
            if !stream_state.is_thinking {
                stream_state.is_thinking = true;
                print!("{}", term::thinking());
            }
            print!("{}", thinking_chunk);
        }
    } 
    else if stream_state.is_thinking {
        stream_state.is_thinking = false;
        if term::enabled() {
            println!("{}\n", term::reset());
        } else {
            println!();
        }
    }

    if let Some(content_chunk) = json["choices"][0]["delta"]["content"].as_str() {
        if !content_chunk.is_empty() {
            stream_state.is_answering = true;
            stream_state.content.push_str(content_chunk);
            print!("{}", content_chunk);
        }
    } else if stream_state.is_answering {
        stream_state.is_answering = false;
        println!("\n");
    }

    if let Some(finish_reason) = json["choices"][0].get("finish_reason").and_then(Value::as_str) {
        stream_state.finish_reason = Some(finish_reason.to_string());
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
            Backend::OpenAI => format!("{}/v1/chat/completions", &host),
        }
    }

    pub fn process_line(&self, stream_state: &mut StreamState, line: &str) -> Result<(), serde_json::Error> {
        match self {
            Backend::Ollama => {
                let json = from_str::<Value>(&line)?;
                process_json_ollama(stream_state, &json);
                Ok(())
            },
            Backend::OpenAI => {
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
                process_json_openai(stream_state, &json);
                Ok(())
            }
        }
    }

    /// Called once the stream ended. OpenAI tool calls are only complete
    /// now that all argument fragments arrived; Ollama sends complete
    /// calls and has nothing to do.
    pub fn finalize(&self, stream_state: &mut StreamState) {
        if let Backend::OpenAI = self {
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
        }
    }
}

impl From<String> for Backend {
    fn from(value: String) -> Self {
        match value.as_str() {
            "openai" => Backend::OpenAI,
            _ =>  Backend::Ollama, // Default if not supported by the string
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
