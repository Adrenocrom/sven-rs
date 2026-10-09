# Code Review — sven-rs (round 4)

**Date:** 2026-10-09
**Scope:** full workspace — `src/main.rs`, `src/sven/**` (agent, backend, chat_history, config,
macros, security, security_error, skills, term, tool, tool_registry, 15 tool files,
`mcp/{mod,transport}.rs`), `Cargo.toml`, `README.md`, `note.md`/`note2.md`/`note.html`,
`diff.txt`. New since round 3: the MCP client (reviewed separately 2026-10-06, round 4 of the
MCP stack), the async migration of `Tool::execute` and the whole MCP/HTTP path (reviewed and
approved 2026-10-06), the `Vllm` backend variant, `GitDiffTool`, and the round-3 fixes
(C1, M1, M2, M4, L8).
**Method:** read every source file; verified the build (`cargo build` passes, exit 0);
reproduced findings marked **[reproduced]** by calling the tools themselves (ManPageTool,
GrepTool, GitDiffTool, FindTool). Findings marked **[code analysis]** are reasoned from the
code. The test suite (47 `#[test]`/`#[tokio::test]` functions across 12 files) was **not
executed** — CompileTool still runs `cargo build` only (carried L2), so everything in
`#[cfg(test)]` is compile-verified at best.

This is the fourth round. Round 3's critical (ManPageTool arbitrary-file read) and its two
mediums (swallowed HTTP errors, `Backend` serde mismatch) are genuinely fixed — each with
regression tests, which is the part previous rounds kept asking for. This round found **one
new medium**: the `Backend` fix's own documentation claims capitalized serde aliases that do
not exist, so config files written before the fix now break in exactly the silent
whole-config-reverts-to-defaults way round 3 flagged. Plus five lows, and a long tail of
carried items. §6 adds the requested **feature ideas**.

## Summary

The project is in noticeably better shape than round 3. The security boundary held under
re-attack: `ManPageTool("/etc/passwd")` is rejected with a clear message, and the fix is
tested at both the helper and the `Tool::execute` level. The agent now surfaces HTTP errors
instead of silently doing nothing, `grep` speaks ERE, OpenAI reasoning models render on both
`reasoning` and `reasoning_content`, and the config round-trips the names the program itself
prints. The MCP client is the strongest subsystem in the codebase — hand-rolled JSON-RPC 2.0
that implements the Streamable HTTP MUSTs correctly and is thoroughly tested through a mock
transport.

The systemic weakness is still **context management**: history grows without bound, the tool
round cap is still an unexplained 250, and nothing in the pipeline knows how many tokens the
conversation occupies. That is also the highest-value *feature* to build next (§6), because
every long session eventually dies of it — silently on Ollama (server-side truncation), or as
a 400 on OpenAI-compatible servers.

---

## §1 Round-3 findings — verification

