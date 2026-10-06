//! Transports for the MCP client.
//!
//! Two of the MCP transports are supported:
//!
//! - **stdio** — the server is a child process, spoken to with
//!   newline-delimited JSON on its stdin/stdout. The process stays
//!   alive for the whole session and is killed when the transport
//!   drops.
//! - **Streamable HTTP** — every JSON-RPC message is its own POST,
//!   sent with the async `reqwest::Client` the chat backends already
//!   use. No `curl` subprocess, and no `reqwest::blocking` — its
//!   client panics inside a tokio runtime.
//!
//! The deprecated HTTP+SSE transport (a GET stream before the first
//! POST) is not supported.

use std::collections::VecDeque;
use std::error::Error;
use std::future::Future;
use std::io::{BufRead, BufReader};
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

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

/// The future the transport methods return — the same desugaring as
/// `ToolFuture` (see `tool.rs`): the trait is used as
/// `Box<dyn Transport>`, and native `async fn` in traits is not
/// dyn-compatible. `Send` because these futures are awaited inside the
/// `+ Send` futures `Tool::execute` returns.
pub type TransportFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, Box<dyn Error>>> + Send + 'a>>;

/// One JSON-RPC message channel to an MCP server.
pub trait Transport: Send {
    /// Send one message (request or notification). For transports where
    /// sending covers the whole exchange, `timeout` bounds it.
    fn send<'a>(&'a mut self, message: &'a Value, timeout: Duration) -> TransportFuture<'a, ()>;
    /// Receive the next message, waiting at most `timeout`. `Ok(None)`
    /// means the server closed the connection.
    fn recv<'a>(&'a mut self, timeout: Duration) -> TransportFuture<'a, Option<Value>>;
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
/// The child is spawned with `std::process` so its stdout stays a plain
/// std handle for the reader thread (tokio's `ChildStdout` cannot be
/// converted back); only the stdin is registered with the runtime via
/// `ChildStdin::from_std` for the async writes.
///
/// A reader thread moves stdout lines into a channel so `recv` can wait
/// with a timeout — a plain blocking read could not be interrupted when
/// a server hangs, and the agent would freeze forever. The channel is
/// tokio's, so `recv` can be awaited.
pub struct StdioTransport {
    child: Child,
    stdin: Option<tokio::process::ChildStdin>,
    lines: UnboundedReceiver<String>,
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
        // register the std handle with the runtime for async writes;
        // requires the runtime context `McpClient::connect` runs in
        let stdin = tokio::process::ChildStdin::from_std(stdin)?;
        let (sender, receiver) = unbounded_channel();
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
    fn send<'a>(&'a mut self, message: &'a Value, _timeout: Duration) -> TransportFuture<'a, ()> {
        Box::pin(async move {
            let stdin = self.stdin.as_mut().ok_or("the server's stdin is closed")?;
            // JSON serialization escapes newlines, so every message is a
            // single line — exactly the framing the stdio transport needs
            stdin
                .write_all(serde_json::to_string(message)?.as_bytes())
                .await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
            Ok(())
        })
    }

    fn recv<'a>(&'a mut self, timeout: Duration) -> TransportFuture<'a, Option<Value>> {
        Box::pin(async move {
            // one deadline for the whole call: non-JSON noise on the
            // server's stdout must not reset the clock
            let deadline = Instant::now() + timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err("no message within the timeout".into());
                }
                match tokio::time::timeout(remaining, self.lines.recv()).await {
                    Ok(Some(line)) => match serde_json::from_str(line.trim()) {
                        // servers occasionally print non-JSON noise to
                        // stdout; skip it instead of failing the request
                        Ok(message) => return Ok(Some(message)),
                        Err(_) => continue,
                    },
                    // every sender is gone: the reader thread saw EOF
                    // and the queue is drained
                    Ok(None) => return Ok(None),
                    Err(_elapsed) => return Err("no message within the timeout".into()),
                }
            }
        })
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
        // `kill()` is sync on std's Child; the loop below reaps the exit
        // so no zombie is left behind.
        let _ = self.child.kill();
        for _ in 0..20 {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => return,
            }
        }
    }
}

// -------------------------------------------------------- HTTP transport

/// Streamable HTTP transport: every JSON-RPC message is one POST via
/// the async `reqwest::Client`.
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
    /// Built once and reused: the client pools connections.
    client: reqwest::Client,
    session_id: Option<String>,
    protocol_version: Option<String>,
    pending: VecDeque<Value>,
}

