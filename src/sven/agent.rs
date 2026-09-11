use std::io::Write;

use reqwest::{Client, Response};
use serde_json::{Value, from_str, json};

use crate::sven::{sven::ChatOptions, tool_registry::ToolRegistry};

struct MessageResponse {
    content: String,
    thinking: String,
    tool_calls: Vec<Value>,
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
}

impl Agent {
    pub fn new(agent_config: AgentConfig) -> Agent {
        Agent {
            client: Client::new(),
            config: agent_config,
        }
    }

    pub async fn run(&self, message: &str) {
        let mut history: Vec<Value> = vec![];
        history.push(json!({
            "role": "system",
            "content": &self.config.system_prompt
        }));
        history.push(json!({
            "role": "user",
            "content": &message
        }));
        println!("");
        loop {
            let url = format!("{}/api/chat", &self.config.host);
            let builder = self.client.post(url).json(&json!({
                "model": &self.config.model,
                "stream": true,
                "options": &self.config.options,
                "tools": &self.config.tool_registry.generate_tool_definitions(),
                "messages": &history
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
            history.push(json!({
                "role": "assist",
                "content": "",
                "tool_calls": message.tool_calls
            }));
            for tool_call in message.tool_calls {
                let tool_name = tool_call["function"]["name"].as_str().unwrap();
                let tool_params = tool_call["function"]["arguments"].clone();
                history.push(json!({
                    "role": "tool",
                    "content": self.process_tool_call(&tool_name, tool_params),
                    "tool_name": tool_name
                }));
            }
        }
    }

    async fn handle_chunks(&self, response: &mut Response) -> MessageResponse {
        let mut content: String = String::new();
        let mut thoughts: String = String::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut is_thinking: bool = false;
        let mut is_answering: bool = false;
        while let Ok(chunk) = response.chunk().await {
            let bytes = match chunk {
                Some(b) => b,
                None => break,
            };

            let str = match String::from_utf8(bytes.to_vec()) {
                Ok(s) => s,
                Err(e) => {
                    println!("... couldn't decode utf8 ...{}", e);
                    break;
                }
            };

            str.split('\n').for_each(|s| {
                if s.is_empty() {
                    return;
                }

                let json: Value = match from_str(&s) {
                    Ok(j) => j,
                    Err(e) => {
                        println!("... couldn't decode JSON: {}", e);
                        return;
                    }
                };

                if let Some(thinking_chunk) = json["message"]["thinking"].as_str() {
                    if !thinking_chunk.is_empty() {
                        if !is_thinking {
                            is_thinking = true;
                            println!("... start thinking ...\n");
                        }
                        thoughts.push_str(&thinking_chunk);
                        print!("{}", &thinking_chunk);
                    }
                } else {
                    if is_thinking {
                        is_thinking = false;
                        println!("\n\n... stopped thinking ...\n");
                    }
                }

                if let Some(content_chunk) = json["message"]["content"].as_str() {
                    if !content_chunk.is_empty() {
                        if !is_answering {
                            is_answering = true;
                        }

                        content.push_str(&content_chunk);
                        print!("{}", &content_chunk);
                    }
                } else {
                    if is_answering {
                        is_answering = false;
                        println!("\n");
                    }
                }

                if let Some(done) = json["done"].as_bool() {
                    if done && is_answering {
                        print!("\n");
                    }
                }

                if let Some(tcs) = json["message"]["tool_calls"].as_array() {
                    for tc in tcs {
                        tool_calls.push(tc.clone());
                    }
                }
            });

            let _ = std::io::stdout().flush();
        }

        MessageResponse {
            content: content,
            thinking: thoughts,
            tool_calls: tool_calls,
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
}
