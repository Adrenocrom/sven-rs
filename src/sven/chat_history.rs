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

/// Prefix of the note `trim_tool_history` leaves when it drops rounds,
/// so repeated trims within one turn replace rather than stack notes.
const TRIM_NOTE_PREFIX: &str = "[context trimmed:";

/// Conversation history. `history` holds the durable messages (system,
/// user, final assistant answers); `tool_history` holds the messages of
/// the tool round currently in progress, bounded to `max_tool_calls`
/// tool results when set — beyond that the oldest complete rounds are
/// dropped. Both are concatenated in order for each request; the tool
/// round is dropped entirely when the next user message arrives.
#[derive(Debug)]
pub struct ChatHistory {
    history: Vec<Value>,
    tool_history: Vec<Value>,
    /// Cap on tool results kept in `tool_history`; the oldest complete
    /// rounds are dropped once it is exceeded. `None` disables the cap.
    max_tool_calls: Option<usize>,
    system_prompt: String,
    /// Attachments are serialized into each message as it is pushed, in
    /// this backend's wire format — the stored `Value`s are sent verbatim
    /// on every round, so media stays visible across the whole
    /// conversation without being re-encoded per request.
    backend: Backend,
    /// Attachments produced by tools during the current round on an
    /// OpenAI-compatible backend. The chat-completions tool message has
    /// no binary parts, so they cannot ride on the tool result; they
    /// are buffered here and delivered by `flush_tool_attachments` as
    /// one user message once the round's tool results are complete —
    /// the only role every OpenAI-compatible server accepts image and
    /// audio parts on. Ollama never buffers (its tool messages carry
    /// `images` directly), so this stays empty there.
    pending_tool_attachments: Vec<Attachment>,
}

impl ChatHistory {
    pub fn new(system_prompt: &str, backend: Backend, max_tool_calls: Option<usize>) -> Self {
        Self {
            history: vec![ChatHistory::system(system_prompt, max_tool_calls)],
            tool_history: Vec::new(),
            max_tool_calls,
            system_prompt: system_prompt.to_string(),
            backend,
            pending_tool_attachments: Vec::new(),
        }
    }

    fn system(prompt: &str, max_tool_calls: Option<usize>) -> Value {
        let now = Local::now();
        let mut system_prompt = format!("{}\nCurrent Date: {}", prompt, now);

        if let Some(max_tool_calls) = max_tool_calls {
            system_prompt += format!("\nMax tool calls in context: {}", max_tool_calls).as_str();
        }


        json!({
            "role": "system",
            "content": system_prompt
        })
    }

    /// Push onto `tool_history` and keep it bounded: once more than
    /// `max_tool_calls` tool results are stored, the oldest complete
    /// rounds are dropped. Every push goes through here, so the cap
    /// cannot be bypassed by a future call site.
    fn push_tool_entry(&mut self, entry: Value) {
        self.tool_history.push(entry);
        self.trim_tool_history();
    }

