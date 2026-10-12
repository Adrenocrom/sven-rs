# Code Review — sven-rs (round 6)

**Date:** 2026-10-12
**Scope:** full workspace — `src/main.rs`, `src/sven/**` (agent, backend, chat_history, config,
macros, security, security_error, skills, stats, term, tool, tool_registry, 16 tool files,
`mcp/{mod,transport}.rs`), `Cargo.toml` (0.3.0), `README.md`, the **uncommitted `config.rs` diff**
(`num_predict` added, `ChatOptions::default` reworked), and the repo root (`output.txt`,
`review.html`, `test`). `README.md` was updated as part of this round (tool table completed,
notification behavior and options semantics documented, requirements list synced) — the review
below covers the tree *including* that doc sync.
**Method:** read every source file; verified the build (`cargo build` passes, exit 0 — with one
new warning, see §3 L1); findings marked **[reproduced]** were verified live by calling the tools
themselves (ManPageTool, GrepTool, CompileTool); findings marked **[code analysis]** are reasoned
from the code. The test suite (68 `#[test]`/`#[tokio::test]` functions across 13 files — round 5
reported 60, a recount says the earlier figure was low; nothing was added since, per `git diff`)
was again **not executed** — CompileTool still runs `cargo build` only (carried L2).

This is the sixth round, and the headline is a first: **a round-5 fix regressed.** The guard
added to address round-5 M1 (`if !message.content.is_empty()` before `history.assistant`) skips
the assistant echo for *every* tool round — tool-call rounds normally carry empty content — so
the `role: "tool"` results that follow dangle without their assistant message. On OpenAI/vLLM
that means a 400 and a dead run on the *happy path*, not the malformed-call edge case round 5
was about; on Ollama the model loses the record of its own calls. Tool calling — the agent's
core feature — is now broken on two of three backends. Add two more self-inflicted wounds in
the same area: `MAX_TOOL_OUTPUT` grew from 10 000 to **100 000** characters (a typo with a
ten-fold effect, §2 M2), and the uncommitted config change defaults `max_tokens` to the full
`num_ctx`, which makes vLLM/OpenAI reject requests out of the box (§2 M3). The good news: the
longest-standing finding (round-4 M1, the serde aliases) is still exactly three `#[serde(alias)]`
lines away, and the security boundary held again **[reproduced]**.

## Summary

The project's small-item trajectory is intact — `num_predict` finally exposed, README gaps
closed, security checks still passing — but this round's changes were made *in* the two most
fragile spots the last review flagged (the history echo and the token caps) and both edits went
the wrong way. The systemic items are unchanged: round-4 M1 (serde aliases, third round
running), unbounded history (now *more* urgent — a single tool result may again flood the
context, see M2), no chat-client/subprocess timeouts, and repo-root debris for the third round
running (`output.txt`, `review.html` still present, plus a new file literally named `test`).

---

## §1 Round-5 findings — verification

| # | Round-5 finding | Status |
|---|---|---|
| M1 (r4) | `Backend`: documented serde aliases do not exist | ❌ open — 3rd round running. The doc comment (`backend.rs:11–14`) still promises "The capitalized forms … are kept as aliases so config files written before the rename keep parsing"; there is still no `#[serde(alias)]`; `from_str_accepts_the_same_names_as_serde` still asserts what serde does not accept; the serde-side test (`"backend": "Ollama"`) was still not added |
| M1 (r5) | Malformed-tool-call recovery loop kills OpenAI/vLLM runs | 🟡 **fix attempted, regressed** — the new `if !message.content.is_empty()` guard (`agent.rs`, `run_rounds`) skips the assistant echo for *every* tool round, not just malformed calls. The original malformed-call path is still broken too. See §2 M1 — this is now the top priority |
| L1 | Run-finished notifications unconditional, undocumented | 🟡 half fixed — the README now documents the behavior, `NotifySendTool` and `GitDiffTool` are now in the tool table and the requirements list is complete (doc sync done this round); the code is unchanged: no config flag, no TTY gate, still fires on errored runs and once per piped prompt |
| L2 | Two SSE parsers, chat one drops legal frames | ❌ open — `backend.rs::process_line` still requires `data: ` with a space and decodes line-by-line; `mcp/transport.rs::parse_sse` still handles both correctly and is still not shared |
| L3 | Input history fragile: fresh-install save failure, SIGINT loss | ❌ open — `data_dir` is still created lazily by `StatsStore::save` after the first run (`init_skills_dir` still only computes the path), history is still saved only at clean REPL exit, Ctrl-C during streaming still loses the session's lines |
| L4 | Repo-root debris: `output.txt`, `review.html` | ❌ **worse** — both still present, plus a new file named `test` (captured `cargo build` output). `.gitignore` still only `/target`. See §3 L2 |
| L5 | Grab-bag (stats last-writer-wins comment, "run finished" on errors, `mcp … connected … \n` stray space + stdout, system-prompt leading newline, O(n²) buffer drain) | ❌ open — all five verified still present |

