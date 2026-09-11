# Code Review — sven-rs

**Date:** 2026-09-11
**Scope:** full working tree (`Cargo.toml`, `src/main.rs`, `src/sven/**`), checked against the Ollama `/api/chat` documentation.

## Overview

sven-rs is a terminal coding agent: a REPL that drives an Ollama model in a tool-calling loop, backed by a registry of ten tools (file I/O, search, man pages, web search/fetch, time). The architecture is sound and pleasantly small:

- `agent.rs` — Ollama chat loop, NDJSON stream parsing, tool dispatch
- `tool.rs` / `tool_registry.rs` — `Tool` trait, name-keyed registry, schema generation
- `macros.rs` — the `tool!` declarative macro, now used uniformly by all tools
- `security.rs` — path-confinement check for filesystem tools
- `tools/*` — one file per tool

The core loop works, the streaming code is genuinely well done, and the `tool!` macro paid off. But there are **three bugs that break core functionality** (one of them corrupts files), **one security hole** in the path check, and a handful of API-conformance and robustness issues. No tests, no README, and an unpinned nightly-compiler requirement round it out.

**Verdict: promising prototype, not yet safe to point at real files.** Findings are ordered by severity; the top five are all quick fixes.

---

## What's done well

- **NDJSON stream handling is correct.** `handle_chunks` buffers raw bytes, only decodes complete lines, survives chunk boundaries mid-JSON and mid-UTF-8, and handles a final line without a trailing newline. This is the part most hand-rolled streaming clients get wrong, and the comments explain *why* it's done this way.
- **No shell anywhere.** Every subprocess uses `Command::new` with an argv vector — `sh -c` never appears, so command injection is structurally impossible. `WebFetch` even places `--` before the URL to stop flag injection into curl.
- **Errors are fed back to the model.** `process_tool_call` returns error text as the tool result instead of crashing the loop — the correct agent pattern, and the same for unknown tool names.
- **Uniform `tool!` usage.** Every tool goes through the macro (the mixed-style problem from `discussion.md` is resolved), and the doc-comments on the params structs double as schema documentation for the model.
- **A security layer exists at all** — most hobby agents skip path confinement entirely.
- `thiserror` enums, clean module boundaries, `Cargo.lock` committed.

---

## Bugs

### 1. 🔴 `SearchAndReplaceTool` appends instead of replacing (data corruption)

`edit_tool.rs` opens the file with `read(true).write(true)`, reads it to a string, then calls `write_all` — but after `read_to_string` the cursor sits at **EOF**, so the "replaced" content is **appended** to the original file. Worse: when the pattern isn't found, `String::replace` returns the input unchanged, so a no-op "replacement" **duplicates the entire file**. The tool reports success either way.

Fix — read and write as separate operations, and report a miss:

```rust
security::is_inside_cwd(&args.path)?;
let content = fs::read_to_string(&args.path)?;
if !content.contains(args.oldcontent.as_str()) {
    return Ok(format!("Pattern not found in {}", args.path));
}
let new_content = content.replace(args.oldcontent.as_str(), args.newcontent.as_str());
fs::write(&args.path, new_content)?;
```

### 2. 🔴 The path check is bypassable via symlinks

`security.rs` uses `normalize_lexically()` + `absolute()` — both are **purely lexical**: they never look at the filesystem. A symlink inside the project directory (`ln -s /etc link`) sails through `is_inside_cwd`, and the model can then read or write through it. For an agent that executes model-chosen paths, this is the most important issue in the repo.

Fix — resolve with `std::fs::canonicalize` (which follows symlinks), with a fallback for paths that don't exist yet (the write tools create files):

```rust
pub fn is_inside_cwd(path: &str) -> Result<(), SecurityError> {
    if path.is_empty() {
        return Err(SecurityError::EmptyPath);
    }
    let cwd = std::fs::canonicalize(".")?;
    let resolved = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => {
            // Path doesn't exist yet (e.g. ReplaceFile creating a new
            // file): confine its parent directory instead.
            let p = Path::new(path);
            let name = p.file_name().ok_or(SecurityError::EmptyPath)?;
            let parent = match p.parent() {
                Some(p) if !p.as_os_str().is_empty() => p,
                _ => Path::new("."),
            };
            std::fs::canonicalize(parent)?.join(name)
        }
    };
    if !resolved.starts_with(&cwd) {
        return Err(SecurityError::Unauthorized);
    }
    Ok(())
}
```

