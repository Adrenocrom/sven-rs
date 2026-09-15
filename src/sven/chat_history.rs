use serde_json::{Value, json};

pub struct MessageResponse {
    pub content: String,
    //pub thinking: String,
    pub tool_calls: Vec<Value>,
}

pub struct ChatHistory {
    history: Vec<Value>,
    tool_history: Vec<Value>,
    system_prompt: String,
}

impl ChatHistory {
    pub fn new(system_prompt: &str) -> Self {
         Self {
            history: vec![json!({
                "role": "system",
                "content": system_prompt
            })],
            tool_history: Vec::new(),
            system_prompt: system_prompt.to_string(),
        }
    }

    pub fn user(&mut self, prompt: &str) {
        self.tool_history.clear();
        self.history.push(json!({
            "role": "user",
            "content": prompt
        }));
    }

    pub fn assistant(&mut self, response: &MessageResponse) {
        let mut entry = json!({
            "role": "assistant",
            "content": response.content,
            "tool_calls": response.tool_calls
        });

        if !&response.tool_calls.is_empty() {
            entry["tool_calls"] = json!(&response.tool_calls);
            self.tool_history.push(entry);
            return;
        }

        self.history.push(entry);
    }

    pub fn tool(&mut self, content: &str, tool_name: &str, _id: Option<Value>) {
        self.tool_history.push(json!({
            "role": "tool",
            "content": content,
            "tool_name": tool_name,
        }));
    }

    pub fn get(&self) -> Vec<Value> {
        let mut combined: Vec<Value> = Vec::new();
        combined.extend(self.history.clone());
        combined.extend(self.tool_history.clone());
        combined
    }

    pub fn clear(&mut self) {
        self.history.clear();
        self.history.push(json!({
            "role": "system",
            "content": &self.system_prompt
        }));
        self.tool_history.clear();
    }
}
