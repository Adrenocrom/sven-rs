use serde::Deserialize;
use serde_json::Value;

use crate::sven::{agent::StreamState, term};

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub enum Backend {
    Ollama,
    OpenAI, // API key lives here
}

impl Backend {
    pub fn endpoint(&self, host: &String) -> String {
        match self {
            Backend::Ollama => format!("{}/api/chat", &host),
            Backend::OpenAI => format!("{}/v1/chat/completions", &host),
        }
    }

    pub fn process_json(&self, stream_state: &mut StreamState, json: &Value) {
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
