# Code Review — sven-rs (round 5)

**Date:** 2026-10-09
**Scope:** full workspace — `src/main.rs`, `src/sven/**` (agent, backend, chat_history, config,
macros, security, security_error, skills, **stats [new]**, term, tool, tool_registry, 16 tool
files, `mcp/{mod,transport}.rs`), `Cargo.toml` (now 0.3.0), `README.md`, `review.html`,
`output.txt`. New since round 4: the statistics subsystem (`stats.rs`, `statistics.json`,
`/stats` + `--stats`, the per-run summary line, `TokenUsage` accounting), `NotifySendTool` and
the agent's run-finished notifications, `MAX_TOOL_OUTPUT` truncation, the `finish_reason:
"length"` warning, rustyline input-history persistence, Ctrl-C re-prompt at the REPL, and
per-backend terminal colors.
**Method:** read every source file; verified the build (`cargo build` passes, exit 0, via
CompileTool); findings marked **[reproduced]** were verified live by calling the tools
themselves (ManPageTool, GrepTool); findings marked **[code analysis]** are reasoned from the
code. The test suite (60 `#[test]`/`#[tokio::test]` functions across 13 files, up from 47/12 in
round 4) was **not executed** — CompileTool still runs `cargo build` only (carried L2), so
everything in `#[cfg(test)]` is compile-verified at best.

This is the fifth round. The headline is uncomfortable: round 4's single medium — the `Backend`
serde-alias contradiction, listed as priority #1 — is **still open, unchanged**, while a feature
landed instead. To be fair, the feature is good: token tracking was implemented essentially as
round 4 proposed (and folded into `statistics.json` rather than a separate `tokens.json`, which
is the better shape), the stats store fails soft in every direction, and `NotifySendTool` is the
best-hardened subprocess tool in the tree. This round found **one new medium** — the
malformed-tool-call recovery loop, whose whole purpose is letting a broken call self-correct,
kills the run one round later on OpenAI-compatible backends — plus five lows, and the familiar
carried long tail. §6 carries the feature list forward with updated statuses.

## Summary

The project keeps improving in the small: errors surface, counts accumulate, output is
truncated, notifications fire. The two systemic items are unchanged and still dwarf everything
else: **round-4 M1** (three comments promise serde aliases that do not exist, so pre-fix
configs silently revert to whole-config defaults) and **unbounded history** — `MAX_TOOL_OUTPUT`
now caps a single tool result at 10 K chars, but nothing caps the conversation against
`num_ctx`, and `MAX_TOOL_ROUNDS` is still an unexplained 250. Every long session still
eventually degrades silently on Ollama or 400s on OpenAI-compatible servers. The security
boundary held: `ManPageTool("/etc/passwd")` is still rejected with the round-3 message
**[reproduced]**, and the MCP client remains the strongest subsystem.

---

## §1 Round-4 findings — verification

| # | Round-4 finding | Status |
|---|---|---|
| M1 | `Backend`: documented serde aliases do not exist | ❌ open — the three comments (`backend.rs:11–14`, `:258–260`, test at `:287`) still claim "capitalized aliases kept for old config files"; there is still no `#[serde(alias)]`; the test name `from_str_accepts_the_same_names_as_serde` still asserts what serde does not accept; the missing serde-side test (`"backend": "Ollama"`) was still not added |
| L1 | OpenAI answers end with a double newline | ❌ open — the redundant `ends_with("\n")` block after `end_content()` is still in the `finish_reason` arm of `process_json_openai` |
| L2 | GitDiffTool: README gap; `git diff .` is unstaged-only | ❌ open — still absent from the README tool table, and **NotifySendTool is now also missing** (19 built-ins registered, 17 listed); `git diff .` still shows unstaged changes only |
| L3 | Repo hygiene: `diff.txt`, `note.html`, `note.md` | 🟡 half fixed — all three deleted ✅, but replaced by new debris: `output.txt` (a raw vLLM SSE transcript) and `review.html` (a rendered duplicate of round-4 `review.md`). See new L4 |
| L4 | Empty assistant messages enter the durable history | ❌ open — `self.history.assistant(&message)` still runs before the tool-call check |
| L5 | Ollama error objects inside a 200 stream are dropped | ❌ open — `process_json_ollama` still has no `json.get("error")` check (the OpenAI path does) |
| L6 | Grab-bag (`Language` enum/description, `term.rs` dead code, `! ` spacing, `impl ToString`, `endpoint(&String)`, in-place file rewrite) | ❌ open — all six items verified still present |

