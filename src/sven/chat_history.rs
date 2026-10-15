use chrono::Local;
use serde_json::{Value, json};

use crate::sven::attachment::Attachment;
use crate::sven::backend::Backend;
use crate::sven::tool::ToolOutput;

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
    /// Attachments are serialized into each message as it is pushed, in
    /// this backend's wire format — the stored `Value`s are sent verbatim
    /// on every round, so media stays visible across the whole
    /// conversation without being re-encoded per request.
    backend: Backend,
}

impl ChatHistory {
    pub fn new(system_prompt: &str, backend: Backend) -> Self {
        Self {
            history: vec![ChatHistory::system(system_prompt)],
            tool_history: Vec::new(),
            system_prompt: system_prompt.to_string(),
            backend,
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

    /// Add the user's prompt, with optional attachments (`@path`
    /// mentions expanded by the agent). The attachments are folded into
    /// the message in the backend's wire format — Ollama: a
    /// message-level `images` array; OpenAI-compatible: typed content
    /// parts — and stay in the history, so the model can still see the
    /// media when answering follow-up questions.
    pub fn user(&mut self, prompt: &str, attachments: &[Attachment]) {
        self.tool_history.clear();
        let mut entry = json!({
            "role": "user",
            "content": prompt
        });
        self.backend.apply_user_attachments(&mut entry, attachments);
        self.history.push(entry);
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

    /// Add a tool result, with optional attachments the tool produced
    /// (an MCP server returning an image, say). Ollama's message object
    /// carries `images` on any role; the OpenAI-compatible wire format
    /// has no binary parts in tool messages, so the backend appends a
    /// text note there instead — the model learns the tool produced
    /// media it cannot see.
    pub fn tool(&mut self, output: &ToolOutput, tool_name: &str, tool_call_id: Option<&str>) {
        let mut entry = json!({
            "role": "tool",
            "content": output.text,
            "tool_name": tool_name,
        });
        // OpenAI matches a tool result to its call via `tool_call_id`;
        // Ollama ignores the field, so it is always sent when known.
        if let Some(id) = tool_call_id {
            entry["tool_call_id"] = json!(id);
        }
        self.backend.apply_tool_attachments(&mut entry, &output.attachments);
        self.tool_history.push(entry);
    }

    /// Remove every attachment from the stored messages — each stripped
    /// message gets a note, so the model learns the media was mentioned
    /// but never delivered (see `Backend::strip_message_attachments`).
    /// Returns whether anything was removed: the caller retries the
    /// request only then, so a persistent error fails the run instead
    /// of looping.
    pub fn strip_attachments(&mut self) -> bool {
        let mut stripped = false;
        for message in self.history.iter_mut().chain(self.tool_history.iter_mut()) {
            stripped |= self.backend.strip_message_attachments(message);
        }
        stripped
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

    fn image() -> Attachment {
        Attachment::Image {
            data: "aGk=".to_string(), // base64 of "hi"
            mime: "image/png".to_string(),
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
        let mut history = ChatHistory::new("sys", Backend::Ollama);
        history.user("question", &[]);
        history.assistant(&response("", vec![tool_call("ReadTool")]));
        history.tool(&ToolOutput::text("file contents"), "ReadTool", None);
        assert_eq!(roles(&history), ["system", "user", "assistant", "tool"]);

        history.assistant(&response("", vec![tool_call("GrepTool")]));
        history.tool(&ToolOutput::text("matches"), "GrepTool", None);
        assert_eq!(
            roles(&history),
            ["system", "user", "assistant", "tool", "assistant", "tool"]
        );

        // the final answer ends the turn; the next user message drops the
        // finished tool round from the context
        history.assistant(&response("final answer", Vec::new()));
        history.user("next question", &[]);
        assert_eq!(roles(&history), ["system", "user", "assistant", "user"]);
    }

    #[test]
    fn pop_user_removes_only_a_trailing_user_message() {
        let mut history = ChatHistory::new("sys", Backend::Ollama);
        history.pop_user(); // trailing message is the system prompt — no-op
        assert_eq!(history.get().len(), 1);

        history.user("q", &[]);
        history.pop_user();
        assert_eq!(history.get().len(), 1);

        history.user("q", &[]);
        history.assistant(&response("a", Vec::new()));
        history.pop_user(); // trailing message is assistant — no-op
        assert_eq!(history.get().len(), 3);
    }

    #[test]
    fn tool_results_carry_the_call_id() {
        let mut history = ChatHistory::new("sys", Backend::Ollama);
        history.user("q", &[]);
        history.assistant(&response("", vec![json!({
            "id": "call_1",
            "function": {"name": "ReadTool", "arguments": {"path": "src/main.rs"}}
        })]));
        history.tool(&ToolOutput::text("file contents"), "ReadTool", Some("call_1"));
        let messages = history.get();
        let tool_message = messages.last().unwrap();
        assert_eq!(tool_message["role"], "tool");
        assert_eq!(tool_message["tool_call_id"], "call_1");
        assert_eq!(tool_message["tool_name"], "ReadTool");
    }

    #[test]
    fn tool_calls_sent_back_always_carry_type_function() {
        let mut history = ChatHistory::new("sys", Backend::Ollama);
        history.user("q", &[]);
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
        let mut history = ChatHistory::new("be helpful", Backend::Ollama);
        history.user("hi", &[]);
        history.clear();
        let messages = history.get();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "system");
        assert!(messages[0]["content"].as_str().unwrap().contains("be helpful"));
    }

    #[test]
    fn user_attachments_serialize_per_backend() {
        // Ollama: a message-level `images` array, content stays a string
        let mut history = ChatHistory::new("sys", Backend::Ollama);
        history.user("look at this", &[image()]);
        let messages = history.get();
        let message = messages.last().unwrap();
        assert_eq!(message["content"], "look at this");
        assert_eq!(message["images"], json!(["aGk="]));

        // OpenAI: `content` becomes an array of typed parts
        let mut history = ChatHistory::new("sys", Backend::OpenAI);
        history.user("look at this", &[image()]);
        let messages = history.get();
        let message = messages.last().unwrap();
        let parts = message["content"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], json!({"type": "text", "text": "look at this"}));
        assert_eq!(
            parts[1]["image_url"]["url"],
            "data:image/png;base64,aGk="
        );

        // no attachments → the message stays a plain string, exactly as
        // before the multimodal support
        let mut history = ChatHistory::new("sys", Backend::OpenAI);
        history.user("plain", &[]);
        let messages = history.get();
        let message = messages.last().unwrap();
        assert_eq!(message["content"], "plain");
        assert!(message.get("content").unwrap().is_string());
    }

    #[test]
    fn tool_attachments_ride_on_ollama_and_note_on_openai() {
        // Ollama's message object carries `images` on any role, so a
        // tool result can show the model an image
        let mut history = ChatHistory::new("sys", Backend::Ollama);
        history.user("q", &[]);
        history.tool(
            &ToolOutput { text: "see".to_string(), attachments: vec![image()] },
            "ShotTool",
            None,
        );
        let messages = history.get();
        let tool_message = messages.last().unwrap();
        assert_eq!(tool_message["images"], json!(["aGk="]));
        assert_eq!(tool_message["content"], "see");

        // the chat-completions wire format has no binary parts in tool
        // messages — the attachment becomes a note in the text
        let mut history = ChatHistory::new("sys", Backend::OpenAI);
        history.user("q", &[]);
        history.tool(
            &ToolOutput { text: "see".to_string(), attachments: vec![image()] },
            "ShotTool",
            None,
        );
        let messages = history.get();
        let tool_message = messages.last().unwrap();
        let content = tool_message["content"].as_str().unwrap();
        assert!(content.starts_with("see"), "{}", content);
        assert!(content.contains("image"), "{}", content);
        assert!(content.contains("not visible"), "{}", content);
        assert!(tool_message.get("images").is_none());
    }

    #[test]
    fn strip_attachments_removes_media_and_notes_the_model() {
        // OpenAI: the content parts collapse back to the plain prompt,
        // with a note naming the dropped modality
        let mut history = ChatHistory::new("sys", Backend::OpenAI);
        history.user("look at this", &[image()]);
        history.user("plain", &[]);
        assert!(history.strip_attachments());
        let messages = history.get();
        let stripped = messages[1];
        assert!(stripped.get("content").unwrap().is_string());
        let content = stripped["content"].as_str().unwrap();
        assert!(content.starts_with("look at this"), "{}", content);
        assert!(content.contains("image attachment(s) removed"), "{}", content);
        // the plain message is untouched
        assert_eq!(messages[2]["content"], "plain");

        // nothing left to strip → false, so the caller does not retry
        assert!(!history.strip_attachments());

        // Ollama: the images array is removed, the note is appended
        let mut history = ChatHistory::new("sys", Backend::Ollama);
        history.user("look at this", &[image()]);
        assert!(history.strip_attachments());
        let messages = history.get();
        let stripped = messages.last().unwrap();
        assert!(stripped.get("images").is_none());
        let content = stripped["content"].as_str().unwrap();
        assert!(content.starts_with("look at this"), "{}", content);
        assert!(content.contains("image attachment(s) removed"), "{}", content);
    }
}