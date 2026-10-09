# Code Review — sven-rs (round 3)

**Date:** 2026-09-30
**Scope:** full workspace — `src/main.rs`, `src/sven/**` (agent, backend, chat_history, config,
macros, security, security_error, skills, term, tool, tool_registry + 13 tool files),
`Cargo.toml`, `Cargo.lock`, `README.md`, `LICENSE`, `.gitignore`, `note.md`. New since round 2:
the OpenAI-compatible backend (`backend.rs`, `Backend` branching in `agent.rs`/`config.rs`,
`tool_call_id` round-trip in `chat_history.rs`) and the 2026-09-30 test-data fixes in
`backend.rs`.
**Method:** read every source file; verified the build (`cargo build` passes via CompileTool,
exit 0); reproduced the findings marked **[reproduced]** by calling the tools themselves
(ManPageTool, GrepTool, FindTool, ListFiles, CompileTool) with crafted arguments. Findings marked
**[code analysis]** are reasoned from the code and documented behavior of the external programs.
The test suite was **not executed** — CompileTool only runs `cargo build` (carried finding L2) —
so the two backend.rs test fixes from 2026-09-30 are compile-verified only.

This is the third review. Round 1 (2 critical, 3 high, 5 medium, 8 low) and round 2's critical
(argument injection into `find`/`ls`) are all fixed; §1 re-verifies them. This round found **one
new critical** — ManPageTool reads arbitrary files outside the workspace — plus two medium issues
in the new backend code, and confirms the carried round-2 mediums are still open.

## Summary

The round-2 critical is genuinely closed: `as_operand()` prefixes `./` onto dash-leading paths,
`ls`/`ddgr` get a `--` marker, `man` rejects leading dashes, and the README now states the
argument-injection caveat. The OpenAI backend added since round 2 is the best-written new code in
the project — fragment merging by `index`, parallel calls, `[DONE]`, mid-stream error objects and
`finish_reason` are all handled and unit-tested, and `tool_call_id` now round-trips so OpenAI can
match results to calls.

But the round-2 review's own warning — *every model-controlled operand must be guarded* — was
applied to `find`, `ls`, `ddgr` and `man`'s option parsing, and missed that **`man` interprets any
argument containing `/` as a file path**. ManPageTool applies no path confinement, so
`ManPageTool(name: "/etc/passwd")` returns `/etc/passwd`. That is the same class of hole as round
1's `file://` bypass: an unconfined read of any file on the system, one prompt injection away.

The two new mediums are in the backend feature: HTTP error responses are never checked (a wrong
model name or API key makes the agent *silently do nothing*), and the config round-trip for
`backend` is broken (`"openai"` fails to parse and silently falls back to Ollama).

---

## §1 Round-2 findings — verification

| # | Round-2 finding | Status |
|---|---|---|
| C1 | argument injection into `find`/`ls` via `path` | ✅ **fixed** — `subprocess::as_operand()` (unit-tested) in FindTool, `--` in ListFiles/WebSearch, leading-`-` rejection in ManPageTool. Verified live: `FindTool(path: "-maxdepth")` → `find: './-maxdepth': No such file or directory` (operand reading forced); `ListFiles(path: "-la")` → `ls: cannot access '-la'` (after `--`). GrepTool was already safe. |
| M1 | grep exit 1 reported as error | ❌ open — reproduced again this round |
| M2 | grep uses BRE, models emit ERE | ❌ open — reproduced again this round |
| M3 | history grows without bound | ❌ open — and aggravated: the round cap was quietly raised 25 → 250 (see M5) |
| M4 | no timeouts except curl | ❌ open — `Client::new()` has none; man/ddgr/cargo/grep/find/ls unbounded |
| L1 | `n: 0` replaces nothing, reports success | ❌ open |
| L2 | CompileTool: case-sensitive enum, build-only | ❌ open |
| L3 | ManPageTool description / roff overstrikes | ❌ open — overstrikes visible in this round's reproductions |
| L4 | full params JSON printed to terminal | ❌ open |
| L5 | FindTool exclusions anchored to `./` | ❌ open |
| L6 | non-interactive mode undocumented; stdout mixing | ❌ open |
| L7 | skills-store writes follow symlinks | ❌ open |
| L8 | `parse_tool_call` rejects string-encoded arguments | ✅ **fixed** — `Some(Value::String(encoded))` branch, empty string → `{}`, tested |
| L9 | small correctness/robustness items | mostly open — see §4 grab-bag |