| # | Round-3 finding | Status |
|---|---|---|
| C1 | ManPageTool reads arbitrary files | ✅ **fixed** — `validate_name()` rejects leading `-` and any `/`; 3 tests incl. end-to-end through `Tool::execute`; README security section updated. **[reproduced]** `ManPageTool(name: "/etc/passwd")` → `invalid man page name /etc/passwd: page names never contain '/'` |
| M1 | HTTP error responses silently swallowed | ✅ **fixed** — status checked before parsing, body surfaced (truncated to 500 chars), `pop_user()` keeps failed turns from leaving a dangling prompt. Residual: an `{"error": …}` object inside a *200* NDJSON stream is still silently dropped on the Ollama path (see L5) |
| M2 | `"backend": "openai"` fails to parse | ✅ **fixed** — `#[serde(rename_all = "lowercase")]` + 3 config tests. **But** the fix's comments claim capitalized aliases are kept for old configs, and they are not — new M1 below |
| M3 | grep exit 1 reported as error | ❌ open — **[reproduced]** again this round: a no-match search returns `grep exited with exit status: 1:` instead of "no matches" |
| M4 | grep uses BRE | ✅ **fixed** — `-rniE`, description says "extended regex (ERE)" |
| M5 | unbounded history; round cap quietly 25 → 250 | ❌ open — `MAX_TOOL_ROUNDS` still 250, comment still doesn't justify it; no trimming against `num_ctx` |
| M6 | no timeouts outside curl | ❌ open — `Client::new()` in `agent.rs` has no connect/read timeout; `man`/`ddgr`/`cargo`/`grep`/`find`/`ls` unbounded. (The MCP stack *does* have timeouts everywhere — the gap is the chat client and the subprocess tools) |
| L1 | `n: 0` replaces nothing, reports success | ❌ open |
| L2 | CompileTool: case-sensitive enum, build-only | ❌ open — and aggravated: the param description now literally says "if **rust** is selected", lowercase, which fails to deserialize. A `Test` variant would also let the agent run this repo's own 47 tests |
| L3 | ManPageTool description / roff overstrikes | ❌ open — description still says "first page"; no `col -b` |
| L4 | full params JSON printed to terminal | ❌ open — every `ReplaceFileTool` still dumps its whole `newcontent` |
| L5 | FindTool exclusions anchored to `./` | ❌ open — `-not -path "./target/*"` never matches when `path` is given |
| L6 | non-interactive mode undocumented; stdout mixing | 🟡 half fixed — README flags table now documents `--prompt`/`--end-of-prompt`; banner, thinking, tool lines and errors still share stdout with the answer |
| L7 | skills-store writes follow symlinks | ❌ open — `fs::write` in `add_skill`/`update_skill` |
| L8 | OpenAI reasoning: only `delta.reasoning` | ✅ **fixed** — both keys tried, `reasoning` first |
| L9 | OpenAI token usage never surfaced | ❌ open — no `stream_options.include_usage`, no usage display |
| L10 | `max_tokens` no-op on Ollama; `num_predict` unexposed | ❌ open — README still silent; `max_tokens` is serialized into Ollama's `options` envelope where Go silently ignores it |
| L11 | subprocess diagnostics in the user's locale | ❌ open — no `LC_ALL=C` anywhere |
| L12 | hygiene grab-bag | 🟡 partial — `From<String>` dead code replaced by a `FromStr` clap actually uses; tokio `time` feature now genuinely used (MCP timeouts); `print_header` no longer shows `num_ctx` for OpenAI; README documents backend/api-key/MCP. Still open: commented-out code (`backend.rs` debug `println!`, `main.rs` `eprintln!`, `term.rs` dead `green`), `impl ToString` instead of `Display`, `endpoint(&self, host: &String)`, the user-visible "is recieved" typo in `--help`, `ReadTool` `(n as usize) + offset` overflow, `find -name` basename-only matching undocumented, `skill_spec.md` referenced but absent, `if let Err(_) =` clippy nit |

Carried from the two 2026-10-06 reviews (still open, unchanged): MCP **L1** stdio Drop skips
SIGTERM, **L2** no `notifications/cancelled` on timeout, **L3** negotiated `protocolVersion`
echoed into a header unvalidated, **L4** SSE stream kept open costs the full timeout, **L5**
no HTTP DELETE on exit, **L6** `inputSchema` guard checks `is_object()` not
`type == "object"` (verified still present in `tool_info`); async nits — 8 files missing EOF
newlines, `ChildStdin::from_std` panics outside a runtime, `StdioTransport::Drop` can block
~2 s, `discover()` doc duplication, `subprocess.rs` module doc lists pandoc, reqwest sends no
`User-Agent`.

---

## §2 New findings — medium

### M1. `Backend`: the documented serde aliases do not exist **[code analysis]**

**Location:** `src/sven/backend.rs` (enum doc comment, `FromStr` doc comment, test comment),
`src/sven/config.rs::load()`

Three comments claim that the capitalized enum names the pre-fix code required are "kept as
aliases so config files written before the rename keep parsing":

> `/// The capitalized forms the un-annotated enum used to require are kept as aliases …`
> `/// (lowercase, plus the capitalized aliases kept for old config files) …`
> `// capitalized aliases kept for old config files`