### Carried from earlier rounds (still open, unchanged)

| # | Finding (origin) | Note |
|---|---|---|
| M3 | grep exit 1 reported as error, not "no matches" (r3) | **[reproduced]** again this round: a no-match search returns `grep exited with exit status: 1:` |
| M5 | Unbounded history; `MAX_TOOL_ROUNDS` = 250 unexplained (r3) | `agent.rs:18`; no trimming against `num_ctx`. `MAX_TOOL_OUTPUT` (new) caps one tool result, not the conversation |
| M6 | No timeouts on the chat client / subprocess tools (r3) | `Client::new()` at `agent.rs:166`; the MCP stack remains fully timeout-covered — the gap is the chat client and `subprocess::run` |
| L1 | `n: 0` replaces nothing, reports success (r3) | `replacen(.., 0)` in `edit_tool.rs` |
| L2 | CompileTool: case-sensitive enum, build-only (r3, aggravated r4) | No `rename_all`, no `Test` variant; the param description still says lowercase "rust", which fails to deserialize |
| L3 | ManPageTool description "first page"; roff overstrikes (r3) | No `col -b` |
| L4 | Full params JSON printed per tool call (r3) | `process_tool_call` prints `params` untruncated — every `ReplaceFileTool` still dumps its whole `newcontent` to the terminal |
| L5 | FindTool exclusions anchored to `./` never match a given `path` (r3) | `-not -path "./target/*"` |
| L6 | Non-interactive mode half documented; stdout mixing (r3) | Flags documented; banner/thinking/tool lines still share stdout with the answer |
| L7 | Skills-store writes follow symlinks (r3) | `fs::write` in `add_skill`/`update_skill`; weak threat in a config dir — hardening only |
| L10 | `max_tokens` no-op on Ollama; `num_predict` unexposed (r3) | Serialized into the `options` envelope where Go silently ignores it |
| L11 | Subprocess diagnostics in the user's locale (r3) | No `LC_ALL=C` anywhere |
| L12 | Hygiene grab-bag (r3/r4) | Dead code (`backend.rs` debug `println!`, `main.rs` `eprintln!`, `term.rs` `green`), `impl ToString for Backend` (`backend.rs:248`), `endpoint(&self, host: &String)` (`backend.rs:197`), the user-visible "is recieved" typo (`main.rs:38`), `ReadTool` `(n as usize) + offset` overflow, `find -name` basename-only matching undocumented, `skill_spec.md` referenced but absent (`skills.rs:2`, `skill_tools.rs:1`), `if let Err(_)` clippy nit |
| MCP L1–L6 | SIGTERM skipped in stdio Drop, no `notifications/cancelled` on timeout, negotiated version echoed into a header unvalidated, SSE stream held open costs the full timeout, no HTTP DELETE on exit, `inputSchema` guard checks `is_object()` not `type == "object"` (2026-10-06) | All verified still present (`transport.rs` Drop, `mod.rs:333`) |
| Async nits | Missing EOF newlines, `ChildStdin::from_std` panics outside a runtime, blocking Drop ~2 s, no `User-Agent`, `subprocess.rs` doc lists pandoc (2026-10-06) | All carried |

---

## §2 New findings — medium

### M1. The malformed-tool-call recovery loop is Ollama-only; on OpenAI/vLLM it kills the run one round later **[code analysis]**

**Location:** `src/sven/agent.rs::run_rounds` (tool-call loop), `src/sven/chat_history.rs::assistant`/`tool`

The design intent is right and documented: *"a malformed call becomes a tool result describing
the problem, so the model can correct itself instead of the agent crashing."* On
OpenAI-compatible backends the mechanism defeats itself:

1. A stream yields a tool call that fails `parse_tool_call` — missing name, non-string name,
   invalid JSON in `arguments`. That is not exotic: it is exactly what a call cut by
   `max_tokens` (`finish_reason: "length"`, mid-`arguments`) looks like after fragment merging,
   and small local models behind vLLM are this agent's target audience.
2. `self.history.assistant(&message)` has **already** pushed the raw call into the assistant
   echo — it runs before the parse loop, and it echoes `message.tool_calls` verbatim (only
   defaulting `type`).
3. The parse failure becomes a tool result. When the fragment carried no `id`, `chat_history::
   tool()` omits `tool_call_id` (it is only sent "when known").