    /// Drop the oldest complete tool rounds until at most
    /// `max_tool_calls` tool results remain. A round is one assistant
    /// message carrying `tool_calls` plus its `tool` result(s) (and any
    /// attachment-flush user message that followed); removing a partial
    /// round would strand a `role: "tool"` message without its
    /// `tool_calls` assistant message, which OpenAI-compatible servers
    /// reject with a 400. A note is left at the front so the model
    /// knows earlier results existed and were dropped — it can re-run
    /// a tool instead of assuming work it no longer sees was never
    /// done.
    fn trim_tool_history(&mut self) {
        let Some(max_tool_calls) = self.max_tool_calls else {
            return; // no cap configured — keep everything
        };
        //let mut dropped = false;
        while self.tool_history.iter().filter(|m| m["role"] == "tool").count()
            > max_tool_calls
        {
            //dropped = true;
            // start of the oldest round: the first assistant message
            // carrying tool_calls
            let round_start = self.tool_history.iter().position(|m| {
                m["role"] == "assistant" && m.get("tool_calls").is_some()
            });
            match round_start {
                Some(start) => {
                    // drop up to (not including) the next round's
                    // assistant message, so an entire round disappears
                    let end = self.tool_history[start + 1..]
                        .iter()
                        .position(|m| m["role"] == "assistant" && m.get("tool_calls").is_some())
                        .map(|d| start + 1 + d)
                        .unwrap_or(self.tool_history.len());
                    self.tool_history.drain(start..end);
                }
                // no round boundary (e.g. a stray flush/note before any
                // tool_calls): drop the oldest entry
                None => {
                    self.tool_history.remove(0);
                }
            }
        }
        //// one note per turn, not one per trim: the note is only added
        //// when the front isn't already one
        //let noted = self.tool_history.first().is_some_and(|m| {
        //    m["role"] == "assistant"
        //        && m["content"]
        //            .as_str()
        //            .is_some_and(|c| c.starts_with(TRIM_NOTE_PREFIX))
        //});
        //if dropped && !noted {
        //    self.tool_history.insert(
        //        0,
        //        json!({
        //            "role": "assistant",
        //            "content": format!(
        //                "{} the oldest tool rounds were dropped to keep at most {} \
        //                 tool results in context — re-run a tool if you need an \
        //                 earlier result]",
        //                TRIM_NOTE_PREFIX, max_tool_calls
        //            ),
        //        }),
        //    );
        //}
    }

    /// Add the user's prompt, with optional attachments (`@path`
    /// mentions expanded by the agent). The attachments are folded into
    /// the message in the backend's wire format — Ollama: a
    /// message-level `images` array; OpenAI-compatible: typed content
    /// parts — and stay in the history, so the model can still see the
    /// media when answering follow-up questions.
    pub fn user(&mut self, prompt: &str, attachments: &[Attachment]) {
        self.tool_history.clear();
        // the tool round is dropped, so its undelivered attachments go
        // with it — they belong to a round the model will never see
        self.pending_tool_attachments.clear();
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
            self.push_tool_entry(entry);
            return;
        }