The enum carries only `#[serde(rename_all = "lowercase")]` — there is no `#[serde(alias)]`
anywhere. Only `FromStr` (the `--backend` CLI flag) accepts `"Ollama"`/`"OpenAI"`. A config
file written when the capitalized form was the *only* working spelling now fails to parse,
and `SvenConfig::load()` answers a parse error by returning `SvenConfig::default()` — the
entire setup (backend, model, host, data_dir) silently reverts to Ollama defaults, with one
stderr line as the only clue. That is precisely the failure mode round-3 M2 called medium;
the fix reintroduced it for pre-fix configs while claiming not to. The test name
(`from_str_accepts_the_same_names_as_serde`) is itself wrong — serde does not accept what it
asserts.

**Fix** — make the code match the comments (one attribute per variant):

```rust
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[serde(alias = "Ollama")]
    Ollama,
    #[serde(alias = "OpenAI")]
    OpenAI,
    Vllm,
}
```

(`Vllm` never existed capitalized-only, so it needs no alias.) Add the missing serde-side
test: `serde_json::from_str::<SvenConfig>(r#"{"backend": "Ollama"}"#)` must succeed.

**Systemic note:** `load()`'s parse-error → whole-config-defaults fallback is what turns every
config typo into a silent downgrade. For an agent with write tools, failing fast
(`eprintln!` + `std::process::exit(1)`) on a *malformed* config is safer than proceeding with
defaults; the per-field `#[serde(default)]` already handles missing fields gracefully.

---

## §3 New findings — low

### L1. OpenAI answers end with a double newline **[code analysis]**

**Location:** `src/sven/backend.rs::process_json_openai` (finish_reason block)

`end_content()` already prints a newline when the content does not end with one; the
`finish_reason` block then repeats the same check and prints a second. Content-only answers
render as `answer\n\n` on OpenAI/vLLM but `answer\n` on Ollama, and a thinking-only response
(finish_reason `length` mid-thought) gets a stray blank line from an empty `content`. Delete
the redundant block — `end_thinking`/`end_content` already do everything it does.

### L2. `GitDiffTool`: README gaps and unstaged-only semantics **[code analysis]**

**Location:** `src/sven/tools/git_tools.rs`, `README.md`

- The tool is missing from the README's tool table (18 tools registered, 17 listed), and
  `git` is missing from the README's requirements list (as are `cargo`/`mvn`/`dotnet`/
  `python` for CompileTool).
- `git diff .` shows **unstaged** changes only. Anything the user staged (`git add`) and all
  untracked files are invisible — for a "review my work" flow that is a surprise. Either
  document it in the description ("get the unstaged Git diff") or use `git diff HEAD .` to
  include staged changes. Untracked files need `git status --short` — a natural companion
  tool (§6).

### L3. Repo hygiene: `diff.txt`, `note.html`, `note.md` **[code analysis]**

