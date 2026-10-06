//! A minimal MCP (Model Context Protocol) client — no SDK, just the
//! JSON-RPC 2.0 messages sven needs: `initialize`, `tools/list` and
//! `tools/call`. The tools a server reports are exposed to the model
//! like built-in tools, under the name `mcp__<server>__<tool>`.

pub mod transport;

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::sven::config::McpServerConfig;
use crate::sven::mcp::transport::{
    CALL_TIMEOUT, HANDSHAKE_TIMEOUT, NOTIFICATION_TIMEOUT, HttpTransport, SessionExpired,
    StdioTransport, Transport,
};
use crate::sven::tools::mcp_tool::McpTool;

/// The protocol revision sven asks for. `tools/list` and `tools/call`
/// are identical in every published revision, so a server answering
/// with a different version is accepted with a warning instead of
/// dropping the connection.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// One tool of a server, as reported by `tools/list`.
#[derive(Debug)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// What an incoming JSON-RPC message is: the answer to one of our
/// requests, a request from the server (which must be answered, even if
/// only with an error), or a notification (which must not be).
enum MessageKind {
    Response(Value),
    Request(Value),
    Notification,
}

fn classify(message: &Value) -> MessageKind {
    match (message.get("id"), message.get("method")) {
        (Some(_), Some(_)) => MessageKind::Request(message["id"].clone()),
        (Some(_), None) => MessageKind::Response(message["id"].clone()),
        (None, _) => MessageKind::Notification,
    }
}

/// A connected MCP server.
pub struct McpClient {
    server: String,
    transport: Box<dyn Transport>,
    next_id: u64,
}

impl McpClient {
    /// Build the transport the config asks for and run the `initialize`
    /// handshake.
    pub fn connect(name: &str, config: &McpServerConfig) -> Result<Self, Box<dyn Error>> {
        let transport: Box<dyn Transport> = match (&config.command, &config.url) {
            (Some(_), Some(_)) => {
                return Err("an MCP server needs either 'command' or 'url', not both".into());
            }
            (Some(_), None) => Box::new(StdioTransport::spawn(config)?),
            (None, Some(_)) => Box::new(HttpTransport::new(config)?),
            (None, None) => {
                return Err(
                    "an MCP server needs 'command' (stdio) or 'url' (streamable HTTP)".into(),
                );
            }
        };
        let mut client = Self::with_transport(name, transport);
        client.initialize()?;
        Ok(client)
    }

    pub fn with_transport(name: &str, transport: Box<dyn Transport>) -> Self {
        Self {
            server: name.to_string(),
            transport,
            next_id: 0,
        }
    }

