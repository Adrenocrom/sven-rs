# sven-rs

A simple terminal chat client for local LLMs, written in Rust. It talks to any
Ollama-compatible server (or similar) via the streaming `POST /api/chat` API and
supports a pluggable tool system so the model can call back into your machine —
read files, run searches, look up man pages, fetch web pages, and more.

## Features

- Streaming chat with an Ollama-compatible server (`/api/chat`)
- Thinking + content output rendered inline in the terminal
- Tool use: the agent can invoke tools (file read/write, grep, find, `man`,
  time, web search/fetch) to act on your workspace
- Pluggable tool registry — add a new tool by implementing one trait

## Usage

```bash
cargo run --release
```

At the prompt type your message and press Enter. Type `/close` to exit.

Configuration (model name, host URL, context size, system prompt) is loaded from
a config file via `SvenConfig::load()` — see `src/sven/sven.rs`.

## Architecture

- `main.rs` — CLI loop: loads config, registers tools, runs the read/eval loop.
- `sven/agent.rs` — handles streaming responses and tool-call execution against
  the chat API.
- `sven/tool_registry.rs` + `sven/tools/*` — a registry of self-describing
  tools; each tool declares its name, description, JSON schema, and an execute
  closure.

## License

See [LICENSE](./LICENSE).
