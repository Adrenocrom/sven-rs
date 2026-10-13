//! Server-Sent Events parsing, shared by the chat backends and the
//! MCP HTTP transport (one parser instead of two — the chat one used
//! to drop legal frames: `data:` without a space after the colon, and
//! payloads split across several `data:` lines).
//!
//! The framing implemented here, per the SSE spec (WHATWG HTML
//! §9.2.6):
//!
//! - an event is a run of field lines ended by a blank line
//! - the `data:` lines of an event are joined with `\n` into its payload
//! - exactly one optional space after the colon is stripped
//! - `event:`, `id:`, `retry:` and `:comment` lines are ignored
//!
//! The chat path needs each event the moment it completes — tokens are
//! printed while the stream runs — so the parser is incremental:
//! `SseParser::line` takes lines as they arrive and returns a payload
//! whenever an event ends. `parse_sse` is the whole-body convenience
//! the MCP transport uses on a response it has already buffered.

use serde_json::Value;

/// Incremental SSE parser: feed lines (without their line ending),
/// get the joined `data:` payload of each event as it completes.
#[derive(Default)]
pub struct SseParser {
    /// the `data:` lines of the event currently being assembled
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one line. `Some(payload)` means this line ended an event —
    /// its `data:` fields joined with `\n`. `None` means the line was
    /// part of an event still being assembled (a `data:` line, a
    /// comment, an `event:`/`id:`/`retry:` field) or a blank line with
    /// nothing pending.
    pub fn line(&mut self, line: &str) -> Option<String> {
        if line.is_empty() {
            // a blank line ends the event; no data lines means there
            // was no event (the spec dispatches nothing)
            if self.data.is_empty() {
                return None;
            }
            return Some(self.take());
        }
        if let Some(payload) = line.strip_prefix("data:") {
            // one optional space after the colon per the spec
            self.data
                .push(payload.strip_prefix(' ').unwrap_or(payload).to_string());
        }
        // everything else — `event:`, `id:`, `retry:`, `:comments` —
        // is ignored
        None
    }

    /// Emit the event still pending when a stream ended without the
    /// blank line that would have closed it.
    pub fn flush(&mut self) -> Option<String> {
        if self.data.is_empty() {
            None
        } else {
            Some(self.take())
        }
    }

    fn take(&mut self) -> String {
        std::mem::take(&mut self.data).join("\n")
    }
}

/// Parse a complete SSE body into the JSON messages of its events.
/// Payloads that are not JSON are skipped — a frame cut off by a
/// timeout is noise, not an error.
pub fn parse_sse(body: &str) -> Vec<Value> {
    let mut parser = SseParser::new();
    let mut messages = Vec::new();
    for line in body.lines() {
        if let Some(payload) = parser.line(line) {
            push(&payload, &mut messages);
        }
    }
    if let Some(payload) = parser.flush() {
        push(&payload, &mut messages);
    }
    messages
}

fn push(payload: &str, messages: &mut Vec<Value>) {
    if let Ok(message) = serde_json::from_str::<Value>(payload) {
        messages.push(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_sse_frames() {
        let body = "event: message\ndata: {\"id\":1}\n\ndata: {\"id\":2}\n\n";
        let messages = parse_sse(body);
        assert_eq!(messages, vec![json!({"id": 1}), json!({"id": 2})]);

        // multi-line data is joined with newlines (JSON tolerates the
        // whitespace between the tokens)
        let messages = parse_sse("data: {\"a\":\ndata: 1}\n\n");
        assert_eq!(messages, vec![json!({"a": 1})]);

        // comments and non-data lines are ignored, a trailing frame cut
        // off mid-JSON is skipped
        assert!(parse_sse(": keepalive\ndata: {\"id\"").is_empty());
        assert!(parse_sse("data: not json\n\n").is_empty());
    }

    #[test]
    fn incremental_parser_emits_events_on_blank_lines() {
        let mut parser = SseParser::new();
        // data without a space after the colon is legal
        assert_eq!(parser.line(r#"data:{"id":1}"#), None);
        assert_eq!(parser.line(""), Some(r#"{"id":1}"#.to_string()));
        // comments and field lines do not become data
        assert_eq!(parser.line(": keep-alive"), None);
        assert_eq!(parser.line("event: message"), None);
        // the data lines of one event are joined with a newline
        assert_eq!(parser.line("data: {\"a\":"), None);
        assert_eq!(parser.line("data: 1}"), None);
        assert_eq!(parser.line(""), Some("{\"a\":\n1}".to_string()));
        // a blank line with nothing pending emits nothing
        assert_eq!(parser.line(""), None);
    }

    #[test]
    fn flush_emits_a_trailing_event_without_blank_line() {
        let mut parser = SseParser::new();
        assert_eq!(parser.line("data: {\"id\":1}"), None);
        assert_eq!(parser.flush(), Some(r#"{"id":1}"#.to_string()));
        assert_eq!(parser.flush(), None);
    }
}