Bonus: this runs on **stable Rust** and eliminates both nightly feature gates (`normalize_lexically`, `path_absolute_method`); the `NormalizationError` variant can go too. A residual check-then-open TOCTOU window remains — acceptable for this threat model, but worth a comment. The `starts_with` prefix check itself is sound: `Path::starts_with` compares components, so there's no `/cwd-evil` prefix confusion.

### 3. 🔴 `GrepTool` can't return matches

`grep -rni <pattern>` is invoked with **no file operand**. Under `Command::output()` the child's stdin is closed, so grep either errors out ("no file specified", depending on the build) or "recursively searches stdin" — which is already at EOF. Either way the tool returns nothing useful, not a search of the project.

Fix — search the project explicitly, and add `--` so a pattern starting with `-` can't be parsed as a grep flag:

```rust
command.arg("-rni");
command.arg("--");
command.arg(args.pattern);
command.arg(".");
```

### 4. 🟡 The `/close` command never exits — and Ctrl-D loops forever

`read_line` keeps the trailing newline, so `"/close".eq(&input)` compares against `"/close\n"` — never true. The exit command is instead sent to the model as a prompt. Worse, EOF isn't handled: `read_line` returns `Ok(0)` on Ctrl-D, the empty string goes to the model as a user message, and the next iteration reads EOF again — an **infinite loop of empty model calls**.

```rust
let n = std::io::stdin().read_line(&mut input)?;
if n == 0 { break; }                    // EOF / Ctrl-D
if input.trim() == "/close" { break; }
agent.run(input.trim()).await;
```

### 5. 🟡 Wrong role name: `"assist"` → `"assistant"`

Per the Ollama API, message roles are `system | user | assistant | tool`, and every history example in the docs uses `"assistant"`. `"assist"` is off-spec — at best it passes through and the model's template misinterprets the turn.

### 6. 🟡 Assistant content is silently dropped when tool calls are present

When the model emits text *and* tool calls in one turn, the text is accumulated in `StreamState.content`, but the history entry is pushed with `"content": ""`. The model loses its own reasoning on the next round — push `message.content` instead. (Related: `MessageResponse.content`/`.thinking` are otherwise unused — use content, and drop thinking or keep it for logging.)

### 7. 🟡 Panic on malformed model output

`tool_call["function"]["name"].as_str().unwrap()` — LLM output is untrusted input. A tool call without a name crashes the whole REPL. Use `let ... else` and push an error string back as the tool result, exactly like a missing tool.

---

## Security review

Good news first: **no shell-injection surface** (argv vectors everywhere, `--` before user-controlled data in curl), and path confinement exists and is applied to every tool that takes a path (read, both edit tools, list_files), while grep/find are rooted at `.`. Beyond bug #2:

- **WebFetch scheme/redirect hardening (🟡).** The description promises "http:// or https://" but nothing enforces it, and `-L` follows redirects — depending on curl version/build, a redirect can cross into other schemes. Validate the scheme in Rust before spawning, and pin curl: `--proto '=http,https' --max-time 30`. Also cap response size (see truncation under Design).
- **Flag injection into grep/find/man/ddgr (🟢).** An argument starting with `-` is consumed as a flag by the child process. Not exploitable (no shell), but it produces confusing failures. Use `--` separators where the tool supports them; otherwise prefix-validate.
- **`is_inside_cwd` prints to stdout (🟢)** — a predicate with a print side effect. Log at the call site, or use `eprintln!`.

---

## Design & architecture

- **No tool-round cap.** `Agent::run` loops until the model stops calling tools; a model stuck in a tool loop spins forever, re-sending the whole history each round. Cap it (`for _ in 0..MAX_ROUNDS`) and tell the model when the cap is hit.
- **No timeouts anywhere.** The reqwest `Client` has no connect/read timeout and curl has no `--max-time`; a hung Ollama silently freezes the REPL.
- **Tool output is unbounded.** One `grep` over a big tree or a long web page flows straight into `num_ctx` (32k). Truncate centrally in `process_tool_call` (e.g. first ~8–16 KB + "…truncated") — one fix covers every tool.
- **No conversation memory across REPL turns.** Each `run()` builds a fresh history, so "now fix that" has no referent. If intentional (cheap contexts), document it; otherwise keep the history on `Agent`.
- **Blocking sync tools inside async.** File I/O and subprocesses run on the async worker thread. Harmless for a single-user REPL, but `tokio::task::spawn_blocking` is the correct pattern — worth adopting before tools ever run concurrently.
- **`ToolRegistry` details.** `pub tools` leaks the map (make it private); `get_tool` returns `Option<&Box<dyn Tool>>` — return `Option<&dyn Tool>` to drop a double indirection; `HashMap` iteration order randomizes the tool list sent to the model on every request (mild nondeterminism — a `BTreeMap` or `Vec` + index is kinder to the model).
- **`"parameters": null`** for no-arg tools (`TimeTool`). The Ollama docs always show a schema object; default to `{"type": "object", "properties": {}}` in `generate_tool_definitions`.
- **`ToolError` is dead code.** Nothing constructs it — tools box raw `io::Error`/`SecurityError` instead. Either route tool errors through it (its `InvalidParameters`/`MissingParameter` variants would give the model better feedback) or delete it.
- **Tool naming/allocations (🟢).** `name()`/`desc()` allocate fresh `String`s on every call — `&'static str` is free. Names are PascalCase (`SearchAndReplaceTool`); snake_case (`search_and_replace`) is the function-calling convention and matches what models see in training data.
- **Shelling out vs. crates already in the tree (🟢).** `WebFetch` uses curl+pandoc while reqwest is already a dependency; a pure-Rust fetch (reqwest + an HTML-to-markdown crate) would drop two external binaries. Same argument applies mildly to grep/find/ls (`regex`, `walkdir`, `fs::read_dir`).