---

## §2 Critical

### C1. ManPageTool reads arbitrary files outside the workspace **[reproduced]**

> **Status update 2026-10-09 — fixed.** `validate_name()` in `manpage_tool.rs` rejects
> leading `-` and any `/` before the argument reaches `man`; unit tests cover both
> rejection and acceptance, plus an end-to-end test through `Tool::execute` asserting
> the tool (not just the helper) refuses `/etc/passwd`. README security section
> updated. Verified against man-db's actual boundary: only slash-containing
> arguments are opened as files (`README.md`, `.` and `..` are looked up as page
> names), so the check fails closed with no legitimate names excluded.

**Location:** `src/sven/tools/manpage_tool.rs`

`man` interprets any argument containing `/` as a **file path**, not a page name — this is
standard man-db behavior, and it is exactly the mechanism the round-2 fix was built to prevent
for `find`/`ls`: a model-controlled string that the called program reads as something other than
what the tool intended. ManPageTool validates only the leading dash:

```rust
if args.name.starts_with('-') { /* rejected */ }
let mut command = Command::new("man");
command.arg(args.name);          // "src/main.rs" or "/etc/passwd" → man opens the FILE
```

There is no `security::is_inside_cwd` check — the README's confinement list ("every tool that
takes a path") doesn't include ManPageTool because the parameter is called `name`, but for `man`
a name containing `/` *is* a path. Reproduced with two harmless reads:

- `ManPageTool(name: "/etc/passwd")` → returned the full contents of `/etc/passwd` (all local
  accounts, including the `ollama` service user).
- `ManPageTool(name: "src/main.rs")` → returned this workspace's own `main.rs` — a file
  `ReadTool` would have been allowed to read, but via a path that bypasses the confinement check
  entirely.

Both outputs are roff-mangled (see L3) but fully legible. Anything the user can read is
reachable: `~/.ssh/id_rsa`, `~/.config/sven/sven.json` (which now holds `api_key`),
`/proc/self/environ`. The chain is the same one round 1 flagged for `file://`: the model reads
untrusted text (WebFetch pages, file contents, skill bodies) → prompt injection →
`ManPageTool(name: "/home/user/.ssh/id_rsa")` → the secret enters the conversation → and
WebFetch is a GET, so exfiltration is one URL with the secret in the query string away.

**Fix** — fail closed on `/`, mirroring the existing leading-dash rejection (no legitimate page
name contains a slash; this also covers `../` traversal, which always contains `/`):

```rust
if args.name.contains('/') {
    return Err(format!(
        "invalid man page name {}: page names never contain '/'",
        args.name
    ).into());
}
```

Add a regression test (the repo has none for tool argv construction — round 2 already asked for
one), and add ManPageTool's name validation to the README security section.

---

## §3 Medium

### M1. HTTP error responses are silently swallowed — the agent "does nothing" **[code analysis]**

**Location:** `src/sven/agent.rs::run()` / `handle_chunks()`

`builder.send()` returns `Ok` for 4xx/5xx, and the response's status is **never checked**. The
body of an error response is not an SSE/NDJSON stream, so every line of it fails to parse or is
skipped:

- **Ollama:** a wrong model name returns `404 {"error":"model \"x\" not found"}`.
  `process_json_ollama` parses it fine, finds no `message`/`done`/`tool_calls` fields, and
  accumulates nothing. No error is printed.
- **OpenAI:** a bad API key returns `401` with a plain JSON body. `process_line` skips lines
  that don't start with `data: ` — the mid-stream `error`-object handling in
  `process_json_openai` never sees it, because that body isn't an SSE event.

In both cases `handle_chunks` returns an empty `MessageResponse`, `run()` pushes an empty
assistant message into the durable history and returns. The user sees a blank line and is back at
the prompt; the model sees nothing. This compounds with M5: on an OpenAI-compatible server, a
context-length 400 mid-session looks identical to the agent hanging.

**Fix:** check the status before parsing the stream, and surface the body:

```rust
if !response.status().is_success() {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    self.history.pop_user();
    eprintln!("error: {} — {}", status, truncate(&body, 500));
    return;
}
```

Optionally mirror the OpenAI path's `json.get("error")` handling in `process_json_ollama` for
error objects delivered inside a 200 stream.

### M2. `"backend": "openai"` fails to parse and silently falls back to Ollama **[code analysis]**

