# sven-rs

A terminal coding agent for local LLMs, written in Rust. sven-rs talks to any
Ollama-compatible server via the streaming `POST /api/chat` API and gives the
model a set of tools so it can act on your workspace — read and edit files,
search the codebase, look up man pages, fetch web pages — and a persistent
skill store so knowledge learned in one session is available in the next.

## Requirements

- **Rust** (stable).
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
  "data_dir": "~/.config/sven",
  "model": "gemma4:12b",
  "host": "http://localhost:11434",
  "system_prompt": "",
  "options": {
    "temperature": 0.1,
    "num_ctx": 32000
  }
}
```

`data_dir` is where the skills store lives (`<data_dir>/skills`); a leading
`~` is expanded to `$HOME`.

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
| `WebFetch`            | GET a URL (http/https only), converted HTML → Markdown via `pandoc` |
| `CompileTool`          | Run `cargo build` and report errors                  |
| `ListSkillsTool`       | List all stored skills with name, description, tags |
| `SearchSkillsTool`     | Keyword search over skills (name, description, tags, body) |
| `GetSkillTool`         | Read a skill's full content                         |
| `AddSkillTool`         | Store new knowledge as a skill                      |
| `UpdateSkillTool`      | Update a skill's description, tags and/or body      |
| `RemoveSkillTool`      | Remove a skill and its directory                    |

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
  impossible.
- **Network:** `WebFetch` only accepts `http://` and `https://` URLs.

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