---

## Project hygiene

- **Zero tests.** The highest-value target is `security.rs`: `../` escapes, absolute paths outside cwd, symlink escape, empty path, and the not-yet-existing-file case from the fix above. Cheap table-driven tests guarding the scariest code in the repo.
- **No README.** Undocumented: the nightly compiler requirement, the external binaries the tools assume (`curl`, `pandoc`, `ddgr`, `man`, `grep`, `find`, `ls`), and the config file format/location.
- **No `rust-toolchain.toml`.** Two unstable features gate the build on whatever nightly the author happens to have installed. Pin the toolchain — or drop the features entirely per bug #2.
- **No CI.** `cargo fmt --check` + `cargo clippy -D warnings` + `cargo test` would have caught several items here (e.g. `if let Ok(_)` in main.rs, formatting drift like `num_lines:  Option<u64>` in `read_tool.rs`).
- **Hardcoded config path** `/home/josef/.config/sven/sven.json` — the binary can't find its config on any other machine. Use `XDG_CONFIG_HOME` / `$HOME/.config` (or the `dirs` crate).
- **Debug leftovers in `main.rs`:** `println!` of the model and `num_ctx` on every startup; ten redundant `Box<dyn Tool>` type annotations; ten `register` calls that collapse to:

```rust
let tools: Vec<Box<dyn Tool>> = vec![
    Box::new(TimeTool), Box::new(ListFiles), Box::new(ReadTool),
    Box::new(WebSearch), Box::new(WebFetch), Box::new(SearchAndReplaceTool),
    Box::new(ReplaceFileTool), Box::new(GrepTool), Box::new(FindTool),
    Box::new(ManPageTool),
];
let mut registry = ToolRegistry::new();
for t in tools { registry.register(t); }
```

---

## Minor / nits

- `agent.rs`: stream-decode failures print to **stdout** (`println!("... couldn't decode JSON")`) — use `eprintln!`.
- `read_tool.rs` always appends `\n` to the final line, even when the file has no trailing newline; also `num_lines: 0` silently means "whole file".
- `find_tool.rs`: no max-depth and no ignore rules — `target/` and `.git/` noise in every result.
- `manpage_tool.rs`: failures return an empty string (stderr discarded, exit status unchecked) — the model can't tell "no such page" from "empty page".
- `crate::sven::sven::SvenConfig` — the doubled module name reads awkwardly at every use site.
- `Cargo.toml`: tokio `features = ["full"]` → `["macros", "rt-multi-thread"]` is all you use; add `description`/`readme` metadata.
- `discussion.md` is a nice design log — consider moving it under `docs/`.

---

## Prioritized action list

1. **Fix `SearchAndReplaceTool`** — it appends and corrupts files; read + `fs::write`, report misses.
2. **Switch `is_inside_cwd` to `canonicalize`** — closes the symlink hole *and* drops the nightly requirement.
3. **Fix `GrepTool`** — add the `.` operand and `--`.
4. **Fix the REPL exit** — trim `/close`, handle EOF, remove debug prints.
5. **Ollama conformance** — `"assistant"` role, preserve `message.content` in history, default `parameters` to an empty object.
6. **Harden the loop** — no `unwrap()` on model output, tool-round cap, central output truncation, client/curl timeouts.
7. **Config path via XDG**; pin the toolchain; write the README (nightly + external binaries + config format).
8. **Add tests for `security.rs`** and wire up fmt/clippy/test CI.

Items 1–5 are small, mechanical fixes; together they take the project from "demo" to "usable on real files".