**Location:** `src/sven/backend.rs`, `src/sven/config.rs`

`Backend` derives `Deserialize` with no serde attributes, so it is externally tagged and expects
`"Ollama"` / `"OpenAI"` — capitalized. But the enum's own `ToString` emits `"ollama"` /
`"openai"`, and `From<String>` accepts `"openai"`. A user who writes the value the program
itself prints gets:

```
Error parsing ~/.config/sven/sven.json: unknown variant `openai`, expected `Ollama` or `OpenAI`
```

…followed by `SvenConfig::default()` — i.e. **the whole config reverts**: backend *and* model,
host, data_dir, api_key. One lowercase word silently turns the OpenAI setup into a default
Ollama setup (the stderr line is the only clue, and in `--end-of-prompt` mode stderr may not be
watched). The README's config example doesn't document `backend` at all, so the capitalized form
is undiscoverable.

**Fix:** make the wire format and the enum agree:

```rust
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Backend { Ollama, OpenAI }
```

`From<String>` then becomes redundant (it is currently dead code — see L12). Add a config test
deserializing `{"backend": "openai"}` — it would have caught this — and document `backend` and
`api_key` in the README's config section.

### M3. grep's "no matches" (exit 1) is reported to the model as an error **[reproduced, carried]**

Unchanged from round 2. Reproduced again: a no-match search returns
`ERROR: grep exited with exit status 1: ` (dangling colon, empty stderr). This round's
methodology note is itself evidence for the finding: one GrepTool call returned exit 1 for a
pattern that matched on three subsequent runs, and there is no way to tell "no matches" from
"bad arguments" from a fluke — which is precisely the ambiguity the model is left in. Fix as
proposed in round 2: `run_allow(&mut command, &[1])` in `subprocess.rs`, used by GrepTool only.

### M4. grep uses BRE, but models emit ERE syntax — silent false negatives **[reproduced, carried]**

Unchanged from round 2. `grep -rni` without `-E` treats `|`, `+`, `(`, `)` as literals;
`GrepTool(pattern: "green|red", path: "src")` still returns nothing although `term.rs` defines
both `fn green` and `fn red`. Add `-E` and say "Extended regular expression" in the param
description — it is the model's only documentation.

### M5. Unbounded history — and the tool-round cap was quietly raised from 25 to 250 **[code analysis, carried + new]**

**Location:** `src/sven/agent.rs:15`, `src/sven/chat_history.rs`

Round 1 fixed the unbounded tool loop with `MAX_TOOL_ROUNDS = 25`; the constant is now **250**,
with no note, comment change, or changelog entry. If intentional, document why; if not, it is a
regression of the round-1 H2 fix. Either way the number is now unreachable in practice: each
round appends an assistant message plus up to 10 000 characters of tool result to
`tool_history`, so with `num_ctx = 32000` (~130 k characters) the context overflows after roughly
a dozen full-size results — long before round 250. On Ollama the prompt is then silently
truncated server-side; on OpenAI-compatible servers the request is rejected with a 400 that M1
currently swallows. Fix: restore a small cap (25 was fine), and/or trim `history` against a
character budget derived from `num_ctx` (drop oldest tool rounds first, then oldest turns).

### M6. No timeouts outside curl **[code analysis, carried]**

Unchanged from round 2: only WebFetch's `--max-time 30` bounds anything. `man`, `ddgr`, `cargo`,
`grep`, `find`, `ls` can hang forever, and `reqwest::Client::new()` has no connect/read timeout,
so a wedged Ollama server hangs the REPL with no message. (Note: the unused tokio `time` feature
in Cargo.toml — see L12 — becomes useful exactly here.)

---

## §4 Low

### L1. `SearchAndReplaceTool` with `n: 0` replaces nothing and reports success *(carried)*

`replacen(old, new, 0)` is a no-op, then "replaced successfully". Validate `n >= 1`.

### L2. `CompileTool`: case-sensitive enum, build-only *(carried)*

`Language` still deserializes `"Rust"` only (`#[serde(rename_all = "lowercase")]` fixes it), and
the tool still runs only `cargo build` — so the agent cannot run this repo's own test suite, and
this review could not either. A `Test` variant would close that loop; until then, run `cargo
test` in CI.

### L3. `ManPageTool`: description and output format *(carried, now with live evidence)*