4. The next round's request therefore carries an assistant message with a malformed
   `tool_calls` entry *and* a `role: "tool"` message without `tool_call_id`. OpenAI-compatible
   servers reject the whole request with 400 — the same litellm/`ChatCompletionMessage…`
   validation the `type: "function"` default was added for.
5. The run ends in `RunStatus::Error`. The model never sees the correction feedback the
   mechanism wrote for it.

The session recovers on the next prompt (`user()` clears `tool_history`), so this is a killed
run, not a bricked session — but the recovery loop simply does not exist on half the supported
backends, and it fails on precisely the malformed-call situation it was built for. Ollama
ignores both problems and works as designed.

**Fix** — validate before anything enters history: parse all calls first, echo only the
parseable ones (or synthesize missing ids), and report dropped calls via `assistant_note` —
the round-cap path already has that shape. This is the same "history accepts whatever the
stream produced" family as round-4 L4 (empty assistant messages); one validation step between
`handle_chunks` and `history.assistant` closes both. Add the missing test: feed a fragment
stream with a nameless call through `Backend` + `ChatHistory`, assert the next request body
contains no malformed `tool_calls` and no id-less tool message.

---

## §3 New findings — low

### L1. Run-finished notifications: unconditional, undocumented **[code analysis]**

**Location:** `src/sven/agent.rs::notify_finished` / `NOTIFY_CRITICAL_AFTER`, `README.md`

`notify_finished` fires after **every** run: scripted `--end-of-prompt` sessions (one
notification per piped prompt), Error runs (titled "Sven: run finished" over a failed request),
and sub-second runs alike. There is no config flag and no TTY gate; without a notification
daemon (or without `notify-send` installed) it prints `could not send notification: …` to
stderr after every single run. The README does not mention the behavior at all — a user
discovers it the first time a desktop popup appears.

Related doc drift in the same area: `NotifySendTool` is missing from the README tool table
(19 built-ins registered, 17 listed — `GitDiffTool` still missing too, carried), and
`notify-send`, `git`, and `cargo`/`mvn`/`dotnet`/`python` are all missing from the
requirements list.

**Fix** — a config flag (e.g. `"notifications": true`, default on), skip when the run errored,
gate on stdout being a TTY for the scripted mode, and sync the README (tool table,
requirements, one sentence on the behavior). The >3 min → critical urgency choice is good —
keep it.

### L2. Two SSE parsers with different strictness — the chat one silently drops legal frames **[code analysis]**

**Location:** `src/sven/backend.rs::process_line` vs `src/sven/mcp/transport.rs::parse_sse`

The chat path requires exactly `data: ` (with space): `line.strip_prefix("data: ")` silently
skips a `data:{…}` frame — legal per the SSE spec — and multi-line data frames can never
parse, because each line is decoded independently (a JSON split across two `data:` lines just
logs "couldn't decode JSON … skipping line"). The MCP transport's `parse_sse` handles both
correctly: optional space, multi-line join, blank-line flush, `event:`/`id:`/`retry:` ignored.

Nothing breaks today — OpenAI, vLLM and litellm all emit single-line `data: ` frames — but the
correct parser already exists in this codebase. Extract it (or share a helper) instead of
maintaining two behaviors; the chat path's line-splitting in `handle_chunks` can feed whole
frames to it the way `read_bounded`'s output already does for MCP.

### L3. The new input history is fragile: fresh-install save failure and SIGINT loss **[code analysis]**

**Location:** `src/main.rs` (history path, REPL exit), `src/sven/skills.rs::init_skills_dir`, `src/sven/stats.rs::save`

The rustyline history is saved only at clean REPL exit, and `<data_dir>` is created lazily by
`StatsStore::save` after the **first recorded run**. Consequences:

- Fresh install, user types `/close` before any run → `could not save history: No such file
  or directory` (the dir does not exist yet; `init_skills_dir` only computes the path, it
  creates nothing).
- Ctrl-C while `agent.run()` is streaming kills the process (default SIGINT disposition; the
  Ctrl-C re-prompt only covers the readline prompt) → every history line of the session is
  lost, because saving happens only after the loop breaks.

**Fix** — create `data_dir` at startup (`init_skills_dir` already computes the path; one
`create_dir_all`), and/or save the history after each run rather than at exit. The second
half pairs naturally with carried feature idea 13 (mid-stream Ctrl-C).

### L4. Repo-root debris again: `output.txt`, `review.html` **[code analysis]**

**Location:** repo root

Round-4 L3 (`note.md`, `note.html`, `diff.txt`) was fixed by deletion — and the pattern
repeated within one round:

- `output.txt` — a raw vLLM SSE transcript (the `data: {…}` lines interleaved with rendered
  thinking/content, i.e. the terminal with the commented-out debug `println!` in
  `backend.rs::process_line` enabled). Session debris.
- `review.html` — a rendered duplicate of round-4 `review.md`; stale the moment this file was
  written.

`.gitignore` still contains only `/target`. The root cause is structural: the agent's cwd is
the repo, so scratch output lands in the repo root and gets committed. Delete both files, and
either keep scratch out of the repo or gitignore it (`output.txt` at minimum). Worth checking
the root before every commit — this is the second round in a row for this finding.

### L5. Grab-bag **[code analysis]**

- `statistics.json` is last-writer-wins across concurrent sven instances (each `StatsStore`
  holds its startup image and rewrites the whole file). Acceptable for the purpose — worth a
  comment so it is a documented choice, not a surprise.
- The run summary and the notification say "run finished" even for `RunStatus::Error` runs —
  "run ended" (or skipping the notification on errors, see L1) would read better.
- `println!("mcp: '{}' connected ({} tools) \n", …)` goes to stdout — it mixes with piped
  answers (the L6 stdout-mixing family) and has a stray space before the newline.
- `ChatHistory::system` with an empty configured `system_prompt` produces `"\nCurrent Date: …"`
  — a leading newline in the system message. Cosmetic.
- `handle_chunks` drains the buffer with `buffer.drain(..=pos)` per line — O(n²) if a single
  chunk contains many lines. Fine in practice; an index walk with `split_off` would be linear.

---

## §4 What's good

- **Round-4 feature idea 3 (token tracking) landed, and better than proposed.** No separate
  `tokens.json` — usage is folded into `statistics.json` alongside runs/rounds/tool calls.
  The per-response `in <prompt> out <completion>` line, the per-run summary, and lifetime
  `/stats` all match the README examples, and the formatting edge cases are pinned by tests
  (`999_999` → `1M`, `59m59.9s` carrying into `1h 0m` instead of `59m 60s`).
- **The stats store fails soft in every direction a stats store should:** missing file →
  zero, corrupt file → stderr + zero, write failure → stderr but never breaks the finished
  run, `#[serde(default)]` keeps older files loading (with a test for exactly that).
- **`MAX_TOOL_OUTPUT` truncation** cuts on character boundaries (never mid-character) and
  appends a marker the model can see; malformed calls become self-describing tool results
  instead of panics — the intent is right, and §2 M1 is about where it falls short, not
  whether it exists.
- **`notify_finished` reuses `NotifySendTool` instead of duplicating argv construction**, and
  `NotifySendTool` itself is the best-hardened subprocess tool in the tree: enum urgency
  (typos and injection rejected at deserialization), `--` terminator, unit-tested argv, and
  an end-to-end empty-summary test that stays green without a daemon installed.
- **The `finish_reason: "length"` warning names the exact config knob to raise** — the kind
  of error message that turns a silent truncation into a one-line fix.
- **Test suite grew 47 → 60** across 12 → 13 files; the man-page fix's end-to-end test
  asserts on the *message*, keeping itself honest on machines without `man`.
- Ctrl-C at the prompt now clears the line and re-prompts instead of exiting.
- `note.md`, `note.html` and `diff.txt` were actually deleted.

## §5 Test coverage

60 test functions across 13 files. The gaps, in payoff order:

1. **Still no serde test for `"backend": "Ollama"`** — the exact test round 4 asked for; it
   would have caught round-4 M1 and still would.
2. **No test covers the malformed-call → history → next-request interaction** — new §2 M1
   lives exactly in the gap between `parse_tool_call`'s tests and `ChatHistory`'s tests.
3. **No CI, and CompileTool still cannot run `cargo test`** (carried L2) — no review round
   has ever executed this repo's tests. A `Test` variant plus a two-line GitHub Actions
   workflow closes both.
4. `Agent::run`'s error paths (HTTP 4xx → `pop_user`, the `length` warning) are untested.
5. `notify_finished`'s urgency threshold (>3 min → critical) is untested — trivial, but it
   is the only new branch with no coverage.

---

## §6 Feature ideas (carried forward from round 4, statuses updated)

Ordered by leverage. "Closes" lists the findings a feature would resolve as a side effect.

### A. Fixes that are features — do these first

