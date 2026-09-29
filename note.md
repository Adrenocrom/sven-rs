# Note: Running sven-rs on an OpenAI-compatible backend

**Date:** 2026-09-29
**Goal:** run sven-rs against an OpenAI-compatible layer (OpenAI, Groq, Together, LM Studio,
vLLM, …) instead of only Ollama.

## Verdict

The project is **Ollama-native, not OpenAI-compatible**, but it is closer than it looks. The
**tool definitions** and the **tool-call parsing** are already OpenAI-shaped. What is hardcoded to
Ollama is the **request endpoint/body** and the **response parser**. Supporting an OpenAI-compatible
backend therefore needs a small **backend abstraction** — the cleanest version is a `Backend` enum
that branches only the wire code.

## What already works (no change needed)

- **Tool definitions** (`src/sven/tool_registry.rs`) are emitted as OpenAI-style
  `{"type":"function","function":{...}}` schemas.
- **`parse_tool_call()`** reads `tool_calls[].function.name/.arguments` — identical shape in both
  Ollama and OpenAI.
- **Message roles** (`system`/`user`/`assistant`/`tool`) match.

## What is Ollama-specific and must change

### 1. Endpoint + request body — `src/sven/agent.rs::run()`

```rust
let url = format!("{}/api/chat", &self.config.host);   // Ollama-native
```

OpenAI-compatible servers speak `POST {host}/v1/chat/completions`. Two more things in the body:

- **`options.num_ctx` is Ollama-specific.** OpenAI has no `options` envelope and no `num_ctx`;
  context window is fixed per model, and output length is `max_tokens`.
- **No auth header.** OpenAI-compatible servers want `Authorization: Bearer <key>`; Ollama ignores
  it.

Current request:

```rust
self.client.post(url).json(&json!({
    "model": ..., "stream": true,
    "options": &self.config.options,   // ← Ollama-only
    "tools": ..., "messages": ...
}))
```

### 2. Response parser — `StreamState::process_json()`

This is the biggest change. The two protocols are structurally different:

| Field | Ollama `/api/chat` | OpenAI `/v1/chat/completions` |
|---|---|---|
| content chunk | `message.content` | `choices[0].delta.content` |
| thinking | `message.thinking` | *(none in standard API)* |
| end-of-stream | `done == true` | `choices[0].finish_reason != null` |
| token stats | `eval_count` / `prompt_eval_count` | `usage` (non-streaming only) |
| tool calls | `message.tool_calls` (whole object at once) | `choices[0].delta.tool_calls` (**chunked**, needs `id` + `index` accumulation) |

So `process_json()` must branch on which shape it sees. OpenAI also **streams tool-call arguments
in pieces** and only gives you the `id` in the first chunk — you must accumulate
`function.arguments` across chunks and key each call by `id`/`index`. Ollama sends the complete
call in one message.

### 3. Tool-result round-trip — `src/sven/chat_history.rs`

```rust
// current (Ollama-style)
json!({"role":"tool","content":content,"tool_name":tool_name})
```

OpenAI matches a tool response to its call via **`tool_call_id`**, not `tool_name`. You currently
discard the id (`_id: Option<Value>` in `tool()`). To work against OpenAI you must:

- capture the `id` from the tool call in `assistant()`,
- emit `{"role":"tool","tool_call_id":<id>,"content":...}` in `tool()`.

(`tool_name` is harmless to OpenAI — it just won't be used for matching — but the missing
`tool_call_id` is what actually breaks it.)

### 4. Config — `src/sven/config.rs`

- Add an `api_key` field (default empty; only sent for non-Ollama backends).
- `ChatOptions.num_ctx` is Ollama-only. Either drop it for OpenAI or map it to `max_tokens`.

## Recommended approach: a `Backend` enum + adapter

Rather than rewriting to OpenAI-only (you'd lose the nice Ollama features — `thinking` rendering,
`eval_count` stats), add a backend selector and branch only the wire code:

```rust
#[derive(Clone, Copy, PartialEq)]
enum Backend { Ollama, OpenAI }

impl Backend {
    fn completions_url(&self, host: &str) -> String {
        match self {
            Backend::Ollama  => format!("{host}/api/chat"),
            Backend::OpenAI  => format!("{host}/v1/chat/completions"),
        }
    }
    fn api_key(&self) -> Option<String> {
        match self { Backend::Ollama => None, Backend::OpenAI => Some(key) }
    }
    fn max_tokens(&self, options: &ChatOptions) -> Option<i64> {
        match self { Backend::Ollama => None, Backend::OpenAI => Some(options.max_tokens) }
    }
}
```

Then:

- **`run()`**: pick URL + key + `max_tokens` from the enum; drop `options` for OpenAI.
- **`process_json()`**: detect the shape (`if json["choices"].is_object() { OpenAI } else { Ollama }`)
  and route to the right field accessors. Accumulate OpenAI tool-call args by `id`/`index`.
- **`chat_history`**: emit `tool_call_id` when backend is OpenAI.

## Practical notes

- **Streaming**: both use SSE, so `stream: true` works unchanged — just the payload shape differs.
- **`num_ctx`**: on OpenAI you can't set the context window; it's fixed per model. `max_tokens`
  caps *output*, not the whole context.
- **Testing tip**: you can validate against a real OpenAI-compatible server by pointing `host` at
  `{base}/v1` and setting the key — no code change needed once the enum is in place.

## Files to touch

| File | Change |
|---|---|
| `src/sven/agent.rs` | `run()` URL/body/`api_key`/`max_tokens`; `process_json()` dual-shape parsing + tool-call accumulation |
| `src/sven/chat_history.rs` | emit `tool_call_id` for OpenAI tool messages |
| `src/sven/config.rs` | add `api_key`; `num_ctx` → `max_tokens` mapping |
| `src/main.rs` | add `--backend` arg / env var to select Ollama vs OpenAI |
