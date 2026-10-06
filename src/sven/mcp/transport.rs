//! Transports for the MCP client.
//!
//! Two of the MCP transports are supported:
//!
//! - **stdio** — the server is a child process, spoken to with
//!   newline-delimited JSON on its stdin/stdout. The process stays
//!   alive for the whole session and is killed when the transport
//!   drops.
//! - **Streamable HTTP** — every JSON-RPC message is its own POST,
//!   executed with `curl` (the same approach `WebFetch` takes: no HTTP
//!   client dependency, and blocking subprocesses fit the synchronous
//!   `Tool::execute`).
//!
//! The deprecated HTTP+SSE transport (a GET stream before the first
//! POST) is not supported.

use std::collections::VecDeque;
use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::sven::config::McpServerConfig;

/// Time budget for `initialize` and `tools/list`. Generous on purpose:
/// `npx`/`uvx`-based servers download their package on the first start.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(120);
/// Time budget for one `tools/call`.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Time budget for sending a notification. Notifications get no answer,
/// so this only bounds a server that answers with an SSE stream it
/// never closes.
pub const NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(30);

/// One JSON-RPC message channel to an MCP server.
pub trait Transport: Send {
    /// Send one message (request or notification). For transports where
    /// sending covers the whole exchange, `timeout` bounds it.
    fn send(&mut self, message: &Value, timeout: Duration) -> Result<(), Box<dyn Error>>;
    /// Receive the next message, waiting at most `timeout`. `Ok(None)`
    /// means the server closed the connection.
    fn recv(&mut self, timeout: Duration) -> Result<Option<Value>, Box<dyn Error>>;
    /// Report the protocol version negotiated during `initialize`; the
    /// HTTP transport echoes it as `MCP-Protocol-Version` on every later
    /// request, the stdio transport has no use for it.
    fn negotiated(&mut self, _version: &str) {}
}

/// What `HttpTransport::send` raises when the server answers 404 to a
/// request that carried a session id: the server terminated the
/// session, which it may do at any time (restart, idle timeout). Per
/// the Streamable HTTP spec the client must then start a new session —
/// `McpClient::request` catches this error, runs `initialize` again
/// and retries the request once.
#[derive(Debug)]
pub struct SessionExpired;

impl std::fmt::Display for SessionExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the server terminated the session (HTTP 404)")
    }
}

impl Error for SessionExpired {}

// ------------------------------------------------------- stdio transport

/// stdio transport: requests are newline-delimited JSON on the server's
/// stdin, responses arrive on its stdout.
///
/// A reader thread moves stdout lines into a channel so `recv` can wait
/// with a timeout — a plain blocking read could not be interrupted when
/// a server hangs, and the agent would freeze forever.
pub struct StdioTransport {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl StdioTransport {
    pub fn spawn(config: &McpServerConfig) -> Result<Self, Box<dyn Error>> {
        let Some(command) = &config.command else {
            return Err("the stdio transport needs a 'command'".into());
        };
        let mut child = Command::new(command)
            .args(config.args.iter().flatten())
            // extra variables on top of the inherited environment
            .envs(config.env.iter().flatten())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // server logs go to stderr per the MCP spec; inheriting them
            // keeps server failures visible in the terminal
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("could not start '{}': {}", command, e))?;
        let stdin = child
            .stdin
            .take()
            .ok_or("could not capture the server's stdin")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("could not capture the server's stdout")?;
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    // 0 bytes = EOF: the server exited, the thread ends
                    // and `recv` reports the closed connection
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if sender.send(line.clone()).is_err() {
                            break; // the transport was dropped
                        }
                    }
                }
            }
        });
        Ok(Self {
            child,
            stdin: Some(stdin),
            lines: receiver,
        })
    }
}