impl HttpTransport {
    pub fn new(config: &McpServerConfig) -> Result<Self, Box<dyn Error>> {
        let Some(url) = &config.url else {
            return Err("the streamable HTTP transport needs a 'url'".into());
        };
        // Only http(s): reqwest would otherwise happily fetch file:// and
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
        // no redirect following, like curl before: a redirect must not
        // carry the configured (possibly authenticated) headers to
        // another origin
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("could not build the http client: {}", e))?;
        Ok(Self {
            url: url.clone(),
            headers,
            client,
            session_id: None,
            protocol_version: None,
            pending: VecDeque::new(),
        })
    }

    /// POST one message and queue whatever it answered.
    async fn post(&mut self, message: &Value, timeout: Duration) -> Result<(), Box<dyn Error>> {
        // curl's `--max-time` covered connect→body-end from one start
        // point; the deadline keeps those total semantics for the body
        // read below.
        let deadline = Instant::now() + timeout;

        let mut request = self.client
            .post(&self.url)
            .header("Content-Type", "application/json")
            // required by the Streamable HTTP spec: the server may
            // answer with JSON or with an SSE stream
            .header("Accept", "application/json, text/event-stream")
            // bounds connect + request + response headers; the manual
            // deadline below is the deterministic bound for the body
            .timeout(timeout);
        if let Some(session_id) = &self.session_id {
            request = request.header("Mcp-Session-Id", session_id);
        }
        if let Some(version) = &self.protocol_version {
            request = request.header("MCP-Protocol-Version", version);
        }
        for (name, value) in &self.headers {
            request = request.header(name.as_str(), value.as_str());
        }

        let mut response = match request.body(serde_json::to_string(message)?).send().await {
            Ok(response) => response,
            // the timeout fired before any headers arrived
            Err(e) if e.is_timeout() => return Err("no answer within the timeout".into()),
            Err(e) => return Err(e.into()),
        };

        let status = response.status().as_u16();
        if status == 404 && self.session_id.is_some() {
            // the server terminated the session (it may do so at any
            // time); per the spec the client must start a new one.
            // Messages of the dead session must not leak into it.
            self.session_id = None;
            self.pending.clear();
            return Err(Box::new(SessionExpired));
        }

        // whatever the body carried before the deadline is kept — a
        // server that answered but kept its SSE stream open (the spec
        // says it SHOULD close it) still delivers its messages
        let (body, timed_out) = read_bounded(&mut response, deadline).await?;

        if !(200..300).contains(&status) {
            return Err(format!(
                "HTTP {} from the mcp server: {}",
                status,
                shorten(&body, 500)
            )
            .into());
        }
        if let Some(session_id) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            self.session_id = Some(session_id.to_string());
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let received = self.pending.len();
        if content_type.contains("text/event-stream") {
            self.pending.extend(parse_sse(&body));
        } else if !body.trim().is_empty() {
            // 202 Accepted (the answer to a notification) has no body
            self.pending.push_back(serde_json::from_str(body.trim())?);
        }
        if timed_out && self.pending.len() == received {
            return Err("no answer within the timeout".into());
        }
        Ok(())
    }
}

/// Read the response body until EOF or the deadline. Mirrors curl's
/// `--max-time` + exit 28: whatever arrived before the cut is returned,
/// and only the caller decides whether it is enough.
async fn read_bounded(
    response: &mut reqwest::Response,
    deadline: Instant,
) -> Result<(String, bool), Box<dyn Error>> {
    let mut body = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok((String::from_utf8_lossy(&body).into_owned(), true));
        }
        match tokio::time::timeout(remaining, response.chunk()).await {
            Ok(Ok(Some(bytes))) => body.extend_from_slice(&bytes),
            // EOF: the body is complete
            Ok(Ok(None)) => return Ok((String::from_utf8_lossy(&body).into_owned(), false)),
            // reqwest's own request timeout cutting the stream counts
            // as the deadline, not as a failure — the bytes collected
            // so far may still contain the answer
            Ok(Err(e)) if e.is_timeout() => {
                return Ok((String::from_utf8_lossy(&body).into_owned(), true))
            }
            Ok(Err(e)) => return Err(e.into()),
            Err(_elapsed) => return Ok((String::from_utf8_lossy(&body).into_owned(), true)),
        }
    }
}

impl Transport for HttpTransport {
    fn send<'a>(&'a mut self, message: &'a Value, timeout: Duration) -> TransportFuture<'a, ()> {
        Box::pin(self.post(message, timeout))
    }

    fn recv<'a>(&'a mut self, _timeout: Duration) -> TransportFuture<'a, Option<Value>> {
        // the POST already completed inside `send`; whatever it carried
        // is queued and returned immediately
        Box::pin(async move { Ok(self.pending.pop_front()) })
    }

    fn negotiated(&mut self, version: &str) {
        self.protocol_version = Some(version.to_string());
    }
}

// -------------------------------------------------------------- parsing

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
    fn send<'a>(&'a mut self, message: &'a Value, _timeout: Duration) -> TransportFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.state.lock().unwrap();
            state.sent.push(message.clone());
            if state.session_expired_once {
                state.session_expired_once = false;
                let err: Box<dyn Error> = Box::new(SessionExpired);
                return Err(err);
            }
            Ok(())
        })
    }
    fn recv<'a>(&'a mut self, _timeout: Duration) -> TransportFuture<'a, Option<Value>> {
        Box::pin(async move { Ok(self.state.lock().unwrap().incoming.pop_front()) })
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