# Code Review — sven-rs

**Date:** 2026-09-29
**Scope:** full workspace (`src/main.rs`, `src/sven/**`, `Cargo.toml`, `README.md`, `.gitignore`, `LICENSE`)
**Method:** read all sources, verified the build (`cargo build` passes on nightly), and reproduced the
security/robustness findings marked **[reproduced]** by exercising the tools directly. Findings marked
**[code analysis]** are reasoned from the code path and documented std behavior.

## Summary

sven-rs is a small, well-structured terminal coding agent with a genuinely nice tool abstraction
(the `tool!` macro + schemars-derived JSON schemas) and an unusually solid hand-rolled skills store.
The core chat loop works, path confinement blocks `..` traversal and absolute paths, and there is no
`sh -c` anywhere.

The problems are concentrated in two areas:

1. **The security model has two real bypasses** — `WebFetch` happily fetches `file://` URLs
   (reproduced: it read `/etc/hostname`), and `is_inside_cwd` does not resolve symlinks.
2. **The tools systematically lie to the model.** Subprocess exit codes and stderr are discarded,
   `SearchAndReplaceTool` reports success when the search text was never found, and nothing
   truncates tool output. An agent that is told "success" when the edit silently did nothing will
   confidently build on a corrupted state.

None of the fixes are large; most are a few lines each.

---

## Critical

### C1. `WebFetch` fetches `file://` URLs — full path-confinement bypass **[reproduced]**

**Location:** `src/sven/tools/web_fetch.rs` (`WebFetch`)

`curl` is invoked with the raw URL and no scheme check. `curl --silent -L -f -- file:///etc/hostname`
reads a local file and pipes it through pandoc.

Reproduced: `WebFetch(url = "file:///etc/hostname")` returned the contents of `/etc/hostname`.
The model can therefore read any file the user can read (`~/.ssh/…`, `~/.aws/credentials`, …),
completely defeating the confinement that `ReadTool` enforces. `curl` also supports `gopher://`,
`dict://`, `ftp://`, etc.

**Fix:** allowlist the scheme before spawning curl:

```rust
if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
    return Err(format!("unsupported URL scheme (only http/https): {}", args.url));
}
```

(Longer term, `reqwest` is already a dependency and refuses `file://` by default.) Also add
`--max-time 30` — there is currently no timeout, so a hung server hangs the agent.

### C2. `is_inside_cwd` does not resolve symlinks **[code analysis]**

**Location:** `src/sven/security.rs` (`is_inside_cwd`)

The check is `normalize_lexically()` → `absolute()` → `starts_with(cwd)`. Both of those are purely
lexical: `Path::absolute` explicitly "does not resolve symlinks". A symlink *inside* the cwd that
points outside (e.g. `./link -> /etc/passwd`) passes the check, and `File::open`/`fs::write` then
follow it. `ReplaceFileTool` makes this *write* access outside the workspace.

**Fix:** canonicalize instead:

```rust
let canonical = std::fs::canonicalize(path)?;      // resolves symlinks; stable API
let cwd = std::env::current_dir()?;                // already canonical
if !canonical.starts_with(&cwd) { return Err(SecurityError::Unauthorized); }
```

Two notes:
- `canonicalize` fails for nonexistent paths. For `ReplaceFileTool` (which creates files),
  canonicalize the parent and re-join the file name.
- This change **removes the only reason the crate needs nightly** (`normalize_lexically`,
  `path_absolute_method`), which the README currently lists as a requirement. That is a win on its
  own.

---

## High

### H1. Malformed tool calls crash the whole agent **[code analysis]**

**Location:** `src/sven/agent.rs` (`Agent::run`)

```rust
let tool_name = tool_call["function"]["name"].as_str().unwrap();
```

Local models emit malformed tool calls regularly (missing `name`, non-string `name`, wrong shape).
One bad payload panics the process and the user loses the session. The same loop indexes
`["function"]["arguments"]` without validation.

**Fix:** treat an unparseable tool call as a tool *result* — push an error message back into the
history (`"Error: malformed tool call: {payload}"`) and continue. The model usually self-corrects.

### H2. No cap on tool rounds

**Location:** `src/sven/agent.rs` (`Agent::run`)