    /// The `initialize` handshake: announce sven, remember the
    /// negotiated protocol version, then confirm with
    /// `notifications/initialized`. Also used to start a fresh session
    /// after the server terminated the old one (see `request`).
    ///
    /// It goes through `request_once`, not `request`: the retry wrapper
    /// re-initializes on `SessionExpired`, so an initialize that itself
    /// hit a session expiry would recurse forever.
    pub fn initialize(&mut self) -> Result<(), Box<dyn Error>> {
        let result = self.request_once(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                // sven supports no server→client capabilities (no
                // sampling, no roots, no elicitation)
                "capabilities": {},
                "clientInfo": {"name": "sven-rs", "version": env!("CARGO_PKG_VERSION")}
            }),
            HANDSHAKE_TIMEOUT,
        )?;
        if let Some(version) = result.get("protocolVersion").and_then(Value::as_str) {
            if version != PROTOCOL_VERSION {
                eprintln!(
                    "mcp '{}': server speaks protocol version {} (sven asked for {}); continuing",
                    self.server, version, PROTOCOL_VERSION
                );
            }
            self.transport.negotiated(version);
        }
        self.notify("notifications/initialized");
        Ok(())
    }

    /// All tools the server offers, following `nextCursor` pagination.
    pub fn list_tools(&mut self) -> Result<Vec<McpToolInfo>, Box<dyn Error>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = BTreeSet::new();
        loop {
            let mut params = json!({});
            if let Some(cursor) = &cursor {
                params["cursor"] = json!(cursor);
            }
            let result = self.request("tools/list", params, HANDSHAKE_TIMEOUT)?;
            for entry in result
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                match tool_info(entry) {
                    Some(info) => tools.push(info),
                    None => {
                        eprintln!("mcp '{}': skipping a tool without a usable name", self.server)
                    }
                }
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            match &cursor {
                None => return Ok(tools),
                // a server repeating a cursor would loop forever
                Some(cursor) if !seen.insert(cursor.clone()) => {
                    return Err(format!(
                        "mcp server '{}': tools/list pagination repeats the cursor '{}'",
                        self.server, cursor
                    )
                    .into());
                }
                Some(_) => {}
            }
        }
    }

    /// Call one remote tool and render its result as text for the model.
    pub fn call_tool(&mut self, tool: &str, arguments: Value) -> Result<String, Box<dyn Error>> {
        let result = self.request(
            "tools/call",
            json!({"name": tool, "arguments": arguments}),
            CALL_TIMEOUT,
        )?;
        format_tool_result(&result)
    }

    /// Send a notification. It gets no answer, so a failure is only
    /// reported — the next request would fail too if the connection
    /// were really broken.
    fn notify(&mut self, method: &str) {
        if let Err(e) =
            self.transport
                .send(&json!({"jsonrpc": "2.0", "method": method}), NOTIFICATION_TIMEOUT)
        {
            eprintln!("mcp '{}': could not send '{}': {}", self.server, method, e);
        }
    }

    /// Send a request and wait for the answer with the matching id. A
    /// session the server terminated mid-conversation (HTTP 404, which
    /// it may answer at any time) is re-established transparently:
    /// `initialize` runs again and the request is retried once on the
    /// new session. The tool registry is not refreshed afterwards —
    /// the tool set stays as discovered at startup.
    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, Box<dyn Error>> {
        match self.request_once(method, params.clone(), timeout) {
            Err(e) if e.downcast_ref::<SessionExpired>().is_some() => {
                eprintln!(
                    "mcp '{}': {}; starting a new session and retrying once",
                    self.server, e
                );
                self.initialize()?;
                self.request_once(method, params, timeout)
            }
            result => result,
        }
    }

    /// One request/response exchange, without retry. Requests the
    /// server sends while we wait are refused (sven supports none of
    /// them), notifications are dropped — both per the JSON-RPC rules,
    /// so a server never hangs waiting for us.
    fn request_once(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, Box<dyn Error>> {
        self.next_id += 1;
        let id = self.next_id;
        if let Err(e) = self.transport.send(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
            timeout,
        ) {
            // SessionExpired must reach `request` unchanged — it is what
            // triggers the re-initialize there; every other error gets
            // the server name prefixed.
            if e.downcast_ref::<SessionExpired>().is_some() {
                return Err(e);
            }
            return Err(format!("mcp server '{}': {}", self.server, e).into());
        }

        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "mcp server '{}': no answer to '{}' within the timeout",
                    self.server, method
                )
                .into());
            }
            let message = self
                .transport
                .recv(remaining)
                .map_err(|e| format!("mcp server '{}': {}", self.server, e))?;
            let Some(message) = message else {
                return Err(format!(
                    "mcp server '{}': connection closed before '{}' was answered",
                    self.server, method
                )
                .into());
            };
            match classify(&message) {
                MessageKind::Response(response_id) if response_id.as_u64() == Some(id) => {
                    return match message.get("error") {
                        Some(error) => Err(jsonrpc_error(&self.server, error)),
                        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                    };
                }
                MessageKind::Request(request_id) => {
                    // refusing instead of ignoring: a server waiting for
                    // a sampling or roots answer would otherwise hang
                    if let Err(e) = self.transport.send(
                        &json!({
                            "jsonrpc": "2.0",
                            "id": request_id,
                            "error": {
                                "code": -32601,
                                "message": "sven supports no server-to-client requests"
                            }
                        }),
                        timeout,
                    ) {
                        // SessionExpired must stay unwrapped so `request`
                        // can re-initialize; the dead session cannot
                        // answer the main request either
                        if e.downcast_ref::<SessionExpired>().is_some() {
                            return Err(e);
                        }
                        return Err(format!("mcp server '{}': {}", self.server, e).into());
                    }
                }
                // a notification, or an answer to an id we never sent
                _ => {}
            }
        }
    }
}