impl Transport for StdioTransport {
    fn send(&mut self, message: &Value, _timeout: Duration) -> Result<(), Box<dyn Error>> {
        let stdin = self.stdin.as_mut().ok_or("the server's stdin is closed")?;
        // JSON serialization escapes newlines, so every message is a
        // single line — exactly the framing the stdio transport needs
        stdin.write_all(serde_json::to_string(message)?.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Option<Value>, Box<dyn Error>> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("no message within the timeout".into());
            }
            match self.lines.recv_timeout(remaining) {
                Ok(line) => match serde_json::from_str(line.trim()) {
                    // servers occasionally print non-JSON noise to
                    // stdout; skip it instead of failing the request
                    Ok(message) => return Ok(Some(message)),
                    Err(_) => continue,
                },
                Err(RecvTimeoutError::Timeout) => {
                    return Err("no message within the timeout".into());
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Closing stdin is the MCP shutdown signal; give the server a
        // second to exit on its own, then kill it so no orphan remains.
        self.stdin.take();
        for _ in 0..20 {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// -------------------------------------------------------- HTTP transport

/// Streamable HTTP transport: every JSON-RPC message is one POST,
/// executed with `curl` like `WebFetch` does.
///
/// The response is either a plain JSON object or an SSE stream whose
/// `data:` frames carry the messages; both are parsed into a queue that
/// `recv` drains. The `Mcp-Session-Id` header of the initialize response
/// is captured and echoed on every later request. A 404 answering a
/// request that carried the session id means the server terminated the
/// session; `send` then raises `SessionExpired` so the client can start
/// a new one.
pub struct HttpTransport {
    url: String,
    headers: Vec<(String, String)>,
    session_id: Option<String>,
    protocol_version: Option<String>,
    pending: VecDeque<Value>,
}

impl HttpTransport {
    pub fn new(config: &McpServerConfig) -> Result<Self, Box<dyn Error>> {
        let Some(url) = &config.url else {
            return Err("the streamable HTTP transport needs a 'url'".into());
        };
        // Only http(s): curl would otherwise happily fetch file:// and
        // read local files (the same check `WebFetch` makes).
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(format!("unsupported URL scheme (only http/https): {}", url).into());
        }
        let mut headers = Vec::new();
        for (name, value) in config.headers.iter().flatten() {
            // a ':' in the name or control characters in either half
            // could smuggle a second header line into the request
            if name.is_empty()
                || name.contains(':')
                || name.chars().any(char::is_control)
                || value.chars().any(char::is_control)
            {
                return Err(format!("invalid header '{}'", name).into());
            }
            headers.push((name.clone(), value.clone()));
        }
        Ok(Self {
            url: url.clone(),
            headers,
            session_id: None,
            protocol_version: None,
            pending: VecDeque::new(),
        })
    }

    /// POST one message and return curl's output (response headers
    /// followed by the body) and whether curl hit its timeout.
    fn curl(&self, message: &Value, timeout: Duration) -> Result<(String, bool), Box<dyn Error>> {
        let mut curl = Command::new("curl");
        curl.arg("-sS")
            .arg("--max-time")
            .arg(timeout.as_secs().to_string())
            .arg("-X")
            .arg("POST")
            .arg("-H")
            .arg("Content-Type: application/json")
            // required by the Streamable HTTP spec: the server may
            // answer with JSON or with an SSE stream
            .arg("-H")
            .arg("Accept: application/json, text/event-stream")
            // curl adds `Expect: 100-continue` to bodies over 1 MB;
            // HTTP/1.1 servers then send an interim "100 Continue"
            // response before the real one, which would land in the
            // header dump and hide the actual status. An empty value
            // removes the header — curl's documented suppression.
            .arg("-H")
            .arg("Expect:")
            // response headers first, then the body, both on stdout
            .arg("-D")
            .arg("-")
            // the body comes from stdin: no argv length limit, and no
            // way for message content to turn into a curl option
            .arg("--data-binary")
            .arg("@-");
        if let Some(session_id) = &self.session_id {
            curl.arg("-H").arg(format!("Mcp-Session-Id: {}", session_id));
        }
        if let Some(version) = &self.protocol_version {
            curl.arg("-H").arg(format!("MCP-Protocol-Version: {}", version));
        }
        for (name, value) in &self.headers {
            curl.arg("-H").arg(format!("{}: {}", name, value));
        }
        let mut child = curl
            .arg("--")
            .arg(&self.url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdin = child.stdin.take().ok_or("curl stdin was not piped")?;
        stdin.write_all(serde_json::to_string(message)?.as_bytes())?;
        stdin.flush()?;
        drop(stdin); // EOF: curl sends the request
        let output = child.wait_with_output()?;

        // exit 28 is curl's timeout: the data received so far is still
        // parsed by the caller — a server that answered but kept its
        // SSE stream open would otherwise turn every call into a
        // timeout
        let timed_out = output.status.code() == Some(28);
        if !output.status.success() && !timed_out {
            return Err(format!(
                "curl exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        Ok((
            String::from_utf8_lossy(&output.stdout).into_owned(),
            timed_out,
        ))
    }
}

impl Transport for HttpTransport {
    fn send(&mut self, message: &Value, timeout: Duration) -> Result<(), Box<dyn Error>> {
        let (output, timed_out) = self.curl(message, timeout)?;
        let (headers, body) = split_headers(&output);
        let status = status_code(headers).ok_or("could not parse the HTTP status line")?;
        if status == 404 && self.session_id.is_some() {
            // the server terminated the session (it may do so at any
            // time); per the spec the client must start a new one.
            // Messages of the dead session must not leak into it.
            self.session_id = None;
            self.pending.clear();
            return Err(Box::new(SessionExpired));
        }
        if !(200..300).contains(&status) {
            return Err(format!(
                "HTTP {} from the mcp server: {}",
                status,
                shorten(body, 500)
            )
            .into());
        }
        if let Some(session_id) = header_value(headers, "mcp-session-id") {
            self.session_id = Some(session_id.to_string());
        }
        let content_type = header_value(headers, "content-type")
            .unwrap_or("")
            .to_ascii_lowercase();
        let received = self.pending.len();
        if content_type.contains("text/event-stream") {
            self.pending.extend(parse_sse(body));
        } else if !body.trim().is_empty() {
            // 202 Accepted (the answer to a notification) has no body
            self.pending.push_back(serde_json::from_str(body.trim())?);
        }
        if timed_out && self.pending.len() == received {
            return Err("no answer within the timeout".into());
        }
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> Result<Option<Value>, Box<dyn Error>> {
        // the POST already completed inside `send`; whatever it carried
        // is queued and returned immediately
        Ok(self.pending.pop_front())
    }

    fn negotiated(&mut self, version: &str) {
        self.protocol_version = Some(version.to_string());
    }
}

// -------------------------------------------------------------- parsing

/// Split curl's combined `-D -` output into the header block and the
/// body. curl terminates the headers with a blank line; HTTP/2 dumps use
/// CRLF like HTTP/1.1, but LF-only is accepted too. Interim 1xx blocks
/// (an "HTTP/1.1 100 Continue" a server sends before the real answer
/// when it honors `Expect: 100-continue`) are skipped.
fn split_headers(output: &str) -> (&str, &str) {
    let mut rest = output;
    loop {
        let (headers, body) = if let Some(pos) = rest.find("\r\n\r\n") {
            (&rest[..pos], &rest[pos + 4..])
        } else if let Some(pos) = rest.find("\n\n") {
            (&rest[..pos], &rest[pos + 2..])
        } else {
            ("", rest)
        };
        match status_code(headers) {
            Some(status) if (100..200).contains(&status) => rest = body,
            _ => return (headers, body),
        }
    }
}

/// Status code from the first header line ("HTTP/1.1 200 OK",
/// "HTTP/2 404").
fn status_code(headers: &str) -> Option<u16> {
    headers
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Value of a header, case-insensitively.
fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// Parse an SSE body into the JSON messages of its `data:` frames.
/// Events end at blank lines; multi-line `data:` fields are joined with
/// newlines per the SSE spec; a trailing frame that does not parse (a
/// stream cut off by the timeout) is skipped.
fn parse_sse(body: &str) -> Vec<Value> {
    fn flush(data: &mut Vec<String>, messages: &mut Vec<Value>) {
        if data.is_empty() {
            return;
        }
        let joined = std::mem::take(data).join("\n");
        if let Ok(message) = serde_json::from_str::<Value>(&joined) {
            messages.push(message);
        }
    }
    let mut messages = Vec::new();
    let mut data: Vec<String> = Vec::new();
    for line in body.lines() {
        if line.is_empty() {
            flush(&mut data, &mut messages);
        } else if let Some(payload) = line.strip_prefix("data:") {
            // one optional space after the colon per the SSE spec
            data.push(payload.strip_prefix(' ').unwrap_or(payload).to_string());
        }
        // `event:`, `id:`, `retry:` lines and `:comments` are ignored
    }
    flush(&mut data, &mut messages);
    messages
}

/// Cap an error body so a misconfigured URL pointing at a huge page
/// cannot flood the terminal.
fn shorten(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        text.chars().take(max_chars).collect()
    }
}

// ----------------------------------------------------------------- mock

/// A scripted transport for tests: `recv` pops from `incoming`, `send`
/// records into `sent`. Shared through an `Arc` so a test can keep
/// inspecting the state after moving the transport into a `McpClient`.
#[cfg(test)]
pub(crate) struct MockTransport {
    pub(crate) state: std::sync::Arc<std::sync::Mutex<MockState>>,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct MockState {
    pub(crate) incoming: VecDeque<Value>,
    pub(crate) sent: Vec<Value>,
    pub(crate) negotiated: Option<String>,
    /// When set, the next `send` fails with `SessionExpired` and the
    /// flag clears itself — a server that terminated the HTTP session.
    pub(crate) session_expired_once: bool,
}

#[cfg(test)]
impl Transport for MockTransport {
    fn send(&mut self, message: &Value, _timeout: Duration) -> Result<(), Box<dyn Error>> {
        let mut state = self.state.lock().unwrap();
        state.sent.push(message.clone());
        if state.session_expired_once {
            state.session_expired_once = false;
            return Err(Box::new(SessionExpired));
        }
        Ok(())
    }
    fn recv(&mut self, _timeout: Duration) -> Result<Option<Value>, Box<dyn Error>> {
        Ok(self.state.lock().unwrap().incoming.pop_front())
    }
    fn negotiated(&mut self, version: &str) {
        self.state.lock().unwrap().negotiated = Some(version.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn splits_the_curl_header_dump_from_the_body() {
        let output = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"a\":1}";
        let (headers, body) = split_headers(output);
        assert_eq!(status_code(headers), Some(200));
        assert_eq!(header_value(headers, "content-type"), Some("application/json"));
        assert_eq!(body, "{\"a\":1}");

        let (headers, body) = split_headers("HTTP/2 404 Not Found\nX: y\n\nbody");
        assert_eq!(status_code(headers), Some(404));
        assert_eq!(body, "body");
    }

    #[test]
    fn skips_interim_100_continue_responses() {
        // a server honoring `Expect: 100-continue` sends an interim 100
        // block before the real answer; only the real one counts
        let output = concat!(
            "HTTP/1.1 100 Continue\r\n\r\n",
            "HTTP/1.1 200 OK\r\n",
            "Content-Type: application/json\r\n",
            "\r\n",
            "{\"a\":1}"
        );
        let (headers, body) = split_headers(output);
        assert_eq!(status_code(headers), Some(200));
        assert_eq!(header_value(headers, "content-type"), Some("application/json"));
        assert_eq!(body, "{\"a\":1}");

        // LF-only dumps are accepted too
        let (headers, body) = split_headers("HTTP/1.1 100 Continue\n\nHTTP/2 201\n\ncreated");
        assert_eq!(status_code(headers), Some(201));
        assert_eq!(body, "created");
    }

    #[test]
    fn header_lookup_is_case_insensitive_and_trims() {
        let headers = "HTTP/1.1 200\r\nMcp-Session-Id: abc123 \r\n";
        assert_eq!(header_value(headers, "mcp-session-id"), Some("abc123"));
        assert_eq!(header_value(headers, "missing"), None);
    }

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
}