### Carried from earlier rounds (still open, unchanged)

| # | Finding (origin) | Note |
|---|---|---|
| M3 | grep exit 1 reported as error, not "no matches" (r3) | **[reproduced]** for the fourth round running: a no-match search returns `grep exited with exit status: 1:` |
| M5 | Unbounded history; `MAX_TOOL_ROUNDS` = 250 unexplained (r3) | No trimming against `num_ctx`. Aggravated this round: `MAX_TOOL_OUTPUT` was raised 10× (§2 M2), so the one cap that existed is now nearly ineffective |
| M6 | No timeouts on the chat client / subprocess tools (r3) | `Client::new()` in `Agent::new`; `subprocess::run` unbounded. The MCP stack remains fully timeout-covered |
| r4 L1 | OpenAI answers end with a double newline | The redundant `ends_with("\n")` block after `end_content()` is still in the `finish_reason` arm of `process_json_openai` |
| r4 L5 | Ollama error objects inside a 200 stream are dropped | `process_json_ollama` still has no `json.get("error")` check (the OpenAI path does) |
| L1 | `n: 0` replaces nothing, reports success (r3) | `replacen(.., 0)` in `edit_tool.rs` |
| L2 | CompileTool: case-sensitive enum, build-only (r3) | **[reproduced]** again: `language: "rust"` — the very value the tool's own description suggests — fails with `unknown variant 'rust', expected one of 'Rust', 'UefiRust', 'Java', 'Dotnet', 'Python'`. No `Test` variant, so no review round has ever executed this repo's tests |
| L3 | ManPageTool: "first page" description; roff overstrikes (r3) | No `col -b` |
| L4 | Full params JSON printed per tool call (r3) | `process_tool_call` still prints `params` untruncated — and there is now a dead `MAX_TOOL_PARAMS_OUTPUT = 80` constant next to it (§3 L1), so the fix was started and abandoned |
| L5 | FindTool exclusions anchored to `./` never match a given `path` (r3) | `-not -path "./target/*"` |
| L6 | Non-interactive mode: banner/thinking/tool lines share stdout with the answer (r3) | Unchanged |
| L7 | Skills-store writes follow symlinks (r3) | `fs::write` in `add_skill`/`update_skill` |
| L10 | `max_tokens` no-op on Ollama (r3) | Still serialized into the `options` envelope where Go silently ignores it. The *second* half (`num_predict` unexposed) is now fixed — see §4 |
| L11 | Subprocess diagnostics in the user's locale (r3) | No `LC_ALL=C` anywhere |
| L12 | Hygiene grab-bag (r3/r4) | Dead code now *commented out* instead of deleted (`backend.rs` debug `println!`, `main.rs` history-load `eprintln!`, `term.rs::green`), `impl ToString for Backend`, `endpoint(&self, host: &String)`, the "is recieved" typo (`main.rs:38`), `ReadTool` `(n as usize) + offset` overflow, `skill_spec.md` referenced but absent (`skills.rs:2`, `skill_tools.rs:1`), `if let Err(_)` clippy nit |
| MCP L1–L6 | SIGTERM skipped in stdio Drop, no `notifications/cancelled`, negotiated version echoed unvalidated, SSE stream held open costs the full timeout, no HTTP DELETE on exit, `inputSchema` guard checks `is_object()` not `type == "object"` (2026-10-06) | All verified still present |
| Async nits | Missing EOF newlines, `ChildStdin::from_std` panics outside a runtime, blocking Drop ~2 s, no `User-Agent`, `subprocess.rs` doc lists pandoc (2026-10-06) | All carried |