1. **Context-window management** *(closes M5, most of L6's pain)* — still the single
   highest-value addition, and the new `MAX_TOOL_OUTPUT` makes it *more* visible, not less:
   one tool result is capped, the conversation still is not. (a) A budget derived from
   `num_ctx` that trims the durable history (drop oldest tool rounds first, then oldest
   turns); (b) summarization — when history exceeds a threshold, summarize all but the last N
   messages and rebuild as system + summary + recent.
2. **`Test` variant for CompileTool** *(closes L2)* — `cargo test` / `mvn test` / `dotnet
   test` / `python -m pytest`; add `#[serde(rename_all = "lowercase")]` while touching the
   enum, and fix the description. Unlocks running this repo's own 60 tests.
3. ~~Token tracking~~ — **done this round** ✅ (closes r3 L9; folded into `statistics.json`).
4. **Timeouts everywhere** *(closes M6)* — `reqwest::Client::builder().connect_timeout(..)`
   for the chat client; a configurable `tool_timeout` in `subprocess::run` via
   `tokio::time::timeout`.
5. **Deterministic tool output** *(closes L3, L11)* — pipe man through `col -b`; `LC_ALL=C` on
   every `Command` in `subprocess::run`.
6. **Notification opt-out** *(closes new L1)* — config flag + TTY gate + README sync; skip on
   errored runs.
7. **Shared SSE parser** *(closes new L2)* — extract `parse_sse` for the chat path.
8. **Startup directory creation + per-run history save** *(closes new L3)* — one
   `create_dir_all` in `init_skills_dir`, save history after each run.

### B. Python-parity ports (from note2.md, prioritized)

9. **FIFO task queue** (`tasks.json`, `add_task`/`current_task`/`complete_task`/`list_tasks`/
   `cancel_task`) — lets the model decompose big goals and survive restarts.
10. **Cross-session conversation history** (`chat_history.json` + `--resume`) — *partially
    done*: the user's **input** history now persists via rustyline; the conversation still
    does not.
11. **`touch` and `replaceline` tools** — `replaceline(path, line, text)` avoids resending
    whole files; good for small contexts.
12. **Git tools beyond diff** — `git status --short`, `git log -n`, opt-in `git add`/`commit`
    (write actions; consider a config gate, see idea 17). Would also fix GitDiffTool's
    unstaged-only surprise (carried r4 L2).
13. **Per-file compile** — `compilefile(path)`; useful for the edit-one-file-verify loop.
14. **Config profiles** — a `profiles` map selected by `SVEN_PROFILE`.
15. **Ollama options**: `keep_alive` (default `"10m"`), `repeat_penalty`, `num_predict`
    *(closes L10)*.
16. **Mid-stream Ctrl-C** — catch interrupt during streaming, keep the partial answer and
    history, return to the prompt *(also closes the SIGINT-loss half of new L3)*.

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
| 6 | Notification opt-out | ★★ | S | new L1 |
| 7 | Shared SSE parser | ★ | S | new L2 |
| 8 | Startup dir + history save | ★★ | S | new L3 |
| 9 | Task queue | ★★ | M | — |
| 12 | Git status/log/commit | ★★ | S | r4 L2 |
| 17 | Project context file | ★★ | S | — |
| 18 | One-shot mode + exit codes | ★★ | S | L6 |
| 20 | Confirm mode + undo | ★★ | M | — |
| 25 | MCP polish | ★ | S | MCP L2, L5 |

---

## §7 Priorities

1. **Round-4 M1** — three `#[serde(alias)]` attributes (or fix the three comments) plus the
   missing serde test; then decide whether `load()` should exit on a malformed config instead
   of silently defaulting. *Still first — it was priority #1 last round and is a few lines.*
2. **New M1** — validate tool calls before they enter history, so the self-correction loop
   works on OpenAI/vLLM too; add the interaction test.
3. **M3** — grep exit 1 as "no matches" (reproduced for the third round running).
4. **Feature 1 (context management)** — still the biggest lever for a local-LLM agent;
   `MAX_TOOL_OUTPUT` capped the puddle, the conversation is still the flood.
5. **Feature 2 + CI** — a `Test` variant and a two-line workflow; no review round has ever
   executed this repo's tests.
6. **New L1/L3/L4** — notification opt-out + README sync; create `data_dir` at startup and
   save history per run; delete `output.txt`/`review.html` and gitignore scratch output.
7. **The carried long tail** (§1) — none of it requires architectural changes.

Items 1–3 and 6 are small; the only substantial work on this list is item 4.