`diff.txt` is a stray `Cargo.lock` diff dump from the async migration; `note.html` is a
rendered duplicate of `note.md`; `note.md` is superseded by `note2.md` — which is itself now
stale (its item 5, "Git `diff` tool — missing", closed by `GitDiffTool`). Delete all three
(note2.md's closed items can be marked done), or move them out of the repo root; they will
otherwise keep feeding the model stale information via `FindTool("*.md")`.

### L4. Empty assistant messages enter the durable history **[code analysis]**

**Location:** `src/sven/agent.rs::run()`

`self.history.assistant(&message)` runs before the tool-call check, so a turn that produced
neither content nor tool calls (a stream cut by `finish_reason: "length"` mid-thought, or a
mid-stream error) pushes `{"role":"assistant","content":""}` into the durable history. The
next request carries the empty message, and small local models handle those poorly. Skip the
push when `content` is empty and `tool_calls` is empty (the `length` warning already tells the
user what happened).

### L5. Ollama error objects inside a 200 stream are still dropped **[code analysis]**

**Location:** `src/sven/backend.rs::process_json_ollama`

Round-3 M1's optional half: the OpenAI path checks `json.get("error")` mid-stream, the Ollama
path does not. An `{"error": "…"}` NDJSON line inside a 200 response parses fine, matches no
field (`message`/`done`/`tool_calls`), and vanishes — the turn ends as an empty answer with no
diagnostic. Three lines mirroring the OpenAI check close it.

### L6. Grab-bag

- `Language` in `compile_tool.rs` has no `rename_all` **and** its description says "if rust
  is selected" — the model's first attempt is now guaranteed to fail (aggravates carried L2).
- `term.rs`: the commented-out `green()` and the commented color line in `thinking()` are
  dead weight; `tool_color`'s doc still says "Green" though it is per-backend now.
- `if ! stream_state…` (space after `!`) in `backend.rs` is not rustfmt style; a `cargo fmt`
  pass would also settle the `mod.rs` module ordering.
- `impl ToString for Backend` → `impl Display` (clippy `inherent_to_string`);
  `endpoint(&self, host: &String)` → `&str` (clippy `ptr_arg`).
- `SearchAndReplaceTool` truncates the file to zero and rewrites in place — a failed
  `write_all` leaves a partial file. Write-temp-then-rename costs four lines and removes the
  window (it would also close carried L7's symlink concern for this tool).

**Security notes (accepted risks — unchanged, keep documented):** TOCTOU between
`is_inside_cwd` and the open; CompileTool executes `build.rs` by design; WebFetch can reach
localhost/private ranges and follows redirects (curl ≥ 7.65.2 already excludes `file://`
from redirect protocols; `--proto-redir -all,http,https` would make it explicit).

---

## §4 What's good

- **The round-3 fixes are real, tested, and re-verified live.** The man-page fix fails
  closed, documents man-db's actual boundary, and — the detail that makes it trustworthy —
  has an end-to-end test through `Tool::execute` that asserts on the *message*, so it stays
  honest even on machines without `man`. The HTTP-status fix includes `pop_user()`, the
  detail that keeps failed turns from poisoning the next one.
- **The MCP client is the best code in the project.** Hand-rolled JSON-RPC 2.0 with every
  client-side Streamable HTTP MUST implemented, session-expiry re-init with retry-once,
  stale-response dropping, pagination loop guard, `isError` → error, binary-content
  placeholders, argv hardening (`--`, header validation, scheme allowlist, no redirects) —
  and a mock transport that tests the whole handshake sequence.
- **The async migration kept curl's semantics faithfully** — `--max-time` → request timeout
  + body deadline with partial-data-kept-on-timeout, `redirect(Policy::none())` so auth
  headers can't leak cross-origin — and the `!Send` traps it hit are documented where they
  bit, which is exactly where the next person needs them.
- **Tests now cover the things that break silently**: config round-trips (including the
  round-3 M2 regression), confinement (traversal, symlinks, dangling symlinks), stream
  shapes (fragment merging, `[DONE]`, mid-stream errors, `finish_reason`), tool-call
  parsing (string arguments, malformed payloads), MCP handshake/id-matching/SSE parsing.
- **The README caught up**: backend table, `SVEN_API_KEY` rationale, MCP section with honest
  behavior notes ("a hung server cannot freeze the agent", "requests the server sends to
  sven are refused"), a security section that states the argv caveat instead of overclaiming,
  and a provenance section that is candid about the Python origin.

## §5 Test coverage

47 test functions across 12 files. The gaps, in payoff order:

1. **No test deserializes a config with `"backend": "Ollama"`** — would have caught new M1.
2. **No test runs at all in CI** — there is no CI (no `.github/`), and CompileTool cannot run
   `cargo test` (carried L2). A `Test` language variant + a two-line GitHub Actions workflow
   would close both.
3. `agent.rs::run()`'s error paths (HTTP 4xx, empty response) are untested — `handle_chunks`
   is testable with a canned `Response` only via an abstraction that doesn't exist yet; at
   minimum, `truncate` + the `length` warning deserve a test.
4. `subprocess::run`'s exit-status formatting is untested (spawn `sh -c 'exit 1'`-style
   fixtures would do).

---

## §6 Feature ideas

Ordered by leverage. "Closes" lists the findings a feature would resolve as a side effect.

### A. Fixes that are features — do these first

1. **Context-window management** *(closes M5, most of L6's pain)* — the single highest-value
   addition. Two parts: (a) a character/token budget derived from `num_ctx` that trims the
   durable history (drop oldest tool rounds first, then oldest turns); (b) Python-parity
   summarization — when history exceeds a threshold, summarize all but the last N messages
   with a dedicated prompt and rebuild as system + summary + recent. Without this, every
   long session eventually degrades silently (Ollama truncates server-side; OpenAI 400s).
2. **`Test` variant for CompileTool** *(closes L2)* — `cargo test` / `mvn test` /
   `dotnet test` / `python -m pytest`. Also unlocks running this repo's own suite, which no
   review round has ever been able to do. Add `#[serde(rename_all = "lowercase")]` while
   touching the enum, and fix the description.
3. **Token tracking** *(closes L9; Python parity)* — request `stream_options:
   {"include_usage": true}` on OpenAI, read `usage` from the final chunk (note: that chunk
   carries `"choices": []`), accumulate lifetime totals in `tokens.json` under `data_dir`,
   print `in N out M | lifetime X|Y` for both backends.
4. **Timeouts everywhere** *(closes M6)* — `reqwest::Client::builder().connect_timeout(..)`
   for the chat client; a configurable `tool_timeout` applied in `subprocess::run` via
   `tokio::time::timeout` around `command.output()`.
5. **Deterministic tool output** *(closes L3, L11)* — pipe man through `col -b`; set
   `LC_ALL=C` on every `Command` in `subprocess::run` so diagnostics are English regardless
   of the user's locale.

### B. Python-parity ports (from note2.md, prioritized)

6. **FIFO task queue** (`tasks.json`, tools `add_task`/`current_task`/`complete_task`/
   `list_tasks`/`cancel_task`) — lets the model decompose big goals and survive restarts;
   today it must track the plan in conversation tokens, which fights feature 1.
7. **Cross-session history** (`chat_history.json` + `--resume`) — the durable history
   already exists in memory; persisting the non-tool messages is a small step for a big
   quality-of-life gain.
8. **`touch` and `replaceline` tools** — `touch` is trivial; `replaceline(path, line, text)`
   is a cheap precise edit that avoids resending whole files (good for small local models
   with small contexts).
9. **Git tools beyond diff** — `git status --short`, `git log -n`, and opt-in
   `git add`/`commit` (write actions; consider gating behind a config flag, see idea 17).
10. **Per-file compile** — `compilefile(path)`; Python parity, and useful for the
    edit-one-file-verify loop.
11. **Config profiles** — a `profiles` map in `sven.json` selected by `SVEN_PROFILE`,
    overlaying per-profile keys; one config for "local small model" vs "big hosted model".
12. **Ollama options**: `keep_alive` (default `"10m"` — avoids model reloads between turns),
    `repeat_penalty`, and `num_predict` *(closes L10)*.
13. **Mid-stream Ctrl-C** — catch interrupt during streaming, keep the partial answer and
    history, return to the prompt (Python does this; sven currently can't interrupt a
    streaming turn at all).

### C. New directions

14. **Project context file** — auto-load `SVEN.md`/`AGENTS.md` from the cwd into the system
    prompt (the CLAUDE.md/Cursor-rules pattern). Pairs naturally with skills: project rules
    local, cross-project knowledge in the skill store.
15. **One-shot mode + exit codes** — `sven -m "prompt"` for scripting (simpler than
    `--end-of-prompt` for the common case), non-zero exit on backend/tool failure, and route
    everything but the answer to stderr when stdout is not a TTY *(closes the stdout-mixing
    half of L6)*.
16. **Slash commands** — `/help`, `/tools` (list registered tools), `/model <name>` and
    `/backend <name>` (hot-swap without restart), `/mcp` (servers + tool counts), `/retry`
    (re-run the last turn), `/undo` (see 17).
17. **Confirmation mode + undo** — a config flag that asks y/N before file-modifying tools
    (`ReplaceFileTool`, `SearchAndReplaceTool`, `RemoveSkillTool`), optionally showing a
    unified diff; and/or an automatic checkpoint before each write (copy to
    `.sven-backups/` or `git stash create`) with `/undo` to revert. Cheap insurance that makes
    the agent safe to run on repos with uncommitted work.
18. **Parallel tool execution** — the agent already receives multiple `tool_calls` per round
    but executes them sequentially; `futures::future::join_all` over read-only tools would
    cut wall-clock time on multi-call rounds (MCP tools share a per-server mutex, so
    parallelism is naturally bounded).
19. **Retry with backoff** — 429/5xx/timeouts on the chat request currently end the turn;
    one retry with a short backoff handles vLLM cold starts and rate limits gracefully.
20. **Startup doctor** — check for `curl`/`pandoc`/`ddgr`/`man`/`git` and warn per missing
    binary (today a missing pandoc surfaces only as a confusing WebFetch error mid-task);
    optionally ping the server and list available models.
21. **Tool allow/deny list in config** — `"tools": {"deny": ["WebFetch", "WebSearch"]}` and
    per-MCP-server enable flags; a natural extension of the existing security model for
    users who want a file-only agent.
22. **MCP polish** *(closes MCP L2, L5 + the frozen-toolset limitation)* — send
    `notifications/cancelled` on timeout; HTTP DELETE on exit; handle
    `notifications/tools/list_changed` by re-listing and refreshing the registry (the
    registry already rebuilds definitions on `register`, so this is mostly plumbing).
23. **Skills auto-injection** — config-gated: at the start of a task, run the existing
    `search_skills` over the user prompt and prepend the top hit's body to the system
    prompt. The search already exists; this just wires it in.
24. **Context-usage indicator** — show `[8.2k/32k]` in the prompt prefix; trivial once
    feature 1's token accounting exists, and it tells the user when to `/clear`.
25. **Multi-model routing** (bigger) — a cheap model for tool-call rounds, a strong model
    for final answers (two entries in `sven.json`, switch on round type). The `Backend`
    abstraction already separates endpoint/wire-format concerns from the agent loop, so this
    is a config-shape change more than a code change.

| # | Idea | Value | Effort | Closes |
|---|------|-------|--------|--------|
| 1 | Context management | ★★★ | M | M5 |
| 2 | CompileTool `Test` | ★★★ | S | L2 |
| 3 | Token tracking | ★★ | S | L9 |
| 4 | Timeouts | ★★ | S | M6 |
| 5 | `col -b` + `LC_ALL=C` | ★★ | S | L3, L11 |
| 6 | Task queue | ★★ | M | — |
| 7 | Cross-session history | ★★ | S | — |
| 8 | touch / replaceline | ★ | S | — |
| 9 | Git status/log/commit | ★★ | S | L2 (docs) |
| 14 | Project context file | ★★ | S | — |
| 15 | One-shot mode + exit codes | ★★ | S | L6 |
| 17 | Confirm mode + undo | ★★ | M | — |
| 22 | MCP polish | ★ | S | MCP L2, L5 |

---

## §7 Priorities

1. **M1** — add the three `#[serde(alias)]` attributes (or fix the three comments) plus the
   missing serde test; then decide whether `load()` should exit on a malformed config
   instead of silently defaulting. *Do first — the code currently contradicts its own
   documentation.*
2. **M3** — grep exit 1 as "no matches" (`run_allow` helper or match on exit code).
3. **L1/L4/L5** — three small agent-loop correctness items (double newline, empty assistant
   message, Ollama mid-stream errors).
4. **Feature 1 (context management)** — the biggest lever for a local-LLM agent; every
   other limitation is dwarfed by sessions that quietly degrade after ~a dozen tool rounds.
5. **Feature 2 + CI** — a `Test` variant and a two-line workflow; no review round has ever
   executed this repo's tests.
6. **Features 3–5** — small, independent, each closes a carried finding.
7. **L2/L3/L6 + repo hygiene** — README table/requirements, delete `diff.txt`/`note.html`/
   `note.md`, mark note2.md item 5 done.

Items 1–3 are a few lines each; none require architectural changes.