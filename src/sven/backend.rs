use serde::Deserialize;
use serde_json::{Value, from_str};

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
    if let Some(thinking_chunk) = json["choices"][0]["delta"]["reasoning"].as_str() {
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
            println!("{}", term::reset());
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

    if json["choices"][0].get("finish_reason") == None && stream_state.is_answering {
        print!("\n");
    }

    //print!("{}", json);
    if let Some(tcs) = json["choices"][0]["delta"]["tool_calls"].as_array() {
        for tc in tcs {
            stream_state.tool_calls.push(tc.clone());
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
                if line.eq("data: [DONE]") {
                    return Ok(());
                }
                let mut line = line.rsplit("data: ");
                let line = line.next().unwrap();
                let json = from_str::<Value>(&line)?;
                process_json_openai(stream_state, &json);
                Ok(())
            }
        }
    }

    pub fn process_json(&self, stream_state: &mut StreamState, json: &Value) {
        match self {
            Backend::Ollama => process_json_ollama(stream_state, json),
            Backend::OpenAI => process_json_openai(stream_state, json),
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
