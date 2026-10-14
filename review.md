# Code Review — sven-rs (round 7)

**Date:** 2026-10-14
**Scope:** full workspace — `src/main.rs`, `src/sven/**` (agent, backend, chat_history, config,
macros, security, security_error, skills, stats, term, tool, tool_registry, **`sse.rs` [new since
round 6]**, 16 tool files, `mcp/{mod,transport}.rs` — **the transport was rewritten from curl to
reqwest**), `Cargo.toml` (0.3.0), `README.md`, and the repo root (`output.txt`, `review.html`,
`test` — all three still present). The working tree is clean: round 6's uncommitted `config.rs`
change was resolved by reverting the `max_tokens` default and committing.
**Method:** read every source file; `cargo build` passes and is **warning-clean again**
**[reproduced]** (round 6's unused-import warning is gone); findings marked **[reproduced]** were
verified live by calling the tools themselves (CompileTool, GrepTool, ManPageTool); findings marked
**[code analysis]** are reasoned from the code. The test suite — now **73** `#[test]`/`#[tokio::test]`
functions across 13 files, up from 68 — was again **not executed**: CompileTool is still build-only
(carried L2, reproduced for the fifth round running).

The seventh round's headline is the sixth round's headline, verbatim: **the M1 regression is still
not fixed.** The `if !message.content.is_empty()` guard in `run_rounds` is unchanged — the
assistant message carrying `tool_calls` is still never recorded for a normal tool round, the next
request is still `[system, user, tool, tool, …]`, OpenAI/vLLM still reject it, and tool calling —
the agent's core feature — remains broken on two of three backends **[code analysis]**. What makes
this round unusual is how much *good* work landed around it: the shared SSE parser (open since
round 5) finally exists, the MCP transport got a clean reqwest rewrite, and the `max_tokens`
default was reverted — real, substantial changes, none of them on the one-line guard at the top of
the last review's priority list. One new self-inflicted wound: the code fix for round-6 M3 was not
propagated to the README, which still presents `max_tokens: 32000` as *the default* — copy-pasting
the documented config block re-introduces the 400-on-every-request footgun the fix removed (§2 M2).

## Summary

The project's two longest-running subsystem-level complaints were closed this round — the duplicate
SSE parsers (r5 L2) and the curl-subprocess MCP transport — and the history layer is now ready for
the tool-call echo it still never receives. The regression that actually breaks the product on two
of three backends survived a full round untouched, the serde aliases hit their fourth round, and
the repo-root debris its third. The pattern worth naming: fixes land *near* the flagged spot but
not *on* it — the params-truncation constant was commented out instead of implemented, the
`max_tokens` default was fixed in code but not in the docs, `MAX_TOOL_OUTPUT` was re-tuned to a
third unexplained value. Finishing a fix means updating the doc comment, the README and the test
that pins it, in the same commit.

---

## §1 Round-6 findings — verification

| # | Round-6 finding | Status |
|---|---|---|
| M1 | `content.is_empty()` guard drops the assistant tool-call echo for every tool round | ❌ **open — 2nd round running.** The guard in `run_rounds` is byte-for-byte unchanged; tool calling remains broken on OpenAI/vLLM, silently degraded on Ollama; the malformed-call path (r5 M1) is still broken too; the interaction test is still missing. Still §2 M1, still priority 1 |
| M2 | `MAX_TOOL_OUTPUT` raised to 100 000 | 🟡 **re-tuned, not resolved** — now `35_000`, a defensible middle ground, but the third value in three rounds (10 000 → 100 000 → 35 000), changed silently, with no rationale, no doc update and still no test pinning the magnitude (§3 L1) |
| M3 | `max_tokens` default = full `num_ctx` | ✅ **fixed in code** — `ChatOptions::default` is back to `max_tokens: None` (and `num_predict: None`). But the README still shows `"max_tokens": 32000, "num_predict": 16000` as *the defaults* (§2 M2), and the defaults-pinning test round 6 asked for was not added |
| L1 | Dead constant + unused-import warning | 🟡 half — the build is warning-clean again **[reproduced]**; the dead constant is now *commented out* (`//const MAX_TOOL_PARAMS_OUTPUT …`), the exact anti-pattern carried L12 flags, and the params dump it was meant to fix (carried L4) is still unfixed |
| L2 | Repo-root debris (`output.txt`, `review.html`, `test`) | ❌ **open — 3rd round running.** All three files still present; `test` is now doubly stale (it contains the already-fixed warning), `review.html` renders round 5. See §3 L3 |

### Carried from earlier rounds (still open, unchanged unless noted)

| # | Finding (origin) | Note |
|---|---|---|
| M1 (r4) | `Backend`: documented serde aliases do not exist | ❌ 4th round running. The doc comment still promises capitalized aliases; serde still accepts lowercase only; the serde-side test still cannot be added. **New this round:** `FromStr` accepts `"Ollama"`/`"OpenAI"` but not `"Vllm"`, and the test name `from_str_accepts_the_same_names_as_serde` is now actively false (§3 L2) |
| M3 (r3) | grep exit 1 reported as error, not "no matches" | ❌ **[reproduced]** for the fifth round running: `grep exited with exit status: 1:` |
| M5 (r3) | Unbounded history; `MAX_TOOL_ROUNDS` = 250 unexplained | ❌ open. Slightly less urgent with the 35 K cap, but there is still no trimming against `num_ctx` |
| M6 (r3) | No timeouts on the chat client / subprocess tools | ❌ open. The chat client is still bare `Client::new()` — and now the *only* client without timeouts: the MCP transport builds its own reqwest client with per-request timeouts and `redirect::Policy::none()`. The chat client keeps reqwest's default redirect-following, so a redirecting chat endpoint can carry the `Authorization` header onward — the exact leak the MCP rewrite guards against |
| L1 (r4) | OpenAI answers end with a double newline | ❌ open — and now literally duplicated: `end_content` contains the `!ends_with("\n") → println!("")` block, and the `finish_reason` arm repeats the same three lines right after calling it |
| L5 (r4) | Ollama error objects inside a 200 stream are dropped | ❌ open — `process_json_ollama` still has no `json.get("error")` check (the OpenAI path does) |
| L2 (r4) | GitDiffTool shows unstaged changes only | ❌ open — still `git diff .` |
| L1 (r3) | `n: 0` replaces nothing, reports success | ❌ open — `replacen(.., 0)` in `edit_tool.rs` |
| L2 (r3) | CompileTool: case-sensitive enum, build-only | ❌ **[reproduced]** for the fifth round running: `language: "rust"` fails with `unknown variant 'rust', expected one of 'Rust', …`. Still no `Test` variant — no review round has ever executed this repo's tests |
| L3 (r3) | ManPageTool: "first page" description; roff overstrikes | ❌ open — no `col -b` |
| L4 (r3) | Full params JSON printed per tool call | ❌ open — `process_tool_call` still prints `params` untruncated; the intended cap is now a commented-out constant |
| L5 (r3) | FindTool exclusions anchored to `./` never match a given `path` | ❌ open — still `-not -path "./target/*"` |
| L6 (r3) | Non-interactive mode: banner/thinking/tool lines share stdout with the answer | ❌ open |
| L7 (r3) | Skills-store writes follow symlinks | ❌ open — `fs::write` in `update_skill` (a `SKILL.md` symlink inside the store can point outside it) |
| L10 (r3) | `max_tokens` no-op on Ollama | ❌ open — still serialized into the `options` envelope (documented in the README now, but still sent) |
| L11 (r3) | Subprocess diagnostics in the user's locale | ❌ open — no `LC_ALL=C` anywhere |
| L12 (r3/r4) | Hygiene grab-bag | ❌ open — new instance this round: `//const MAX_TOOL_PARAMS_OUTPUT`. Still present: commented-out `term::green` and Ollama thinking color, commented-out history-load `eprintln!` inside an empty `if let Err(_)` block, `impl ToString for Backend`, `endpoint(&self, host: &String)`, the "is recieved" typo (`main.rs:38`), `ReadTool` `(n as usize) + offset` overflow, `skill_spec.md` referenced but absent (`skills.rs:2`, `skill_tools.rs:1`) |
| L1 (r5) | Run-finished notifications unconditional | 🟡 unchanged — README half done in round 6; code half open: no config flag, no TTY gate, fires on errored runs and once per piped prompt |
| L2 (r5) | Two SSE parsers, chat one drops legal frames | ✅ **FIXED** — `sse.rs` (see §4). One parser, used by both the chat backends and the MCP transport, with tests for exactly the frames the old chat parser dropped |
| L3 (r5) | Input history fragile: fresh-install save failure, SIGINT loss | ❌ open — `init_skills_dir` still only computes the path, history is still saved only at clean REPL exit, Ctrl-C during streaming still kills the process and loses the session's lines |
| L5 (r5) | Grab-bag (stats last-writer-wins comment, "run finished" on errors, `mcp … connected … \n` stray space + stdout, system-prompt leading newline, O(n²) buffer drain) | ❌ open — all five verified still present |
| MCP L1 (r4) | stdio Drop skips SIGTERM | ❌ open — still close-stdin → 1 s → `kill()` (SIGKILL) |
| MCP L2 (r4) | No `notifications/cancelled` on timeout | ❌ open |
| MCP L3 (r4) | Negotiated protocolVersion echoed into a header unvalidated | ❌ open, **impact downgraded by the reqwest rewrite**: an invalid header value now fails the request with an error instead of curl's potential header smuggling. Still worth validating (printable ASCII, else fall back to `PROTOCOL_VERSION`) |
| MCP L4 (r4) | SSE stream kept open costs the full timeout | ❌ open (latency only) — `read_bounded` waits for EOF or the deadline with no early exit once a complete message arrived. The *failure mode* is gone, though: whatever arrived before the cut is kept and the call succeeds |
| MCP L5 (r4) | No HTTP DELETE on exit | ❌ open |
| MCP L6 (r4) | `inputSchema` guard checks `is_object()` not `type == "object"` | ❌ open — `tool_info` unchanged |
| Async nits (r4) | `ChildStdin::from_std` panics outside a runtime, blocking Drop ~2 s, no `User-Agent`, `subprocess.rs` doc lists pandoc | ❌ all carried |

---

## §2 New findings — medium

### M1. (Carried, 2nd round) The content-emptiness guard still drops the assistant tool-call echo — tool calling is still broken on OpenAI/vLLM **[code analysis]**

**Location:** `src/sven/agent.rs::run_rounds` — `if !message.content.is_empty() { self.history.assistant(&message); }`

Unchanged since round 6, so the analysis is unchanged: a tool-call round normally has **empty
content** (OpenAI streams `delta.content: ""` around the `tool_calls` fragments; Ollama sends
`message.content: ""` alongside `tool_calls`), the guard skips `history.assistant` exactly then,
and the `role: "tool"` results that follow dangle without their assistant message. The next
round's request is `[system, user, tool, …]`; OpenAI-compatible servers reject it with a 400
("messages with role 'tool' must be a response to a preceding message with 'tool_calls'"), and the
run dies on the happy path. On Ollama no 400 arrives, but the model sees tool results with no
record of what it asked for. The original r5 M1 scenario is equally unfixed: a malformed call still
produces an orphaned `role: "tool"` "malformed" message. The second side effect also stands: a
pure-thinking final answer (empty content, no calls) leaves the user message unanswered, so the
next prompt starts with two consecutive `user` messages.

What makes the carry painful is that the *history layer is now ready*: `chat_history.rs` gained
`tool_calls_sent_back_always_carry_type_function` (the litellm/vLLM 400 fix) and ordering tests
that assert exactly the `[system, user, assistant, tool]` sequence the agent fails to produce. The
tests pass because they call `history.assistant()` directly — the regression lives one layer up,
in the agent, and nothing connects the two.

**Fix** (unchanged from round 6): the guard must test `!message.tool_calls.is_empty() ||
!message.content.is_empty()`, and the tool-call branch should validate all calls with
`parse_tool_call` *before* anything enters history — echo only the parseable ones (synthesizing an
id when the fragment carried none), report dropped calls via `assistant_note`. Then add the
interaction test (§5, gap 1): feed a stream with a tool call and empty content through `Backend` +
`ChatHistory` and assert the next request body contains the assistant `tool_calls` echo. That
test has been requested for three rounds and would have caught this regression on the day it was
written.

### M2. The README still documents the round-6 M3 footgun as "the defaults" **[code analysis]**

**Location:** `README.md` (Configuration section) vs `src/sven/config.rs::ChatOptions::default`

The code fix for round-6 M3 is real: `ChatOptions::default` is back to `max_tokens: None`,
`num_predict: None`, so a fresh install is safe out of the box again. But the README's config block
still reads:

```json
"options": {
    "temperature": 0.1,
    "num_ctx": 32000,
    "max_tokens": 32000,
    "num_predict": 16000
},
```

— followed by *"Fields missing from the config file fall back to the defaults shown above."* The
README is therefore instructing users that `max_tokens: 32000` is the default — the exact value
round 6 M3 identified as a 400-on-every-request footgun (vLLM validates `prompt + max_tokens`
against the model's context; OpenAI rejects output caps above the model's limit). A user who
copies the documented block into `sven.json` — the most likely action for a new user reading a
configuration section — opts back into the failure the code fix removed. `num_predict: 16000` is
equally stale but harmless.

While syncing, the README's architecture list also needs `sse.rs` — the newest source file, shared
by two subsystems, is missing from it.

**Fix** — update the block to the real defaults (`"max_tokens": null, "num_predict": null`, or
drop the two keys and say "unset by default"), add `sse.rs` to the architecture list, and add the
defaults-pinning test round 6 asked for (§5, gap 3): `partial_config_keeps_defaults_for_missing_fields`
still asserts `num_ctx` only, so the default flipping from `None` to `Some(32000)` and back changed
nothing observable to the suite — twice.

---

## §3 New findings — low

### L1. `MAX_TOOL_OUTPUT` re-tuned to 35 000 — third value in three rounds, still nothing pins or explains it **[code analysis]**

**Location:** `src/sven/agent.rs` — `const MAX_TOOL_OUTPUT: usize = 35_000;`

Round 5 set 10 000, round 6's typo made it 100 000, this round it is 35 000. The new value is a
defensible middle ground — roughly 9–12 K tokens, about a third of the default 32 K window, enough
for most man pages untruncated — but it changed silently, with the doc comment still claiming the
cap exists so a single tool *"cannot flood the model's context window"* (a third of the window per
tool result, stacking across the results of a round, is a meaningful flood), and the unit test
still derives its input from the constant, so it stays green at any value. Three rounds of churn
on one constant with no rationale recorded is the definition of a value nobody can review.

**Fix** — write the rationale into the doc comment (or derive the cap from `num_ctx`), and pin
the magnitude in the test with a literal (`truncate(&long, 35_000)`-style assertions), so the next
silent change at least breaks a test.

### L2. `FromStr` for `Backend` accepts `"Ollama"`/`"OpenAI"` but not `"Vllm"` — and the test name is now false **[code analysis]**

**Location:** `src/sven/backend.rs` — `impl FromStr for Backend`, test `from_str_accepts_the_same_names_as_serde`

The CLI parser matches `"ollama" | "Ollama"`, `"openai" | "OpenAI"`, `"vllm"` — the capitalized
`Vllm` is missing, so `--backend Ollama` works, `--backend Vllm` errors. Meanwhile serde accepts
*only* the lowercase names, so `from_str_accepts_the_same_names_as_serde` now asserts a falsehood:
`FromStr` is a strict superset of serde, and the capitalized aliases it tests are exactly the
ones the enum's doc comment still (wrongly) promises serde keeps. This is the r4 M1 family
aggravated: the doc comment, serde, `FromStr` and the test name now disagree in four different
ways.

**Fix** — fold into the r4 M1 fix: either add the three `#[serde(alias)]` attributes (making the
doc comment true and the test name true), or drop the capitalized `FromStr` arms and the doc
comment's alias sentence together. Either way, add `"Vllm"` or remove `"Ollama"`/`"OpenAI"` so
the CLI is self-consistent, and add the serde-side test.

### L3. Repo-root debris, third round running — now with stale-on-stale content **[code analysis]**

**Location:** repo root

Round 6 named `output.txt` (raw vLLM SSE transcript), `review.html` (renders round 5) and `test`
(captured `cargo build` output). All three are still here, and two have aged into new confusion:

- `test` contains the `unused import: PathBuf` warning — which was fixed this round. The file now
  documents a warning the tree no longer produces, under a name that shadows the conventional
  `tests/` directory.
- `review.html` renders round 5; this file makes it twice superseded.
- `.gitignore` still contains only `/target`, and the working tree is clean — meaning the debris
  was committed, not just left lying.

The round-6 diagnosis stands: the agent's cwd is the repo, scratch output lands in the root, and
nothing sweeps it. Three rounds in a row is a process finding. Delete all three files, widen
`.gitignore`, and consider a pre-commit check for untracked root files.

---

## §4 What's good

- **The shared SSE parser landed** (`src/sven/sse.rs`) — open since round 5, closed cleanly. One
  parser, two interfaces: `SseParser` feeds the chat path line-by-line (tokens print while the
  stream runs) and `parse_sse` digests a buffered body for the MCP transport. The framing is
  spec-correct (one optional space after `data:`, multi-line data joined with `\n`, `event:`/
  `id:`/`retry:`/comments ignored, trailing event flushed), and the tests cover exactly the frames
  the old chat parser dropped: `data:` without a space, payloads split across lines, an event
  whose closing blank line never arrived.
- **The MCP transport rewrite (curl → reqwest) is a net win on every axis.** No subprocess, no
  argv surface to harden; a pooled client; `redirect::Policy::none()` keeps configured headers
  from leaking cross-origin; per-request `.timeout()` plus a manual body deadline in `read_bounded`
  with "keep what arrived before the cut" semantics — a server that answers and then holds its SSE
  stream open is a slow success instead of exit-28 gymnastics. The 404 session-expiry path
  (`SessionExpired`, `pending` cleared, re-initialize, retry once) survived the rewrite intact and
  is still covered by the mock-transport test. The rewrite also moots the old `Expect:
  100-continue` finding (hyper never sends it) and downgrades the negotiated-version header risk
  (reqwest rejects invalid header values with an error instead of curl's potential smuggling).
- **`max_tokens` default reverted to `None`** — the code half of round-6 M3; fresh installs work
  out of the box again (the README half is §2 M2).
- **`pop_user`** — a failed request no longer leaves a dangling user message for the next turn;
  small, correct, tested.
- **`finish_reason: "length"` now prints a visible warning** instead of silently returning a
  truncated answer — with the config field to raise named in the message. Tested. (Gap: Ollama
  never populates `finish_reason`, so an Ollama user hitting `num_predict` still gets a silent
  cut — the warning cannot fire there.)
- **`type: "function"` is defaulted on the history echo**, with the litellm/vLLM 400 rationale
  documented in place, and `stream_options: {"include_usage": true}` plus `reasoning`/
  `reasoning_content` handling round out the OpenAI-compatible path — all tested, including a
  full realistic vLLM stream.
- **The security boundary held again [reproduced]:** `ManPageTool("/etc/passwd")` is rejected with
  the round-3 message; the `as_operand`/`--`/`validate_name` guards are all still in place and
  unit-tested.
- **The build is warning-clean again [reproduced]** — the unused-import warning is gone.
- Test suite: 68 → **73** functions across 13 files (sse.rs's 3, plus new backend coverage for
  the previously-dropped SSE frames, the usage chunk and `finish_reason`).

## §5 Test coverage

73 test functions across 13 files. The gaps, in payoff order:

1. **The tool-call → history → next-request interaction test** — requested in round 5, still
   missing, now three rounds. It would have caught round-6 M1 on the day it landed and would catch
   it today: feed a stream with a tool call and empty content through `Backend` + `ChatHistory`,
   assert the next request body contains the assistant `tool_calls` echo and no orphaned tool
   messages. The chat_history tests cannot catch it — they call `assistant()` directly.
2. **Serde test for `"backend": "Ollama"`** — fourth round running; blocked only on the r4 M1
   aliases existing. The `FromStr` test that exists instead now has a false name (§3 L2).
3. **Config-defaults pinning test** — second round asked; `max_tokens` flipped `None` →
   `Some(32000)` → `None` across two rounds with nothing in the suite noticing either change.
4. **CompileTool still cannot run `cargo test`** (carried L2, reproduced for the fifth round) —
   no review round has ever executed this repo's tests.
5. `notify_finished`'s urgency threshold (>3 min → critical) is still untested.

---

## §6 Feature ideas (carried forward, statuses updated)

Ordered by leverage. "Closes" lists the findings a feature would resolve as a side effect.

### A. Fixes that are features — do these first

1. **Context-window management** *(closes M5, most of L6's pain)* — still the single
   highest-value addition. (a) A budget derived from `num_ctx` that trims the durable history
   (drop oldest tool rounds first, then oldest turns); (b) summarization — when history exceeds
   a threshold, summarize all but the last N messages and rebuild as system + summary + recent.
2. **`Test` variant for CompileTool** *(closes L2)* — `cargo test` / `mvn test` / `dotnet test` /
   `python -m pytest`; add `#[serde(rename_all = "lowercase")]` while touching the enum, and fix
   the description that suggests the lowercase value that fails. Unlocks running this repo's own
   73 tests — five rounds of reviews have never executed them.
3. ~~Token tracking~~ — done (round 5) ✅.
4. **Timeouts everywhere** *(closes M6)* — `reqwest::Client::builder().connect_timeout(..)` (and
   consider `Policy::none()` to match the MCP transport) for the chat client; a configurable
   `tool_timeout` in `subprocess::run` via `tokio::time::timeout`.
5. **Deterministic tool output** *(closes L3, L11)* — pipe man through `col -b`; `LC_ALL=C` on
   every `Command` in `subprocess::run`.
6. **Notification opt-out** *(closes the code half of r5 L1)* — config flag + TTY gate; skip on
   errored runs. The README half is done.
7. ~~Shared SSE parser~~ — **done this round** ✅ (`sse.rs`).
8. **Startup directory creation + per-run history save** *(closes r5 L3)* — one `create_dir_all`
   in `init_skills_dir`, save history after each run.

### B. Python-parity ports (prioritized)

9. **FIFO task queue** (`tasks.json`, `add_task`/`current_task`/`complete_task`/`list_tasks`/
   `cancel_task`) — lets the model decompose big goals and survive restarts.
10. **Cross-session conversation history** (`chat_history.json` + `--resume`) — *partially
    done*: the user's **input** history persists via rustyline; the conversation still does not.
11. **`touch` and `replaceline` tools** — `replaceline(path, line, text)` avoids resending whole
    files; good for small contexts.
12. **Git tools beyond diff** — `git status --short`, `git log -n`, opt-in `git add`/`commit`
    (write actions; consider a config gate, see idea 17). Would also fix GitDiffTool's
    unstaged-only surprise (carried r4 L2).
13. **Per-file compile** — `compilefile(path)`; useful for the edit-one-file-verify loop.
14. **Config profiles** — a `profiles` map selected by `SVEN_PROFILE`.
15. **Ollama options**: `num_predict` done (round 6) ✅; `keep_alive` (default `"10m"`),
    `repeat_penalty` still open *(closes the rest of L10)*.
16. **Mid-stream Ctrl-C** — catch interrupt during streaming, keep the partial answer and
    history, return to the prompt *(also closes the SIGINT-loss half of r5 L3)*.

### C. New directions

17. **Project context file** — auto-load `SVEN.md`/`AGENTS.md` from the cwd into the system
    prompt; pairs with skills.
18. **One-shot mode + exit codes** — `sven -m "prompt"` for scripting, non-zero exit on
    backend/tool failure, everything but the answer to stderr when stdout is not a TTY
    *(closes the stdout-mixing half of L6)*.
19. **Slash commands** — `/help`, `/tools`, `/model <name>`, `/backend <name>`, `/mcp`,
    `/retry`, `/undo`.
20. **Confirmation mode + undo** — y/N before file-modifying tools, optionally with a unified
    diff; and/or automatic checkpoints before writes with `/undo`.
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
    `notifications/tools/list_changed` by re-listing. Note: the reqwest rewrite already
    re-landscaped this area (timeouts, redirects, session recovery are done); what remains is
    the notification/DELETE work.
26. **Skills auto-injection** — config-gated: search skills over the user prompt, prepend the
    top hit's body to the system prompt.
27. **Context-usage indicator** — `[8.2k/32k]` in the prompt prefix; trivial once idea 1's
    accounting exists.
28. **Multi-model routing** — cheap model for tool rounds, strong model for final answers; the
    `Backend` abstraction already separates the concerns.

| # | Idea | Value | Effort | Closes |
|---|------|-------|--------|--------|
| 1 | Context management | ★★★ | M | M5 |
| 2 | CompileTool `Test` | ★★★ | S | L2 |
| 4 | Timeouts | ★★ | S | M6 |
| 5 | `col -b` + `LC_ALL=C` | ★★ | S | L3, L11 |
| 6 | Notification opt-out | ★★ | S | r5 L1 (code half) |
| 8 | Startup dir + history save | ★★ | S | r5 L3 |
| 9 | Task queue | ★★ | M | — |
| 12 | Git status/log/commit | ★★ | S | r4 L2 |
| 17 | Project context file | ★★ | S | — |
| 18 | One-shot mode + exit codes | ★★ | S | L6 |
| 20 | Confirm mode + undo | ★★ | M | — |
| 25 | MCP polish | ★ | S | MCP L2, L5 |

---

## §7 Priorities

1. **Round-6 M1 (2nd round at #1)** — change the guard to
   `!message.tool_calls.is_empty() || !message.content.is_empty()`, implement the
   validate-before-history fix (echo parseable calls, synthesize missing ids, drop via
   `assistant_note`), and add the interaction test. Tool calling is the agent's core feature and
   is broken on OpenAI/vLLM; the history layer it needs is already written and tested.
2. **Round-4 M1 (4th round)** — the serde aliases plus the `FromStr`/doc-comment/test-name
   cleanup (§3 L2). Minutes of work, four rounds old.
3. **§2 M2** — sync the README's options block to the real defaults, add `sse.rs` to the
   architecture list, add the defaults-pinning test. Minutes; until then the docs re-introduce
   the round-6 M3 footgun.
4. **§3 L1** — write down why `MAX_TOOL_OUTPUT` is 35 000 and pin it in the test. Minutes.
5. **Round-3 M3 + L2 (5th round each)** — grep exit 1 as "no matches"; the CompileTool `Test`
   variant. Both small; the latter unlocks executing this repo's own test suite for the first
   time in the review series' history.
6. **Feature 1 (context management)** — the biggest lever for a local-LLM agent.
7. **Debris + hygiene** — delete `output.txt`, `review.html`, `test`; widen `.gitignore`; delete
   (don't comment out) the dead code in L12. Third round running is a process problem, not an
   accident.

Items 2–5 are minutes each; item 1 is an afternoon including the test; the only substantial work
on this list is item 6.