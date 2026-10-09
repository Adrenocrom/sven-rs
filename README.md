# sven-rs

A terminal coding agent for local LLMs, written in Rust. sven-rs talks to any
Ollama-compatible server via the streaming `POST /api/chat` API and gives the
model a set of tools so it can act on your workspace — read and edit files,
search the codebase, look up man pages, fetch web pages — and a persistent
skill store so knowledge learned in one session is available in the next.

## Requirements

- **Rust** (stable).
- An **Ollama-compatible server** (default: `http://localhost:11434`), or an
  **OpenAI-compatible server** — OpenAI, vLLM, LM Studio, OpenRouter, … — via
  the `backend` config field (see below).
- External binaries used by the tools: `curl` + `pandoc` (web fetch), `ddgr`
  (web search), `man`, `grep`, `find`, `ls`.

## Usage

```bash
cargo run --release
```

`host`, `model` and `backend` can be overridden on the command line without
touching `sven.json` — useful for trying a different server or model for one
run:

```bash
cargo run --release -- --backend vllm --model Qwen/Qwen2.5-7B-Instruct --host http://localhost:8000
```

| Flag        | Effect                                          |
| ----------- | ----------------------------------------------- |
| `--backend` | `ollama`, `openai` or `vllm` (invalid → error)  |
| `--model`   | Model name sent to the server                    |
| `--host`    | Server root URL                                 |
| `--prompt`  | REPL prompt prefix (default `>> `)              |
| `--end-of-prompt` | Non-interactive mode: read stdin until the marker |
| `--config-dir` | Directory to load `sven.json` from (default `~/.config/sven`) |

Flags override the config file; anything not given falls back to `sven.json`
(or the built-in defaults).

At the prompt, type your message and press Enter. The model streams its
thinking (rendered in green) and its answer inline; tool calls are printed
with a 🔧 as they execute, and their results are fed back to the model until it
stops calling tools.

Each streamed response prints its token counts (`in <prompt> out <completion>`)
as reported by the server, and when the run finishes a summary line shows the
accumulated tokens of all rounds and the wall time:

```
run finished in 1m 3s — in 15234 out 812 tokens (16046 total)
```

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
  "backend": "ollama",
  "data_dir": "~/.config/sven",
  "model": "gemma4:12b",
  "host": "http://localhost:11434",
  "system_prompt": "",
  "options": {
    "temperature": 0.1,
    "num_ctx": 32000
  },
  "mcp_servers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {"GITHUB_TOKEN": "…"}
    }
  }
}
```

`data_dir` is where the skills store lives (`<data_dir>/skills`); a leading
`~` is expanded to `$HOME`.

### Backends

`backend` selects the server protocol (lowercase). The default host is
`http://localhost:11434` for every backend — set `host` explicitly when it
differs:

| Value    | Protocol                          | Typical host                 |
| -------- | --------------------------------- | ---------------------------- |
| `ollama` | Ollama `POST /api/chat` (NDJSON)  | `http://localhost:11434`     |
| `openai` | OpenAI `/v1/chat/completions`     | `https://api.openai.com`     |
| `vllm`   | OpenAI-compatible (vLLM)          | `http://localhost:8000`      |

`vllm` and `openai` share the OpenAI wire format (SSE streaming, fragmented
tool calls, `tool_call_id` matching); `vllm` exists as its own value so the
config is self-documenting and vLLM-specific behavior has a place to diverge.

### vLLM