/// Turn a `tools/list` entry into an `McpToolInfo`; `None` if it has no
/// usable name — the only field the protocol requires.
fn tool_info(entry: &Value) -> Option<McpToolInfo> {
    let name = entry.get("name")?.as_str()?;
    if name.is_empty() {
        return None;
    }
    Some(McpToolInfo {
        name: name.to_string(),
        description: entry
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        input_schema: match entry.get("inputSchema") {
            Some(schema) if schema.is_object() => schema.clone(),
            // a missing or malformed schema becomes "no parameters"
            _ => json!({"type": "object", "properties": {}}),
        },
    })
}

/// Render a `tools/call` result as the text the model sees. Text content
/// passes through; binary content (images, audio) becomes a placeholder
/// — the base64 payload would only flood the context. `isError: true`
/// becomes an error so the agent flags it.
fn format_tool_result(result: &Value) -> Result<String, Box<dyn Error>> {
    let text = content_text(result.get("content"));
    if result.get("isError") == Some(&json!(true)) {
        let message = if text.is_empty() {
            "(no message)".to_string()
        } else {
            text
        };
        return Err(format!("the tool reported an error: {}", message).into());
    }
    if !text.is_empty() {
        return Ok(text);
    }
    if let Some(structured) = result.get("structuredContent") {
        return Ok(structured.to_string());
    }
    if result.get("content").is_none() {
        return Err("malformed tools/call result: no 'content' array".into());
    }
    Ok("(the tool returned no text content)".to_string())
}