---

## §2 New findings — medium

### M1. The round-5 M1 "fix" drops the assistant tool-call echo for every tool round — tool calling is now broken on OpenAI/vLLM **[code analysis]**

**Location:** `src/sven/agent.rs::run_rounds` (the `if !message.content.is_empty()` guard), `src/sven/chat_history.rs::assistant`/`tool`

Round 5 asked for *validation before history*: parse the calls first, echo only the parseable
ones, report dropped calls via `assistant_note`. What landed instead is a content-emptiness
guard:

```rust
if !message.content.is_empty() {
    self.history.assistant(&message);
}
```

But a tool-call round normally has **empty content** — OpenAI streams `delta.content: ""`
while the `tool_calls` fragments arrive, and Ollama sends `message.content: ""` alongside
`tool_calls`. The guard therefore skips `history.assistant` exactly when it matters most:

1. The model emits tool calls (content empty). The assistant message carrying `tool_calls` is
   **never recorded**.
2. The tool results are recorded: `history.tool(...)` pushes `role: "tool"` messages into
   `tool_history`.
3. The next round's request is `[system, user, tool, tool, …]` — tool messages with no
   preceding assistant message. OpenAI-compatible servers reject this with a 400
   ("messages with role 'tool' must be a response to a preceding message with
   'tool_calls'"); litellm in front of vLLM does the same validation.
4. `RunStatus::Error`, run dead — on the *happy path*, the first tool call, not the malformed
   edge case round 5 was about.

On Ollama (the default backend) no 400 arrives, but the model now sees tool results with no
record of what it asked for — multi-round tool use degrades silently. The original round-5 M1
scenario is also still broken: a malformed call still produces a `role: "tool"` message
("malformed") with no assistant echo to anchor it.

A second, smaller side effect of the same guard: a final answer that is *pure thinking*
(content empty, no tool calls — exactly what a reasoning model hitting `max_tokens` during
thinking produces) leaves the user message unanswered in history, so the next prompt starts
with two consecutive `user` messages.

**Fix** — the guard tests the wrong field. The correct condition is
`if !message.tool_calls.is_empty() || !message.content.is_empty()`, and the tool-call branch
should do what round 5 actually proposed: validate all calls with `parse_tool_call` *before*
anything enters history, echo only the parseable ones (synthesizing an id when the fragment
carried none, so the tool result can carry `tool_call_id`), and report dropped calls via
`assistant_note`. Then add the interaction test round 5 asked for — feed a stream with a tool
call and empty content through `Backend` + `ChatHistory` and assert the next request body
contains the assistant `tool_calls` echo. That test would have caught this regression the day
it was written.

### M2. `MAX_TOOL_OUTPUT` raised from 10 000 to 100 000 characters — a typo that guts the truncation feature **[code analysis]**

**Location:** `src/sven/agent.rs:22` — `const MAX_TOOL_OUTPUT: usize = 10_0000;`

`10_0000` is 100 000. Round 5 introduced this cap at 10 000 chars and the review praised it;
the constant's own doc comment still reads *"Larger outputs are truncated so a single tool
(e.g. a full man page) cannot flood the model's context window"* — which is exactly what
100 000 chars now does: at roughly 3–4 chars per token that is ~25–30 K tokens, most of the
default 32 K window, from a *single* tool result. The unit test (`truncates_long_outputs`)
uses the constant, so it stays green at any value — nothing pins the intended magnitude.

**Fix** — revert to `10_000` (one character), or make the cap configurable and derived from
`num_ctx`. If the raise was deliberate (e.g. to stop truncating file reads), then the doc
comment, the README sentence and the round-5 rationale all need updating — as it stands the
code contradicts its own documentation.

### M3. The uncommitted config change defaults `max_tokens` to the full `num_ctx` — OpenAI/vLLM reject requests out of the box **[code analysis]**

**Location:** `src/sven/config.rs::ChatOptions::default` (uncommitted diff)

```rust
let num_ctx = 32000;
let max_tokens = num_ctx;        // → Some(32000)
let num_predict = num_ctx / 2;  // → Some(16000)
```

`num_predict` is a genuine improvement (§4). But `max_tokens: Some(32000)` is sent verbatim as
the top-level `max_tokens` on every OpenAI-compatible request (`request_body`), and an output
cap equal to the *input* window is a request the server must refuse:

- **vLLM** validates `prompt + max_tokens` against the model's context: with the README's own
  example (Qwen2.5-7B, 32 K context) the system prompt + tool definitions + 32 000 exceeds the
  window → 400 on *every* request, before a single token is generated.
- **OpenAI** rejects `max_tokens` above the model's output cap (16 384 for gpt-4o-class
  models) → 400.