The description still says "first page" (there is no paging), and the roff overstrikes are
plainly visible in this round's C1 reproductions (`‐` for `-`, mangled spacing, `’0` artifacts).
Pipe through `col -b` so the model gets clean text.

### L4. `process_tool_call` prints the full parameter JSON *(carried)*

Every `ReplaceFileTool` call still dumps its entire `newcontent` to the terminal. Truncate the
printed form (the helper exists).

### L5. `FindTool`'s exclusions only work for the default search *(carried)*

`-not -path "./target/*"` is anchored to `./`; with `path: "sub"` the emitted paths are `sub/…`
and the exclusion never matches. Use `"*/target/*"` / `"*/.git/*"` or `-prune`.

### L6. Non-interactive mode is undocumented; diagnostics share stdout *(carried)*

`--end-of-prompt` and `--prompt` are still absent from the README, and the banner, thinking and
tool-call lines still share stdout with the answer, so a scripted consumer cannot extract the
reply. Route everything but the answer to stderr when `--end-of-prompt` is set.

### L7. Skills-store writes follow pre-existing symlinks *(carried)*

`fs::write` in `add_skill`/`update_skill` follows a planted `SKILL.md` symlink. Hardening, not a
hole: `O_NOFOLLOW` or write-temp-then-rename.

### L8. OpenAI reasoning: only `delta.reasoning` is read **[code analysis]**

`process_json_openai` renders thinking from `choices[0].delta.reasoning`. Common
OpenAI-compatible servers differ: DeepSeek and vLLM's reasoning parser emit
`delta.reasoning_content`; OpenRouter uses `reasoning`. On those servers thinking is silently
dropped (not shown, not kept). Accept both keys. Related: `temperature` is always sent — several
reasoning models (o-series and successors) reject any value other than the default with a 400
that M1 then hides; consider omitting it when it equals the default.

### L9. OpenAI token usage is never surfaced **[code analysis]**

Ollama prints `in N out M` from `eval_count`; the OpenAI path has no equivalent — `usage` is
never requested (`stream_options.include_usage`) nor read. Minor, but the asymmetry means users
switching backends lose the only cost signal the tool has. (If it is added: a usage-only final
chunk carries `"choices": []`, which currently falls into the "end of answering" branch and
prints a stray blank line — harmless today, worth knowing when adding it.)

### L10. `max_tokens` is a no-op on Ollama

`ChatOptions` now has `max_tokens`, which is sent only in the OpenAI body — correct — but a user
setting it while on Ollama gets no output cap at all: Ollama's knob is `num_predict`, which the
config doesn't expose. One sentence in the README's config section prevents the confusion.

### L11. Subprocess diagnostics arrive in the user's locale **[reproduced]**

A bad regex came back as `grep: »(« oder »\(« ohne schließende Klammer` — German, because grep
inherits the environment. The model gets a different error language per machine, and non-English
text is worse signal for small local models. Set `LC_ALL=C` (or `LANG=C`) on the `Command`s in
`subprocess::run` / the tools so errors are deterministic English.

### L12. Hygiene grab-bag

- `impl From<String> for Backend` is dead code (nothing calls it) and, once M2's
  `rename_all` lands, redundant — delete it. The `// API key lives here` comment on the `OpenAI`
  variant is misleading (the key lives in config).
- Commented-out code: `//println!("\x1b[33m {}", &line);` in `backend.rs::process_line`;
  `//eprintln!("no history to load: {}", e);` in `main.rs` (use `let _ =` and drop the comment).
- `impl ToString` should be `impl Display` (clippy), and `endpoint(&self, host: &String)` should
  take `&str`.
- tokio's `time` feature is declared but unused (`#[tokio::main]` needs only `macros` + `rt`);
  drop it — or keep it and land M6, which is its natural consumer.
- README is stale for the backend feature: the config example lacks `backend`/`api_key`/
  `max_tokens`, "talks to any Ollama-compatible server" undersells the OpenAI support, and the
  security section needs ManPageTool's name validation added once C1 is fixed. Also recommend
  `chmod 600` for `sven.json` now that it can hold an API key.
- `note.md`'s testing tip ("point `host` at `{base}/v1`") is wrong against the implementation:
  `endpoint()` appends `/v1/chat/completions`, so `host` must be the bare base — following the
  note yields `…/v1/v1/chat/completions`.
- `ReadTool`: `(n as usize) + offset` can overflow (debug panic) for absurd `num_lines` —
  `saturating_add`.
