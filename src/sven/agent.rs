use std::io::Write;

use reqwest::{Client, Response};
use serde_json::{Value, from_str, json};

use crate::sven::{chat_history::{ChatHistory, MessageResponse}, config::ChatOptions, tool_registry::ToolRegistry};

#[derive(Default)]
struct StreamState {
    content: String,
    thinking: String,
    tool_calls: Vec<Value>,
    is_thinking: bool,
    is_answering: bool,
}

impl StreamState {
    fn process_json(&mut self, json: &Value) {
        if let Some(thinking_chunk) = json["message"]["thinking"].as_str() {
            if !thinking_chunk.is_empty() {
                if !self.is_thinking {
                    self.is_thinking = true;
                    print!("\x1b[38;2;10;140;75m");
                }
                self.thinking.push_str(thinking_chunk);
                print!("{}", thinking_chunk);
            }
        } else if self.is_thinking {
            self.is_thinking = false;
            println!("\x1b[0m\n");
        }

        if let Some(content_chunk) = json["message"]["content"].as_str() {
            if !content_chunk.is_empty() {
                self.is_answering = true;
                self.content.push_str(content_chunk);
                print!("{}", content_chunk);
            }
        } else if self.is_answering {
            self.is_answering = false;
            println!("\n");
        }

        if json["done"].as_bool() == Some(true) && self.is_answering {
            print!("\n");
        }

        if let Some(tcs) = json["message"]["tool_calls"].as_array() {
            for tc in tcs {
                self.tool_calls.push(tc.clone());
            }
        }
    }
}

pub struct AgentConfig {
    pub host: String,
    pub model: String,
    pub system_prompt: String,
    pub options: ChatOptions,
    pub tool_registry: ToolRegistry,
}

pub struct Agent {
    client: Client,
    config: AgentConfig,
    history: ChatHistory
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
        loop {
            let url = format!("{}/api/chat", &self.config.host);
            let builder = self.client.post(url).json(&json!({
                "model": &self.config.model,
                "stream": true,
                "options": &self.config.options,
                "tools": &self.config.tool_registry.generate_tool_definitions(),
                "messages": self.history.get()
            }));
            let mut response = match builder.send().await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: {}", e);
                    return;
                }
            };

            let message: MessageResponse = self.handle_chunks(&mut response).await;
            if message.tool_calls.is_empty() {
                break;
            }
            self.history.assistant(&message);
            for tool_call in message.tool_calls {
                let tool_name = tool_call["function"]["name"].as_str().unwrap();
                let tool_params = tool_call["function"]["arguments"].clone();
                self.history.tool(&self.process_tool_call(&tool_name, tool_params), &tool_name, None);
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

                match from_str::<Value>(line) {
                    Ok(json) => state.process_json(&json),
                    Err(e) => {
                        println!("... couldn't decode JSON: {}", e);
                        println!("... skipping line {:?}", line);
                    }
                }
            }

            let _ = std::io::stdout().flush();
        }

        if !buffer.is_empty() {
            let line = String::from_utf8_lossy(&buffer);
            let line = line.trim();
            if !line.is_empty() {
                match from_str::<Value>(line) {
                    Ok(json) => state.process_json(&json),
                    Err(e) => println!("... couldn't decode JSON: {}", e),
                }
            }
        }

        MessageResponse {
            content: state.content,
            //thinking: state.thinking,
            tool_calls: state.tool_calls,
        }
    }

    pub fn process_tool_call(&self, tool_name: &str, params: Value) -> String {
        match self.config.tool_registry.get_tool(tool_name) {
            Some(tool) => {
                println!("\t🔧  \x1b[32m{}\x1b[0m {}\n", tool_name, params);
                let result = match tool.execute(params) {
                    Ok(result) => result,
                    Err(err) => {
                        println!("    \x1b[31mERROR: {}\x1b[0m", err);
                        err.to_string()
                    }
                };
                return result;
            }
            None => return format!("Error: Tool '{}' not found in registry", tool_name),
        }
    }

    pub fn clear(&mut self) {
        self.history.clear();
    }
}