A fresh install writes these defaults to `sven.json` (`create_default`), so the footgun
persists. The old default (`max_tokens: None` → server default) was safer.

**Fix** — default `max_tokens` back to `None` (or a small fraction of `num_ctx`, e.g. 4096);
keep `num_predict` for Ollama. Since the change is still uncommitted, this is the cheapest
possible moment to fix it. Add a config test pinning the defaults —
`partial_config_keeps_defaults_for_missing_fields` asserts `num_ctx` only, so nothing would
have caught a default flipping from `None` to `Some(32000)`.

---

## §3 New findings — low

### L1. Dead constant and a new build warning — the tree is no longer warning-clean **[reproduced]**

**Location:** `src/sven/agent.rs:25`, `src/sven/config.rs:3`

- `const MAX_TOOL_PARAMS_OUTPUT: usize = 80;` is declared and never used — an abandoned start
  on carried L4 (the per-call params dump), which itself remains unfixed: every
  `ReplaceFileTool` still prints its whole `newcontent` to the terminal.
- `cargo build` now emits `warning: unused import: PathBuf` (`config.rs:3`). The captured
  output sitting in the repo-root `test` file (see L2) contains this same warning, so the
  debris and the warning were created in the same session.

**Fix** — either implement the params truncation (the constant suggests 80 chars was the
intended cap) or delete the constant; drop the unused import. Zero-warning builds are worth
keeping: a warning that is always present is one that will hide the next one.

### L2. Repo-root debris, third round running — now with a file named `test` **[code analysis]**

**Location:** repo root

Round-5 L4 named `output.txt` (raw vLLM SSE transcript) and `review.html` (rendered round-4
review duplicate). Both are still here, and the pattern repeated again:

- `test` — a plain file containing captured `cargo build` output (the L1 warning plus
  "Finished dev profile"). Almost certainly a shell redirect gone sideways
  (`cargo build … > test`), and confusingly named: it shadows the conventional `tests/`
  directory name without being either a directory or a test.
- `review.html` is now doubly stale — it renders round 5, and this file supersedes it.
- `.gitignore` still contains only `/target`.

The root cause is unchanged from round 5's diagnosis: the agent's cwd is the repo, scratch
output lands in the root, and nothing sweeps it before a commit. Delete all three files (or
at minimum gitignore them), and consider a pre-commit check for untracked root files — three
rounds in a row is a process finding, not bad luck.

---

## §4 What's good

- **`num_predict` is finally exposed** (round-3 L10's second half, feature idea 15): an
  Ollama output cap configurable from `sven.json`, flowing through the existing `options`
  envelope with no new plumbing. The `ChatOptions::default` rework that came with it derives
  every cap from a single `num_ctx` variable — one place to retune, nothing to drift.
- **The README doc sync landed** (this round): all 19 built-in tools are in the table (was
  17 — `GitDiffTool` and `NotifySendTool` were the gaps), the run-finished notification
  behavior is documented for the first time (including the >3 min → critical urgency rule),
  the `options` semantics are spelled out per backend (`num_ctx`/`num_predict` Ollama-only,
  `max_tokens` OpenAI-compatible-only), the requirements list now names `git`, `notify-send`
  and the per-language build tools, and `stats.rs` appears in the architecture list.
- **The security boundary held again [reproduced]:** `ManPageTool("/etc/passwd")` is still
  rejected with the round-3 message, and the argument-injection guards (`as_operand`, `--`
  markers, `validate_name`) are all still in place and still unit-tested.
- **The MCP client remains the strongest subsystem** — timeouts on every exchange, 404
  session-expiry recovery, refusal of server→client requests, lenient result rendering — all
  unchanged and all still covered by the mock-transport tests.
- **The stats store still fails soft in every direction**, and the per-model/per-host
  breakdown from round 5 is unchanged and correct.
- Test suite: 68 functions across 13 files by this round's recount (round 5 said 60 — the
  earlier count was low, not the suite smaller).