        self.history.push(entry);
    }

    /// Add a tool result, with optional attachments the tool produced
    /// (an MCP server returning an image, say). Ollama's message object
    /// carries `images` on any role, so the media rides on the tool
    /// message itself. The OpenAI-compatible wire format has no binary
    /// parts in tool messages, so the attachments are buffered there
    /// and delivered by `flush_tool_attachments` as one user message
    /// once the round's tool results are complete.
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
        match self.backend {
            Backend::Ollama => {
                self.backend.apply_tool_attachments(&mut entry, &output.attachments)
            }
            Backend::OpenAI | Backend::Vllm => {
                self.pending_tool_attachments.extend(output.attachments.clone())
            }
        }
        self.push_tool_entry(entry);
    }

    /// Deliver the tool attachments buffered by `tool` on an
    /// OpenAI-compatible backend as one user message, so the model
    /// actually sees the media. Called once the round's tool results
    /// are complete — a user message in the middle of the tool round
    /// would interleave the results of parallel calls, and the
    /// chat-completions format requires every `role: "tool"` message to
    /// follow its `tool_calls` assistant message directly. Ollama never
    /// buffers, so this is a no-op there.
    pub fn flush_tool_attachments(&mut self) {
        if self.pending_tool_attachments.is_empty() {
            return;
        }
        let attachments = std::mem::take(&mut self.pending_tool_attachments);
        let mut entry = json!({
            "role": "user",
            "content": "[the tool result(s) above included the following attachment(s) — \
                        use them as if they were part of the tool output]",
        });
        self.backend.apply_user_attachments(&mut entry, &attachments);
        self.push_tool_entry(entry);
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
        self.push_tool_entry(json!({
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
        self.history.push(ChatHistory::system(&self.system_prompt, self.max_tool_calls));
        self.tool_history.clear();
        self.pending_tool_attachments.clear();
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
        let mut history = ChatHistory::new("sys", Backend::Ollama, None);
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
        let mut history = ChatHistory::new("sys", Backend::Ollama, None);
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
        let mut history = ChatHistory::new("sys", Backend::Ollama, None);
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
        let mut history = ChatHistory::new("sys", Backend::Ollama, None);
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
        let mut history = ChatHistory::new("be helpful", Backend::Ollama, None);
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
        let mut history = ChatHistory::new("sys", Backend::Ollama, None);
        history.user("look at this", &[image()]);
        let messages = history.get();
        let message = messages.last().unwrap();
        assert_eq!(message["content"], "look at this");
        assert_eq!(message["images"], json!(["aGk="]));

        // OpenAI: `content` becomes an array of typed parts
        let mut history = ChatHistory::new("sys", Backend::OpenAI, None);
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
        let mut history = ChatHistory::new("sys", Backend::OpenAI, None);
        history.user("plain", &[]);
        let messages = history.get();
        let message = messages.last().unwrap();
        assert_eq!(message["content"], "plain");
        assert!(message.get("content").unwrap().is_string());
    }

    #[test]
    fn tool_attachments_ride_on_ollama_and_flush_on_openai() {
        // Ollama's message object carries `images` on any role, so a
        // tool result can show the model an image
        let mut history = ChatHistory::new("sys", Backend::Ollama, None);
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
        // messages — the attachments are buffered and flushed as one
        // user message once the round's tool results are complete
        let mut history = ChatHistory::new("sys", Backend::OpenAI, None);
        history.user("q", &[]);
        history.tool(
            &ToolOutput { text: "see".to_string(), attachments: vec![image()] },
            "ShotTool",
            None,
        );
        // nothing flushed yet — the tool message stays text-only
        let messages = history.get();
        let tool_message = messages.last().unwrap();
        assert_eq!(tool_message["content"], "see");
        assert!(tool_message.get("images").is_none());

        history.flush_tool_attachments();
        let messages = history.get();
        let flush = messages.last().unwrap();
        assert_eq!(flush["role"], "user");
        let parts = flush["content"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(
            parts[1]["image_url"]["url"],
            "data:image/png;base64,aGk="
        );
        // the buffer is drained — a second flush adds nothing
        let len_after_flush = messages.len();
        history.flush_tool_attachments();
        assert_eq!(history.get().len(), len_after_flush);
    }

    #[test]
    fn tool_history_is_capped_dropping_oldest_rounds() {
        let mut history = ChatHistory::new("sys", Backend::Ollama, Some(2));

        // two full rounds (assistant tool_calls + tool result) = 2 tool
        // results, exactly at the cap — nothing is dropped
        for name in ["ReadTool", "GrepTool"] {
            history.assistant(&response("", vec![tool_call(name)]));
            history.tool(&ToolOutput::text("ok"), name, None);
        }
        // system prompt + two rounds of two messages each
        assert_eq!(history.get().len(), 5);

        // a third round overflows the cap → the first round is dropped,
        // leaving the trim note plus the two newest rounds, still paired
        history.assistant(&response("", vec![tool_call("FindTool")]));
        history.tool(&ToolOutput::text("ok"), "FindTool", None);
        let messages = history.get();
        assert_eq!(messages[1]["role"], "assistant");
        assert!(messages[1]["content"]
            .as_str()
            .unwrap()
            .starts_with(TRIM_NOTE_PREFIX));
        let roles: Vec<_> = messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(
            roles,
            ["system", "assistant", "assistant", "tool", "assistant", "tool"]
        );
    }

    #[test]
    fn tool_history_cap_counts_tool_results_not_entries() {
        let mut history = ChatHistory::new("sys", Backend::Ollama, Some(2));

        // one round with three parallel calls: 3 tool results exceed the
        // cap of 2, so the whole round is dropped — the cap counts tool
        // results, not messages
        history.assistant(&response(
            "",
            vec![tool_call("ReadTool"), tool_call("GrepTool"), tool_call("FindTool")],
        ));
        for name in ["ReadTool", "GrepTool", "FindTool"] {
            history.tool(&ToolOutput::text("ok"), name, None);
        }
        assert!(history.get().iter().all(|m| m["role"] != "tool"));

        // the next round starts clean and is kept, behind the trim note
        history.assistant(&response("", vec![tool_call("TimeTool")]));
        history.tool(&ToolOutput::text("ok"), "TimeTool", None);
        let messages = history.get();
        assert!(messages[1]["content"]
            .as_str()
            .unwrap()
            .starts_with(TRIM_NOTE_PREFIX));
        let roles: Vec<_> = messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["system", "assistant", "assistant", "tool"]);
    }

    #[test]
    fn tool_history_cap_drops_flushed_attachments_with_their_round() {
        let mut history = ChatHistory::new("sys", Backend::OpenAI, Some(1));

        // round 1: a tool result plus its attachment-flush user message
        history.assistant(&response("", vec![tool_call("ShotTool")]));
        history.tool(
            &ToolOutput { text: "see".to_string(), attachments: vec![image()] },
            "ShotTool",
            None,
        );
        history.flush_tool_attachments();
        // system + assistant + tool + the flush user message
        assert_eq!(history.get().len(), 4);

        // round 2 overflows the cap → round 1 disappears entirely,
        // including the flush message, leaving the trim note plus a
        // clean pair
        history.assistant(&response("", vec![tool_call("ReadTool")]));
        history.tool(&ToolOutput::text("ok"), "ReadTool", None);
        let messages = history.get();
        assert!(messages[1]["content"]
            .as_str()
            .unwrap()
            .starts_with(TRIM_NOTE_PREFIX));
        let roles: Vec<_> = messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["system", "assistant", "assistant", "tool"]);
    }

    #[test]
    fn tool_history_cap_never_strands_a_tool_result() {
        let mut history = ChatHistory::new("sys", Backend::Ollama, Some(1));

        // rounds with interleaved assistant notes (as the round-cap
        // notice produces) — trimming must still keep every tool result
        // paired with its assistant tool_calls message
        for name in ["ReadTool", "GrepTool", "FindTool"] {
            history.assistant(&response("", vec![tool_call(name)]));
            history.tool(&ToolOutput::text("ok"), name, None);
            history.assistant_note("note");
        }
        let messages = history.get();
        // system + trim note + one round (assistant + tool) + its note
        assert_eq!(messages.len(), 5);
        assert!(messages[1]["content"]
            .as_str()
            .unwrap()
            .starts_with(TRIM_NOTE_PREFIX));
        assert_eq!(messages[2]["role"], "assistant");
        assert!(messages[2].get("tool_calls").is_some());
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[4]["role"], "assistant");
        assert!(messages[4].get("tool_calls").is_none());
    }

    #[test]
    fn no_cap_keeps_the_tool_round_unbounded() {
        // `max_tool_calls: null` in the config means no trimming: every
        // round stays in context, no trim note is ever inserted
        let mut history = ChatHistory::new("sys", Backend::Ollama, None);
        for name in ["ReadTool", "GrepTool", "FindTool", "TimeTool"] {
            history.assistant(&response("", vec![tool_call(name)]));
            history.tool(&ToolOutput::text("ok"), name, None);
        }
        let messages = history.get();
        // system + four rounds of two messages each — nothing dropped
        assert_eq!(messages.len(), 9);
        assert!(!messages.iter().any(|m| {
            m["content"]
                .as_str()
                .is_some_and(|c| c.starts_with(TRIM_NOTE_PREFIX))
        }));
    }

    #[test]
    fn strip_attachments_removes_media_and_notes_the_model() {
        // OpenAI: the content parts collapse back to the plain prompt,
        // with a note naming the dropped modality
        let mut history = ChatHistory::new("sys", Backend::OpenAI, None);
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
        let mut history = ChatHistory::new("sys", Backend::Ollama, None);
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