vLLM serves an OpenAI-compatible API, so point `host` at the server root
(vLLM's default port is 8000) and set `backend` to `vllm`:

```json
{
  "backend": "vllm",
  "model": "Qwen/Qwen2.5-7B-Instruct",
  "host": "http://localhost:8000"
}
```

Tool calling requires the server to be started with
`--enable-auto-tool-choice --tool-call-parser hermes` (the parser must match
the model — `hermes` for Qwen, `llama3_json` for Llama 3.1, `mistral` for
Mistral, …). Without those flags vLLM rejects requests carrying `tools` with
a 400 error, which sven reports instead of silently doing nothing. For
reasoning models, add `--reasoning-parser deepseek_r1` to stream the model's
thinking (vLLM emits it as `reasoning_content`, rendered in green like
Ollama's `thinking`).

If the server was started with `--api-key`, export the same value as
`SVEN_API_KEY`; otherwise no key is needed.

### API key

The API key is **not** part of the config file — it is read from the
environment variable `SVEN_API_KEY`:

```bash
export SVEN_API_KEY="sk-..."
```

It is only sent for the OpenAI-compatible backends (`openai`, `vllm`) as
`Authorization: Bearer …`; Ollama ignores it. Keeping the key out of
`sven.json` means it cannot leak through file reads, backups or dotfile
syncs.

### MCP servers

sven-rs is an MCP **client**: every server listed under `mcp_servers` in
`sven.json` is connected at startup, and each tool it reports is registered
like a built-in tool, named `mcp__<server>__<tool>`. The implementation is
hand-rolled JSON-RPC 2.0 — no SDK, no extra dependencies — and supports the
two current transports:

| Transport | Config fields                                        | Notes |
| --------- | ---------------------------------------------------- | ----- |
| stdio     | `command`, `args`, `env`                             | The server runs as a child process for the whole session; `env` adds to the inherited environment. |
| Streamable HTTP | `url`, `headers`                                | One POST per message via `reqwest`; `headers` can carry e.g. `Authorization`. |

```json
{
  "mcp_servers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {"GITHUB_TOKEN": "ghp_…"}
    },
    "remote": {
      "url": "https://mcp.example.com/mcp",
      "headers": {"Authorization": "Bearer …"}
    }
  }
}
```

Behavior worth knowing:

- A server that cannot be started or reached is reported on stderr and
  skipped — it does not stop the agent.
- `initialize` and `tools/list` get 120 s (package managers like `npx`/`uvx`
  download on first start), a `tools/call` gets 60 s; a hung server cannot
  freeze the agent.
- Requests the server sends *to* sven (sampling, roots, elicitation) are
  refused with a JSON-RPC error — sven supports none of them, and refusing
  keeps a waiting server from hanging.
- Tool results are rendered as text for the model; binary content (images,
  audio) becomes a placeholder instead of a base64 flood, and `isError`
  results become tool errors.
- An HTTP server that terminated the session (it answers 404, and may do so
  at any time — restart, idle timeout) is re-initialized transparently and
  the failed request retried once.
- The deprecated HTTP+SSE transport (a GET stream before the first POST) is
  not supported.

## Tools

| Tool                   | What it does                                        |
| ---------------------- | --------------------------------------------------- |
| `TimeTool`             | Local date and time                                 |
| `ListFiles`            | List files in a directory                           |
| `ReadTool`             | Read a file, with optional offset and line count     |
| `SearchAndReplaceTool` | Search-and-replace content in a file                |
| `ReplaceFileTool`      | Overwrite a file (or create it if it doesn't exist)  |
| `GrepTool`             | Recursive regex search (extended regex, `foo|bar` alternation) |
| `FindTool`             | Find files matching a wildcard pattern              |
| `ManPageTool`          | Display a man page                                  |
| `WebSearch`            | DuckDuckGo search via `ddgr`                        |
| `WebFetch`            | GET a URL (http/https only), converted HTML → Markdown via `pandoc` |
| `CompileTool`          | Run `cargo build` and report errors                  |
| `ListSkillsTool`       | List all stored skills with name, description, tags |
| `SearchSkillsTool`     | Keyword search over skills (name, description, tags, body) |
| `GetSkillTool`         | Read a skill's full content                         |
| `AddSkillTool`         | Store new knowledge as a skill                      |
| `UpdateSkillTool`      | Update a skill's description, tags and/or body      |
| `RemoveSkillTool`      | Remove a skill and its directory                    |
| `mcp__<server>__<tool>` | Any tool of a configured MCP server (see above)     |

## Skills

sven-rs has a persistent knowledge store: skills are markdown files under
`<data_dir>/skills/<kebab-case-name>/SKILL.md`, each with YAML frontmatter
(name, description, tags, created_at) and a markdown body. The skill tools
(`SearchSkillsTool`, `GetSkillTool`, `AddSkillTool`, …) let the model look up
and store knowledge, so it survives across runs.

Skill names are sanitized to snake_case identifiers (kebab-case directory
names), tags to lowercase hyphenated keywords (3–8 after sanitization).
Because names are reduced to `[a-z0-9-]`, skill paths cannot escape the
store — the skill tools rely on this sanitization instead of the
path-confinement check used by the file tools.

## Security model

- **Path confinement:** every tool that takes a path (`ReadTool`, both edit
  tools, `ListFiles`, `GrepTool`, `FindTool`) validates that the path stays
  inside the current working directory (`src/sven/security.rs`), resolving
  symlinks so a link pointing outside the workspace cannot be used to escape.
- **No shell:** all subprocesses are spawned with explicit argv vectors —
  there is no `sh -c` anywhere — so shell command injection is structurally
  impossible. An explicit argv does *not* prevent argument injection: a
  program may still read a model-controlled string as one of its own
  options (for `find`, `-delete` is an action, not a path). Every
  model-controlled operand is therefore guarded — path-like parameters are
  prefixed with `./` or passed after the `--` end-of-options marker, and
  man page names, which never legitimately start with `-` or contain `/`,
  are rejected: `man` reads a slash-containing argument as a *file path*,
  so without this check `ManPageTool` would be an arbitrary-file reader.
- **Network:** `WebFetch` only accepts `http://` and `https://` URLs.
- **MCP:** the HTTP transport accepts only `http://`/`https://` URLs and
  rejects header names containing `:` or control characters (a crafted
  header could smuggle a second header line into the request). stdio
  servers run with explicit argv — no shell — and inherit the
  environment plus the configured `env` entries. MCP tools are remote
  code by definition: they run whatever the server implements, so only
  list servers you trust.

## Architecture

- `src/main.rs` — REPL loop: loads config, registers tools, runs the agent.
- `src/sven/agent.rs` — streaming chat loop: parses the NDJSON (Ollama) or
  SSE (OpenAI-compatible) stream, renders thinking/content, dispatches tool
  calls, and feeds results back to the model.
- `src/sven/backend.rs` — the `Backend` enum (`ollama`, `openai`, `vllm`):
  endpoint, wire format and stream parsing per server protocol.
- `src/sven/chat_history.rs` — conversation history sent with each request.
- `src/sven/config.rs` — config file loading (`SvenConfig::load()`).
- `src/sven/mcp/` — the MCP client: JSON-RPC 2.0 over stdio (child
  process) or Streamable HTTP (one `reqwest` POST per message), plus the
  `Tool` wrapper that exposes remote tools as `mcp__<server>__<tool>`.
- `src/sven/tool.rs` + `tool_registry.rs` — the `Tool` trait and a registry
  that generates JSON-schema tool definitions for the model.
- `src/sven/macros.rs` — the `tool!` macro; every tool is defined with it.
- `src/sven/security.rs` — path-confinement check (symlink-aware).
- `src/sven/security_error.rs` — its error type.
- `src/sven/term.rs` — ANSI colors, gated on TTY and `NO_COLOR`.
- `src/sven/skills.rs` — skill store: minimal YAML frontmatter parser,
  SKILL.md serialization, keyword search.
- `src/sven/tools/*` — one file per tool; `subprocess.rs` holds the shared
  exit-status/stderr-aware command runner.

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

### Provenance

sven-rs is an independent Rust implementation of the ideas in the Python
project [Adrenocrom/sven](https://github.com/Adrenocrom/sven) (GPL-3.0). No
code, comments, prompts or configuration from that project were copied; the
Rust implementation was written from scratch (different architecture: a
`tool!` macro with schemars-derived JSON schemas, a streaming NDJSON chat
parser, a hand-rolled YAML frontmatter parser, a sanitization-based skills
store). It is distributed under MIT on that basis.