## §5 Test coverage

68 test functions across 13 files. The gaps, in payoff order:

1. **The tool-call → history → next-request interaction test** — requested in round 5, still
   missing, and it would have caught this round's M1 regression on the day it was written:
   feed a stream with a tool call and empty content through `Backend` + `ChatHistory`, assert
   the next request body contains the assistant `tool_calls` echo and no orphaned tool
   messages.
2. **Still no serde test for `"backend": "Ollama"`** — third round running; it would have
   caught round-4 M1 and still would.
3. **No test pins the config defaults** — `partial_config_keeps_defaults_for_missing_fields`
   asserts `num_ctx` only, so `max_tokens` flipping from `None` to `Some(32000)` (§2 M3)
   changed nothing observable to the suite.
4. **No CI, and CompileTool still cannot run `cargo test`** (carried L2, reproduced again) —
   no review round has ever executed this repo's tests.
5. `notify_finished`'s urgency threshold (>3 min → critical) is still untested.

---

## §6 Feature ideas (carried forward, statuses updated)

Ordered by leverage. "Closes" lists the findings a feature would resolve as a side effect.

### A. Fixes that are features — do these first

1. **Context-window management** *(closes M5, most of L6's pain)* — still the single
   highest-value addition, and this round made it *more* urgent: with `MAX_TOOL_OUTPUT` at
   100 K chars (§2 M2) the one cap that existed is nearly gone. (a) A budget derived from
   `num_ctx` that trims the durable history (drop oldest tool rounds first, then oldest
   turns); (b) summarization — when history exceeds a threshold, summarize all but the last N
   messages and rebuild as system + summary + recent.
2. **`Test` variant for CompileTool** *(closes L2)* — `cargo test` / `mvn test` / `dotnet
   test` / `python -m pytest`; add `#[serde(rename_all = "lowercase")]` while touching the
   enum, and fix the description that suggests the lowercase value that fails. Unlocks
   running this repo's own 68 tests.
3. ~~Token tracking~~ — done (round 5) ✅.
4. **Timeouts everywhere** *(closes M6)* — `reqwest::Client::builder().connect_timeout(..)`
   for the chat client; a configurable `tool_timeout` in `subprocess::run` via
   `tokio::time::timeout`.
5. **Deterministic tool output** *(closes L3, L11)* — pipe man through `col -b`; `LC_ALL=C` on
   every `Command` in `subprocess::run`.
6. **Notification opt-out** *(closes the code half of r5 L1)* — config flag + TTY gate; skip
   on errored runs. The README half is done.
7. **Shared SSE parser** *(closes r5 L2)* — extract `parse_sse` for the chat path.
8. **Startup directory creation + per-run history save** *(closes r5 L3)* — one
   `create_dir_all` in `init_skills_dir`, save history after each run.

### B. Python-parity ports (from note2.md, prioritized)

9. **FIFO task queue** (`tasks.json`, `add_task`/`current_task`/`complete_task`/`list_tasks`/
   `cancel_task`) — lets the model decompose big goals and survive restarts.
10. **Cross-session conversation history** (`chat_history.json` + `--resume`) — *partially
    done*: the user's **input** history persists via rustyline; the conversation still
    does not.
11. **`touch` and `replaceline` tools** — `replaceline(path, line, text)` avoids resending
    whole files; good for small contexts.
12. **Git tools beyond diff** — `git status --short`, `git log -n`, opt-in `git add`/`commit`
    (write actions; consider a config gate, see idea 17). Would also fix GitDiffTool's
    unstaged-only surprise (carried r4 L2).
13. **Per-file compile** — `compilefile(path)`; useful for the edit-one-file-verify loop.
14. **Config profiles** — a `profiles` map selected by `SVEN_PROFILE`.
15. **Ollama options**: ~~`num_predict`~~ **done this round** ✅ (uncommitted); `keep_alive`
    (default `"10m"`), `repeat_penalty` still open *(closes the rest of L10)*.
16. **Mid-stream Ctrl-C** — catch interrupt during streaming, keep the partial answer and
    history, return to the prompt *(also closes the SIGINT-loss half of r5 L3)*.

### C. New directions

17. **Project context file** — auto-load `SVEN.md`/`AGENTS.md` from the cwd into the system
    prompt; pairs with skills (project rules local, cross-project knowledge in the store).
18. **One-shot mode + exit codes** — `sven -m "prompt"` for scripting, non-zero exit on
    backend/tool failure, everything but the answer to stderr when stdout is not a TTY
    *(closes the stdout-mixing half of L6)*.
19. **Slash commands** — `/help`, `/tools`, `/model <name>`, `/backend <name>`, `/mcp`,
    `/retry`, `/undo`.
20. **Confirmation mode + undo** — y/N before file-modifying tools, optionally with a
    unified diff; and/or automatic checkpoints before writes with `/undo`.
21. **Parallel tool execution** — read-only tools via `join_all`; MCP tools are naturally
    bounded by the per-server mutex.
22. **Retry with backoff** — one retry on 429/5xx/timeouts handles vLLM cold starts and rate
    limits.
23. **Startup doctor** — warn per missing binary (`curl`, `pandoc`, `ddgr`, `man`, `git`,
    `notify-send`); today a missing pandoc surfaces only as a confusing WebFetch error
    mid-task.
24. **Tool allow/deny list in config** — `"tools": {"deny": […]}` plus per-MCP-server enable
    flags.
25. **MCP polish** *(closes MCP L2, L5 + the frozen-toolset limitation)* —
    `notifications/cancelled` on timeout; HTTP DELETE on exit; handle
    `notifications/tools/list_changed` by re-listing.
26. **Skills auto-injection** — config-gated: search skills over the user prompt, prepend the
    top hit's body to the system prompt.
27. **Context-usage indicator** — `[8.2k/32k]` in the prompt prefix; trivial once idea 1's
    accounting exists.
28. **Multi-model routing** — cheap model for tool rounds, strong model for final answers;
    the `Backend` abstraction already separates the concerns.

| # | Idea | Value | Effort | Closes |
|---|------|-------|--------|--------|
| 1 | Context management | ★★★ | M | M5 |
| 2 | CompileTool `Test` | ★★★ | S | L2 |
| 4 | Timeouts | ★★ | S | M6 |
| 5 | `col -b` + `LC_ALL=C` | ★★ | S | L3, L11 |
| 6 | Notification opt-out | ★★ | S | r5 L1 (code half) |
| 7 | Shared SSE parser | ★ | S | r5 L2 |
| 8 | Startup dir + history save | ★★ | S | r5 L3 |
| 9 | Task queue | ★★ | M | — |
| 12 | Git status/log/commit | ★★ | S | r4 L2 |
| 17 | Project context file | ★★ | S | — |
| 18 | One-shot mode + exit codes | ★★ | S | L6 |
| 20 | Confirm mode + undo | ★★ | M | — |
| 25 | MCP polish | ★ | S | MCP L2, L5 |

---

## §7 Priorities

1. **New M1** — revert the `content.is_empty()` guard, implement the validate-before-history
   fix round 5 actually proposed (echo parseable calls, synthesize missing ids, drop via
   `assistant_note`), and add the interaction test. Tool calling is the agent's core feature
   and is currently broken on OpenAI/vLLM; on Ollama it is silently degraded.
2. **M3** — fix the `max_tokens` default *before committing* the config change; it is
   uncommitted right now, which is the cheapest it will ever be to fix. Pin the defaults
   with a test.
3. **M2** — one character: `10_0000` → `10_000`.
4. **Round-4 M1** — the serde aliases, third round running, still a few lines plus the
   missing test.
5. **M3 (grep)** — exit 1 as "no matches", reproduced for the fourth round running.
6. **Feature 1 (context management)** — the biggest lever for a local-LLM agent, and the
   `MAX_TOOL_OUTPUT` raise just removed the only cap that was holding the flood.
7. **Debris + warning-clean build** — delete `output.txt`, `review.html`, `test`; drop the
   dead constant and the unused import; widen `.gitignore`. Third round running is a process
   problem, not an accident.

Items 2, 3 and 7 are minutes of work; item 1 is an afternoon including the test; the only
substantial work on this list is item 6.