The `loop` runs until the model stops requesting tools. A model stuck in a loop (common with small
local models) will call tools forever — burning tokens, hammering the Ollama server, and on this
machine re-running `cargo build` via `CompileTool` indefinitely.

**Fix:** count rounds, bail out (with a message into the history and to the user) after e.g. 25.

### H3. Subprocess tools discard exit status and stderr — silent failures **[reproduced]**

**Location:** `grep_tool.rs`, `find_tool.rs`, `list_files.rs`, `manpage_tool.rs`, `web_search.rs`,
`web_fetch.rs`, `compile_tool.rs`

Every subprocess tool does `command.output()?` and then returns `String::from_utf8(output.stdout)`.
`output()?` only fails if the *program couldn't be spawned*; a program that ran and failed produces
an empty `Ok("")`. Reproduced:

- `GrepTool` with an invalid regex (`"["`) → empty output, no error (grep exits 2, stderr dropped).
- `ListFiles` on a nonexistent directory → empty output, no error.
- `WebFetch` on an HTTP 404 → empty output, no error (`curl -f` suppresses the body).
- `WebSearch`/`ManPageTool`/`CompileTool` behave the same if `ddgr`/`man`/`cargo` are missing.

The model cannot distinguish "no matches" from "the tool is broken", which produces confidently
wrong answers ("I searched the codebase and found nothing").

**Fix:** a small helper used by all subprocess tools:

```rust
fn run(command: &mut Command) -> Result<String, Box<dyn std::error::Error>> {
    let out = command.output()?;
    if !out.status.success() {
        return Err(format!("{} exited with {}: {}",
            command.get_program().to_string_lossy(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8(out.stdout)?)
}
```

---

## Medium

### M1. `SearchAndReplaceTool` reports success when nothing was replaced **[reproduced]**

**Location:** `src/sven/tools/edit_tool.rs`

`content.replace(old, new)` is a no-op when `old` isn't present, and the tool then returns
`"File … replaced successfully"`. Reproduced: replacing a string that does not occur in the file
returned success. For an *agent*, this is the worst failure mode — it believes the edit landed and
moves on. Additionally, an empty `oldcontent` is accepted: `str::replace("", "X")` inserts `X`
between every character, so `oldcontent: ""` silently shreds the file and reports success.

**Fix:**

```rust
if args.oldcontent.is_empty() { return Err("oldcontent must not be empty".into()); }
if !content.contains(&args.oldcontent) {
    return Err(format!("pattern not found in {}", args.path));
}
```

Also remove the `println!("{}", new_content)` — it dumps the entire file to the terminal on every
edit.

### M2. Unbounded tool output floods the model context **[reproduced]**

**Location:** all tools; `Agent::process_tool_call` is the natural chokepoint

`ReadTool` has `num_lines`, but `GrepTool`, `FindTool`, `ManPageTool`, `WebFetch` and `WebSearch`
return their full output. Reproduced: `ManPageTool("grep")` returned the entire ~15 KB man page.
A few of those and the 32 K context is gone, and the model starts truncating its own reasoning.

**Fix:** truncate centrally in `process_tool_call` before pushing to history, e.g. cap at ~10 000
characters with a `… [output truncated]` suffix. One place, every tool covered.

### M3. Config loading: partial files silently reset everything; missing `HOME` panics

**Location:** `src/sven/config.rs` (`SvenConfig::load`)

- `SvenConfig`/`ChatOptions` have no `#[serde(default)]`, so a config file that sets only
  `"model"` fails to deserialize — and the code then falls back to *all* defaults, printing one
  line to stdout. The user's other settings (host, num_ctx, system_prompt) are silently discarded.
- `std::env::var("HOME").expect(…)` panics if `HOME` is unset (common in cron/systemd contexts).

**Fix:** annotate fields with `#[serde(default)]` (the `Default` impls already exist), and fall
back gracefully instead of `expect`. Return `Result`/log to stderr rather than `println!`.

### M4. The REPL has no input history — arrow keys don't recall anything

**Location:** `src/main.rs` (rustyline loop)

`DefaultEditor` does **not** auto-add lines to history (rustyline's `auto_add_history` defaults to
false), and `add_history_entry` is never called. Up-arrow does nothing, which for a REPL whose
README advertises rustyline is surprising. There is also no `load_history`/`save_history`, so
history doesn't survive restarts.

