# sven-rs

A terminal coding agent for local LLMs, written in Rust. sven-rs talks to any
Ollama-compatible server via the streaming `POST /api/chat` API and gives the
model a set of tools so it can act on your workspace — read and edit files,
search the codebase, look up man pages, and fetch web pages.

## Requirements

- **Rust nightly** — the path-confinement check uses two unstable features
  (`normalize_lexically`, `path_absolute_method`).
- An **Ollama-compatible server** (default: `http://localhost:11434`).
- External binaries used by the tools: `curl` + `pandoc` (web fetch), `ddgr`
  (web search), `man`, `grep`, `find`, `ls`.

## Usage

```bash
cargo run --release
```

At the prompt, type your message and press Enter. The model streams its
thinking (rendered in green) and its answer inline; tool calls are printed
with a 🔧 as they execute, and their results are fed back to the model until it
stops calling tools.

| Command  | Effect                          |
| -------- | ------------------------------- |
| `/clear` | Reset the conversation history |
| `/close` | Exit                            |

Conversation history persists across prompts within a session; `/clear`
resets it.

## Configuration

sven-rs reads `~/.config/sven/sven.json` and falls back to built-in defaults
if the file is missing or invalid:

```json
{
  "model": "gemma4:12b",
  "host": "http://localhost:11434",
  "system_prompt": "",
  "options": {
    "temperature": 0.1,
    "num_ctx": 32000
  }
}
```

## Tools

| Tool                   | What it does                                        |
| ---------------------- | --------------------------------------------------- |
| `TimeTool`             | Local date and time                                 |
| `ListFiles`            | List files in a directory                           |
| `ReadTool`             | Read a file, with optional offset and line count     |
| `SearchAndReplaceTool` | Search-and-replace content in a file                |
| `ReplaceFileTool`      | Overwrite a file (or create it if it doesn't exist)  |
| `GrepTool`             | Recursive regex search                              |
| `FindTool`             | Find files matching a wildcard pattern              |
| `ManPageTool`          | Display a man page                                  |
| `WebSearch`            | DuckDuckGo search via `ddgr`                        |
| `WebFetch`             | GET a URL, converted HTML → Markdown via `pandoc`    |

## Security model

- **Path confinement:** every tool that takes a path (`ReadTool`, both edit
  tools, `ListFiles`) validates that the path stays inside the current working
  directory (`src/sven/security.rs`).
- **No shell:** all subprocesses are spawned with explicit argv vectors —
  there is no `sh -c` anywhere — so shell command injection is structurally
  impossible.

## Architecture

- `src/main.rs` — REPL loop: loads config, registers tools, runs the agent.
- `src/sven/agent.rs` — streaming chat loop against `/api/chat`: parses the
  NDJSON stream, renders thinking/content, dispatches tool calls, and feeds
  results back to the model.
- `src/sven/chat_history.rs` — conversation history sent with each request.
- `src/sven/config.rs` — config file loading (`SvenConfig::load()`).
- `src/sven/tool.rs` + `tool_registry.rs` — the `Tool` trait and a registry
  that generates JSON-schema tool definitions for the model.
- `src/sven/macros.rs` — the `tool!` macro; every tool is defined with it.
- `src/sven/security.rs` — path-confinement check.
- `src/sven/tools/*` — one file per tool.

### Adding a tool

Define a params struct (its doc comments become the schema descriptions the
model sees), implement the tool with the `tool!` macro, and register it in
`main.rs`:

```rust
#[derive(Deserialize, Debug, JsonSchema)]
struct MyToolParams {
    /// file path
    path: String,
}

tool!(MyTool, MyToolParams, "Describe what the tool does.", execute(args) {
    // args is the deserialized MyToolParams
    Ok(format!("done with {}", args.path))
});
```

## License

MIT — see [LICENSE](./LICENSE).