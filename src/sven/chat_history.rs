use chrono::Local;
use serde_json::{Value, json};

/// Token counts of one streamed response, as reported by the server
/// (Ollama: `prompt_eval_count`/`eval_count` on the done chunk;
/// OpenAI-compatible: the `usage` chunk). The agent accumulates them
/// across the rounds of a run.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

impl TokenUsage {
    pub fn add(&mut self, other: &TokenUsage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
    }

    pub fn total(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }
}

pub struct MessageResponse {
    pub content: String,
    pub tool_calls: Vec<Value>,
    /// Why the model stopped (`stop`, `length`, `tool_calls`, …). OpenAI
    /// only; Ollama has no equivalent and leaves it None.
    pub finish_reason: Option<String>,
    /// Token counts of this response; the agent sums them over the
    /// rounds of a run.
    pub usage: TokenUsage,
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
            // Every tool call needs `type: "function"` when it is echoed
            // back inside the assistant message — OpenAI-compatible
            // servers reject the request without it (litellm returns a
            // 400 naming `ChatCompletionMessageFunctionToolCallParam`).
            // Servers don't always include it: vLLM puts it only on the
            // first streamed fragment, Ollama omits it entirely, so it
            // is defaulted here, where history becomes the next request
            // body. Ollama ignores the extra field.
            let mut tool_calls = response.tool_calls.clone();
            for tool_call in &mut tool_calls {
                if tool_call.get("type").and_then(Value::as_str).is_none() {
                    tool_call["type"] = json!("function");
                }
            }
            entry["tool_calls"] = json!(tool_calls);
            self.tool_history.push(entry);
            return;
        }

        self.history.push(entry);
    }

    pub fn tool(&mut self, content: &str, tool_name: &str, tool_call_id: Option<&str>) {
        let mut entry = json!({
            "role": "tool",
            "content": content,
            "tool_name": tool_name,
        });
        // OpenAI matches a tool result to its call via `tool_call_id`;
        // Ollama ignores the field, so it is always sent when known.
        if let Some(id) = tool_call_id {
            entry["tool_call_id"] = json!(id);
        }
        self.tool_history.push(entry);
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

    fn response(content: &str, tool_calls: Vec<Value>) -> MessageResponse {
        MessageResponse {
            content: content.to_string(),
            tool_calls,
            finish_reason: None,
            usage: TokenUsage::default(),
        }
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
        history.assistant(&response("", vec![tool_call("ReadTool")]));
        history.tool("file contents", "ReadTool", None);
        assert_eq!(roles(&history), ["system", "user", "assistant", "tool"]);

        history.assistant(&response("", vec![tool_call("GrepTool")]));
        history.tool("matches", "GrepTool", None);
        assert_eq!(
            roles(&history),
            ["system", "user", "assistant", "tool", "assistant", "tool"]
        );

        // the final answer ends the turn; the next user message drops the
        // finished tool round from the context
        history.assistant(&response("final answer", Vec::new()));
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
        history.assistant(&response("a", Vec::new()));
        history.pop_user(); // trailing message is assistant — no-op
        assert_eq!(history.get().len(), 3);
    }

    #[test]
    fn tool_results_carry_the_call_id() {
        let mut history = ChatHistory::new("sys");
        history.user("q");
        history.assistant(&response("", vec![json!({
            "id": "call_1",
            "function": {"name": "ReadTool", "arguments": {"path": "src/main.rs"}}
        })]));
        history.tool("file contents", "ReadTool", Some("call_1"));
        let messages = history.get();
        let tool_message = messages.last().unwrap();
        assert_eq!(tool_message["role"], "tool");
        assert_eq!(tool_message["tool_call_id"], "call_1");
        assert_eq!(tool_message["tool_name"], "ReadTool");
    }

    #[test]
    fn tool_calls_sent_back_always_carry_type_function() {
        let mut history = ChatHistory::new("sys");
        history.user("q");
        // Ollama-shaped call: no `type`, object arguments — the field is
        // defaulted so the history is valid for any backend
        history.assistant(&response("", vec![json!({
            "function": {"name": "TimeTool", "arguments": {}}
        })]));
        let messages = history.get();
        let assistant = messages.last().unwrap();
        assert_eq!(assistant["tool_calls"][0]["type"], "function");
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