**Fix:** `let _ = rl.add_history_entry(&line);` after each successful read, plus
`load_history`/`save_history` on `<data_dir>/history` if persistence is wanted.

### M5. `GrepTool`/`FindTool` search `target/` and `.git` **[reproduced]**

**Location:** `src/sven/tools/grep_tool.rs`, `src/sven/tools/find_tool.rs`

`grep -rni <pattern>` with no operand relies on the GNU extension "no FILE → search the working
directory" (it is not what POSIX requires), and it walks everything — including `target/` (build
artifacts, easily 10× the source tree) and `.git`. Reproduced: a search for `ref_cast` (a transitive
dep) returned 30+ hits, every single one from `target/`, none from `src/`.

**Fix:** add `--exclude-dir=target --exclude-dir=.git` (GNU grep) / a `-not -path` clause for find,
and consider an optional `path` parameter so the model can scope searches. Passing an explicit `.` operand would also make the grep invocation portable.

---

## Low

### L1. Model-facing text bugs

These are visible to the LLM in the tool schema, so they directly affect tool use:

- `compile_tool.rs`: parameter is spelled **`lanuguage`**.
- `edit_tool.rs`: the `n` parameter is documented as *"type of replacing method, one of First, All
  and Last"* — it is actually an occurrence count for `replacen`.
- `web_search.rs`: description ends "…for further **investion**".
- `grep_tool.rs`: description says "in given files or stdin", but the tool takes no file parameter.
- `manpage_tool.rs`: description contains leftover CLI text "Usage: manpage <name>".

### L2. `ToolRegistry` non-determinism and repeated work

`tools` is a `HashMap`, so `generate_tool_definitions()` emits the tool list in a different random
order on every request — needlessly non-deterministic for the model — and the schemars schemas are
regenerated on every chat round. Use a `Vec`/`BTreeMap` and build the definitions once.

### L3. Tools run synchronously inside the async runtime

`process_tool_call` is a sync call inside `Agent::run` (async). `CompileTool` can block a runtime
thread for minutes. Acceptable for a single-user CLI, but `tokio::task::spawn_blocking` (or making
`Tool::execute` async) would be the clean fix.

### L4. Command handling inconsistencies

- `/close` and `/clear` are matched with exact `eq` in the rustyline path (no trim — `"/close "` is
  sent to the model), but the `--end-of-prompt` path trims. Trim in both.
- In `--end-of-prompt` mode the line containing the sentinel is dropped entirely, not just the
  sentinel marker.
- `Err(_) => break` in the rustyline loop treats Ctrl-C (`Interrupted`) as exit; conventionally it
  should clear the line and re-prompt.

### L5. Hardcoded ANSI colors

Thinking is rendered with a hardcoded `\x1b[38;2;10;140;75m` regardless of TTY or `NO_COLOR`. Gate on
`std::io::stdout().is_terminal()`.

### L6. Packaging / repo hygiene

- `.gitignore` lists `Cargo.lock` — for a binary crate the lockfile should be committed for
  reproducible builds.
- No `rust-toolchain.toml` despite `#![feature(...)]`: on stable the build fails with a confusing
  error rather than a clear "nightly required" message. (C2's fix removes the nightly requirement
  entirely, which is the better outcome.)
- `Cargo.toml` lacks `license`, `description`, `repository` metadata (the MIT `LICENSE` file exists).
- `tokio` with `features = ["full"]` is heavy; `rt-multi-thread`, `macros`, `time` would do.

### L7. README drift

- `CompileTool` is registered in `main.rs` but missing from the README tool table; the architecture
  list omits `compile_tool.rs` and `security_error.rs`.
- The README says "The agent is instructed to search stored skills before answering a task" — but
  the default `system_prompt` is `""`; the only nudge is inside the tool descriptions. Either ship a
  default system prompt or soften the claim.
- The stats line prints a placeholder: `used (-/-)`.

### L8. Minor code hygiene

- Commented-out code left in `agent.rs` (`//thinking: …`, debug `eprintln!`s) and
  `chat_history.rs`/`config.rs` (`//pub repeat_penalty`).
- `compile_rust` returns `Ok(out.to_string())` — redundant clone of a `String`.
- `ChatHistory::get()` deep-clones the whole history on every request; `serde_json::Value` clones
  are not free at 32 K context.