- `find -name` matches basenames only, so a pattern containing `/` (e.g. `src/*.rs`) silently
  matches nothing; say so in the param description or offer `-path`.
- `skills.rs`/`skill_tools.rs` still cite `skill_spec.md`, which is not in the repo.
- The empty-`thinking`-chunk cosmetic (color left on) now exists in **two** copies
  (Ollama + OpenAI paths) — fix both or factor the chunk-rendering into one helper.
- `print_header` prints `num_ctx` for the OpenAI backend, where it is meaningless.
- `main.rs` doc comment "is recieved" → "is received" — it is a clap doc comment, so the typo is
  user-visible in `--help`.

### Security notes (accepted risks — unchanged, keep documented)

- **TOCTOU** between `is_inside_cwd` and the open; fine for a single-user CLI.
- **CompileTool executes `build.rs`** — arbitrary code by design; say so in the README.
- **WebFetch can reach localhost/private ranges** (including the Ollama server and its own
  config dir via C1's sibling, `~/.config/sven/sven.json`); inherent to a fetch tool.
- **curl ≥ 7.65.2 excludes `file://` from redirect protocols** — `--proto-redir -all,http,https`
  would make it explicit.

---

## §5 What's good

- **The round-2 fix is real and verified live**: `as_operand()` is unit-tested, the `--` markers
  are in place, and the README's security section now states the argv caveat honestly instead of
  overclaiming.
- **`backend.rs` is the strongest new code in the project.** OpenAI's fragmented tool-call
  streaming is the part everyone gets wrong, and it is handled correctly: fragments merged by
  `index` during the stream, completed in `finalize()`, parallel calls kept separate, missing
  `index` treated as a complete call, `[DONE]` and SSE comments skipped, mid-stream `error`
  objects reported, `finish_reason` captured — each with a test.
- **The note.md design was implemented faithfully and completely**: endpoint/body/auth
  branching, `tool_call_id` capture and round-trip (with test), string-encoded arguments
  normalized (round-2 L8, resolved), `max_tokens` mapped per backend.
- **The `length` finish_reason handling** tells the user to raise `max_tokens` instead of
  silently returning a cut-off answer — a small touch most agents lack.
- **Fail-closed patterns are consistent**: leading-dash rejection in man, scheme allowlist in
  WebFetch, empty-path/traversal/symlink denials in security.rs, malformed tool calls become
  tool results instead of panics.
- **Honest docs**: the provenance section stands, and the README's tool table matches the
  17 registered tools.

## §6 Test coverage

Tests exist for the parts most likely to break silently: confinement (security.rs), message
ordering and `tool_call_id` (chat_history.rs), malformed payloads, string arguments and
truncation (agent.rs), config defaults (config.rs), YAML/skills round-trips (skills.rs), and —
new and welcome — seven backend tests covering the OpenAI stream shapes. Gaps, in order of
payoff:

- **No test deserializes a config with `"backend": "openai"`** — would have caught M2.
- **No test covers the HTTP-error path** (M1) — a mock 404/401 response is easy with an
  `#[tokio::test]` and a local listener.
- Still no tests for `subprocess::run`'s exit-code mapping (M3) or tool argv construction
  (C1 — a test asserting ManPageTool rejects `/etc/passwd` belongs in the fix).
- `search_skills` scoring remains untested.
- Standing meta-issue: the agent cannot run its own test suite (L2), so this review — like the
  2026-09-30 test-data fix — is compile-verified only. Run `cargo test` in CI.

## §7 Prioritized action list

1. **C1** — reject `/` in ManPageTool names; add a regression test; update the README security
   section. *Three lines; do first.*
2. **M2** — `#[serde(rename_all = "lowercase")]` on `Backend`; config test; document
   `backend`/`api_key` in the README.
3. **M1** — check `response.status()` before parsing the stream; print status + body; mirror
   error-object handling for Ollama.
4. **M3** — treat grep exit 1 as "no matches" (`run_allow`).
5. **M4** — `grep -E` + document the dialect in the param description.
6. **M5** — restore or justify the 250 round cap; trim history against a `num_ctx` budget.
7. **M6** — timeouts for subprocess tools and a connect timeout on the chat client.
8. **L1–L12** — polish; L1 (`n: 0`), L2 (language casing) and L11 (`LC_ALL=C`) are the ones a
   model will actually hit.

Items 1–5 are a few lines each; none require architectural changes.