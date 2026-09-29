use chrono::Local;
use serde_json::{Value, json};

pub struct MessageResponse {
    pub content: String,
    pub tool_calls: Vec<Value>,
}

/// Conversation history. `history` holds the durable messages (system,
/// user, final assistant answers); `tool_history` holds the messages of
/// the tool round-robin currently in progress. Both are concatenated in
/// order for each request; the tool round is dropped when the next user
/// message arrives.
#[derive(Debug)]
pub struct ChatHistory {
    history: Vec<Value>,
    tool_history: Vec<Value>,
    system_prompt: String,
}

impl ChatHistory {
    pub fn new(system_prompt: &str) -> Self {
        Self {
            history: vec![ChatHistory::system(system_prompt)],
            tool_history: Vec::new(),
            system_prompt: system_prompt.to_string(),
        }
    }

    fn system(prompt: &str) -> Value {
        let now = Local::now();
        let system_prompt = format!("{}\nCurrent Date: {}", prompt, now);
        json!({
            "role": "system",
            "content": system_prompt
        })
    }

    pub fn user(&mut self, prompt: &str) {
        self.tool_history.clear();
        self.history.push(json!({
            "role": "user",
            "content": prompt
        }));
    }

    /// Drop a trailing user message — used when the request failed, so the
    /// next turn doesn't start with an unanswered prompt.
    pub fn pop_user(&mut self) {
        if self.history.last().is_some_and(|m| m["role"] == "user") {
            self.history.pop();
        }
    }

    pub fn assistant(&mut self, response: &MessageResponse) {
        let mut entry = json!({
            "role": "assistant",
            "content": response.content,
        });

        if !response.tool_calls.is_empty() {
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

    /// Append a plain assistant message to the current tool round (used
    /// for the round-cap notice), keeping chronological order with the
    /// surrounding tool messages.
    pub fn assistant_note(&mut self, content: &str) {
        self.tool_history.push(json!({
            "role": "assistant",
            "content": content,
        }));
    }

    /// All messages to send: the durable history followed by the current
    /// tool round. Returns borrowed values — the history can be large and
    /// deep-cloning it on every request is wasteful.
    pub fn get(&self) -> Vec<&Value> {
        let mut combined: Vec<&Value> =
            Vec::with_capacity(self.history.len() + self.tool_history.len());
        combined.extend(self.history.iter());
        combined.extend(self.tool_history.iter());
        combined
    }

    pub fn clear(&mut self) {
        self.history.clear();
        self.history.push(ChatHistory::system(&self.system_prompt));
        self.tool_history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call(name: &str) -> Value {
        json!({"function": {"name": name, "arguments": {}}})
    }

    fn roles(history: &ChatHistory) -> Vec<&str> {
        history
            .get()
            .iter()
            .copied()
            .map(|m| m["role"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn orders_messages_across_tool_rounds() {
        let mut history = ChatHistory::new("sys");
        history.user("question");
        history.assistant(&MessageResponse {
            content: String::new(),
            tool_calls: vec![tool_call("ReadTool")],
        });
        history.tool("file contents", "ReadTool", None);
        assert_eq!(roles(&history), ["system", "user", "assistant", "tool"]);

        history.assistant(&MessageResponse {
            content: String::new(),
            tool_calls: vec![tool_call("GrepTool")],
        });
        history.tool("matches", "GrepTool", None);
        assert_eq!(
            roles(&history),
            ["system", "user", "assistant", "tool", "assistant", "tool"]
        );

        // the final answer ends the turn; the next user message drops the
        // finished tool round from the context
        history.assistant(&MessageResponse {
            content: "final answer".to_string(),
            tool_calls: Vec::new(),
        });
        history.user("next question");
        assert_eq!(roles(&history), ["system", "user", "assistant", "user"]);
    }

    #[test]
    fn pop_user_removes_only_a_trailing_user_message() {
        let mut history = ChatHistory::new("sys");
        history.pop_user(); // trailing message is the system prompt — no-op
        assert_eq!(history.get().len(), 1);

        history.user("q");
        history.pop_user();
        assert_eq!(history.get().len(), 1);

        history.user("q");
        history.assistant(&MessageResponse {
            content: "a".to_string(),
            tool_calls: Vec::new(),
        });
        history.pop_user(); // trailing message is assistant — no-op
        assert_eq!(history.get().len(), 3);
    }

    #[test]
    fn clear_resets_to_the_system_prompt() {
        let mut history = ChatHistory::new("be helpful");
        history.user("hi");
        history.clear();
        let messages = history.get();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "system");
        assert!(messages[0]["content"].as_str().unwrap().contains("be helpful"));
    }
}