- `security.rs` prints denial diagnostics to stdout (`println!`) — should be part of the error, or
  go to stderr.
- `Agent::run` returns early on a request error but leaves the user message in history, so the
  next turn starts with a dangling user message.
- `ReadTool`: `num_lines: 0` is treated as "whole file" rather than "zero lines" (quirky, worth a
  doc comment or explicit handling).

---

## What's done well

- **The `tool!` macro** is the best part of the codebase: params struct + doc comments → JSON schema
  the model sees, with zero per-tool boilerplate. Adding a tool is genuinely a 10-line exercise, as
  the README promises.
- **No shell anywhere.** Every subprocess gets an explicit argv vector; shell injection is
  structurally impossible. `WebFetch` even passes `--` before the URL.
- **The skills store** (`skills.rs`) is engineered well beyond the rest of the project: the
  hand-rolled YAML parser documents exactly what it does and does not support, quoting round-trips,
  name sanitization reduces skill paths to `[a-z0-9-]` (making the store escape-proof by
  construction), broken skills are reported instead of aborting listings, and it has real unit tests.
- **The NDJSON stream parser** handles chunk boundaries correctly (buffer + newline scan + trailing
  flush after EOF) — a place where naive implementations usually lose data.
- **Path confinement works for the cases it claims**: `../../etc/hostname`, `/etc/hostname` and
  `//etc/hostname` are all rejected; `src/../Cargo.toml` is allowed **[all reproduced]**.
- Tool *errors* are fed back to the model as text rather than aborting — the right design for an
  agent loop.

---

## Status of previously flagged issues

| Issue | Status |
| --- | --- |
| Skill tools failing `is_inside_cwd` for a global data dir | **Fixed** — cwd check removed; sanitization-based safety documented in `skills.rs` |
| `SearchAndReplace` leaving stale bytes when new content is shorter | **Fixed** — `set_len(0)` + `seek(0)` before write |
| `"assist"` role string | **Fixed** — `chat_history.rs` uses `"assistant"` |
| `/close` not trimmed | **Fixed** in `--end-of-prompt` mode; still untrimmed in the interactive path (L4) |
| Symlink bypass in `is_inside_cwd` | **Open** (C2) |
| Tool-round cap | **Open** (H2) |
| Output truncation | **Open** (M2) |
| `GrepTool` operand | **Open** in the literal sense, but GNU grep searches cwd without an operand; reframed as portability + `target/` noise (M5) |

---

## Prioritized recommendations

1. **Reject non-http(s) URLs in `WebFetch`** and add a curl timeout. (C1 — one line, closes a
   confirmed arbitrary-file-read.)
2. **Switch `is_inside_cwd` to `canonicalize`** — closes the symlink bypass *and* drops the nightly
   requirement. (C2)
3. **Handle malformed tool calls gracefully and cap tool rounds** in `Agent::run`. (H1, H2)
4. **Check exit status + stderr in all subprocess tools** via a shared helper. (H3)
5. **Make `SearchAndReplaceTool` fail on no-match and reject empty `oldcontent`**; drop the
   full-file `println!`. (M1)
6. **Truncate tool results centrally** in `process_tool_call`. (M2)
7. **`#[serde(default)]` the config** and stop panicking on missing `HOME`. (M3)
8. **`rl.add_history_entry`** after each line. (M4)
9. **Exclude `target/`/`.git` from grep/find**; add an optional path parameter. (M5)
10. Fix the model-facing typos and README drift (L1, L7); commit `Cargo.lock`.

### Testing gap

Only `skills.rs` has tests. The highest-value additions, in order: `is_inside_cwd` (traversal,
absolute paths, symlinks — regression protection for C2), `ChatHistory` ordering across tool rounds,
and a parser test for malformed tool-call payloads (H1). The agent loop itself would benefit from a
mock HTTP layer, but the three above are cheap and target the confirmed bugs.

### Licensing note

The workspace is a Rust rewrite of the GPL-3.0 Python project `Adrenocrom/sven` and is distributed
as MIT. That is defensible only as long as the implementation is genuinely independent of the GPL'd
code (per the earlier license analysis, the architecture diverges enough to support independence).
Worth documenting the provenance basis (e.g. a NOTICE or a paragraph in the README) so the claim
doesn't rest on institutional memory.