fn content_text(content: Option<&Value>) -> String {
    let Some(items) = content.and_then(Value::as_array) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for item in items {
        match item.get("type").and_then(Value::as_str).unwrap_or("unknown") {
            "text" => parts.push(
                item.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
            "image" => parts.push("[image content omitted]".to_string()),
            "audio" => parts.push("[audio content omitted]".to_string()),
            "resource" => parts.push(format!(
                "[embedded resource: {}]",
                item.pointer("/resource/uri")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
            )),
            "resource_link" => parts.push(format!(
                "[resource link: {}]",
                item.get("uri").and_then(Value::as_str).unwrap_or("?")
            )),
            other => parts.push(format!("[unsupported content type: {}]", other)),
        }
    }
    parts.join("\n")
}

/// A JSON-RPC error object as an error, with the server named.
fn jsonrpc_error(server: &str, error: &Value) -> Box<dyn Error> {
    let code = error
        .get("code")
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unknown code".to_string());
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("(no message)");
    match error.get("data") {
        Some(data) => {
            format!("mcp server '{}': {} (code {}, data {})", server, message, code, data)
        }
        None => format!("mcp server '{}': {} (code {})", server, message, code),
    }
    .into()
}

/// Connect to every configured MCP server and wrap its tools. A server
/// that cannot be reached is reported on stderr and skipped — one
/// broken server must not take the whole agent down.
pub fn discover(servers: &BTreeMap<String, McpServerConfig>) -> Vec<McpTool> {
    let mut tools = Vec::new();
    for (name, config) in servers {
        let client = match McpClient::connect(name, config) {
            Ok(client) => client,
            Err(e) => {
                eprintln!("mcp: could not connect to '{}': {}\n", name, e);
                continue;
            }
        };
        let client = Arc::new(Mutex::new(client));
        let infos = match client
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .list_tools()
        {
            Ok(infos) => infos,
            Err(e) => {
                eprintln!("mcp: '{}' connected but tools/list failed: {}\n", name, e);
                continue;
            }
        };
        println!("mcp: '{}' connected ({} tools) \n", name, infos.len());
        for info in &infos {
            tools.push(McpTool::new(name, info, Arc::clone(&client)));
        }
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sven::mcp::transport::{MockState, MockTransport};

    /// A client over a scripted mock, plus the shared state to inspect
    /// what was sent.
    fn mock_client(incoming: Vec<Value>) -> (Arc<Mutex<MockState>>, McpClient) {
        let state = Arc::new(Mutex::new(MockState {
            incoming: incoming.into(),
            ..Default::default()
        }));
        let client = McpClient::with_transport(
            "mock",
            Box::new(MockTransport {
                state: Arc::clone(&state),
            }),
        );
        (state, client)
    }

    fn answer(id: u64, result: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    }

    #[test]
    fn initializes_lists_and_calls_tools() {
        let (state, mut client) = mock_client(vec![
            answer(
                1,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "serverInfo": {"name": "mock", "version": "0"}
                }),
            ),
            answer(
                2,
                json!({
                    "tools": [{
                        "name": "echo",
                        "description": "Echo a text",
                        "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}
                    }],
                    "nextCursor": "page2"
                }),
            ),
            answer(3, json!({"tools": [{"name": "ping"}]})),
            answer(4, json!({"content": [{"type": "text", "text": "pong"}]})),
        ]);

        client.initialize().unwrap();
        assert_eq!(
            state.lock().unwrap().negotiated.as_deref(),
            Some(PROTOCOL_VERSION)
        );

        let tools = client.list_tools().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[1].description, "");
        assert_eq!(tools[1].input_schema, json!({"type": "object", "properties": {}}));

        assert_eq!(
            client.call_tool("echo", json!({"text": "hi"})).unwrap(),
            "pong"
        );

        let sent = state.lock().unwrap().sent.clone();
        let methods: Vec<&str> = sent
            .iter()
            .filter_map(|message| message.get("method").and_then(Value::as_str))
            .collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/list",
                "tools/call"
            ]
        );
        // request ids increment, the notification carries none
        assert_eq!(sent[0]["id"], json!(1));
        assert_eq!(sent[1].get("id"), None);
        assert_eq!(sent[2]["id"], json!(2));
        assert_eq!(sent[3]["id"], json!(3));
        assert_eq!(sent[4]["id"], json!(4));
        assert_eq!(sent[4]["params"]["name"], json!("echo"));
    }

    #[test]
    fn a_terminated_session_is_reinitialized_and_the_call_retried() {
        let (state, mut client) = mock_client(vec![
            answer(1, json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}})),
            // after the 404: the new initialize (id 3) and the retried
            // call (id 4)
            answer(3, json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}})),
            answer(4, json!({"content": [{"type": "text", "text": "again"}]})),
        ]);
        client.initialize().unwrap();
        // the next request is answered with HTTP 404: session over
        state.lock().unwrap().session_expired_once = true;

        assert_eq!(
            client.call_tool("echo", json!({"text": "hi"})).unwrap(),
            "again"
        );

        // the failed call, then a full new handshake, then the retry
        let sent = state.lock().unwrap().sent.clone();
        let methods: Vec<&str> = sent
            .iter()
            .filter_map(|message| message.get("method").and_then(Value::as_str))
            .collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "notifications/initialized",
                "tools/call",
                "initialize",
                "notifications/initialized",
                "tools/call",
            ]
        );
    }

    #[test]
    fn refuses_server_requests_instead_of_hanging() {
        let (state, mut client) = mock_client(vec![
            answer(1, json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}})),
            // a server→client request arrives while tools/list is pending
            json!({"jsonrpc": "2.0", "id": 77, "method": "sampling/createMessage", "params": {}}),
            answer(2, json!({"tools": []})),
        ]);
        client.initialize().unwrap();
        assert_eq!(client.list_tools().unwrap().len(), 0);
        let sent = state.lock().unwrap().sent.clone();
        let refusal = sent
            .iter()
            .find(|message| message.get("id") == Some(&json!(77)))
            .expect("the server request was answered");
        assert_eq!(refusal["error"]["code"], json!(-32601));
    }

    #[test]
    fn jsonrpc_errors_become_errors() {
        let (_, mut client) = mock_client(vec![
            answer(1, json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}})),
            json!({"jsonrpc": "2.0", "id": 2, "error": {"code": -32601, "message": "no tools for you"}}),
        ]);
        client.initialize().unwrap();
        let error = client.list_tools().unwrap_err().to_string();
        assert!(error.contains("no tools for you"), "{}", error);
        assert!(error.contains("mock"), "{}", error);
    }

    #[test]
    fn a_closed_connection_is_reported() {
        let (_, mut client) = mock_client(vec![answer(
            1,
            json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}}),
        )]);
        client.initialize().unwrap();
        let error = client.list_tools().unwrap_err().to_string();
        assert!(error.contains("closed"), "{}", error);
    }

    #[test]
    fn classifies_jsonrpc_messages() {
        assert!(matches!(
            classify(&json!({"id": 1, "result": {}})),
            MessageKind::Response(_)
        ));
        assert!(matches!(
            classify(&json!({"id": 1, "error": {}})),
            MessageKind::Response(_)
        ));
        assert!(matches!(
            classify(&json!({"id": 5, "method": "x"})),
            MessageKind::Request(_)
        ));
        assert!(matches!(
            classify(&json!({"method": "x"})),
            MessageKind::Notification
        ));
        // a non-object message cannot be anything sven must answer
        assert!(matches!(classify(&json!([])), MessageKind::Notification));
    }

    #[test]
    fn extracts_tool_infos_leniently() {
        let info = tool_info(&json!({
            "name": "a", "description": "d", "inputSchema": {"type": "object"}
        }))
        .unwrap();
        assert_eq!(info.name, "a");
        assert_eq!(info.description, "d");

        // optional fields default, a malformed schema becomes "no parameters"
        let info = tool_info(&json!({"name": "b", "inputSchema": "broken"})).unwrap();
        assert_eq!(info.description, "");
        assert_eq!(info.input_schema, json!({"type": "object", "properties": {}}));

        // no usable name → the tool is skipped
        assert!(tool_info(&json!({"description": "nameless"})).is_none());
        assert!(tool_info(&json!({"name": ""})).is_none());
        assert!(tool_info(&json!({"name": 42})).is_none());
    }

    #[test]
    fn formats_tool_results_for_the_model() {
        let text = format_tool_result(&json!({
            "content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]
        }))
        .unwrap();
        assert_eq!(text, "a\nb");

        // binary content becomes a placeholder, not a base64 flood
        let text = format_tool_result(&json!({
            "content": [
                {"type": "image", "data": "AAAA", "mimeType": "image/png"},
                {"type": "resource", "resource": {"uri": "file:///x"}}
            ]
        }))
        .unwrap();
        assert!(text.contains("[image content omitted]"), "{}", text);
        assert!(text.contains("[embedded resource: file:///x]"), "{}", text);

        // structured output is the fallback when there is no text
        let structured = format_tool_result(&json!({
            "content": [],
            "structuredContent": {"answer": 42}
        }))
        .unwrap();
        assert_eq!(structured, json!({"answer": 42}).to_string());

        // isError results become errors carrying the message
        let error = format_tool_result(&json!({
            "content": [{"type": "text", "text": "boom"}], "isError": true
        }))
        .unwrap_err()
        .to_string();
        assert!(error.contains("boom"), "{}", error);

        assert!(format_tool_result(&json!({})).is_err());
    }
}
