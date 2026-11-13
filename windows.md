# Running sven-rs on Windows — research findings

Verdict up front: **the crate compiles on Windows almost as-is** (one test-only
import breaks `cargo test`, nothing breaks `cargo build`), but at runtime four
things break the agent out of the box: the home-directory lookup, the path
confinement check (Windows `canonicalize` returns `\\?\`-prefixed verbatim
paths), missing ANSI support in classic consoles, and the eight tools that
shell out to Unix binaries (`ls`, `grep`, `find`, `man`, `ddgr`, `pandoc`,
`notify-send`, …). Everything else — rustyline, tokio, reqwest/TLS, the MCP
client, the chat backends — is cross-platform and works.

Findings below are ordered by severity: §1–§3 are hard blockers, §4 is the
tool-by-tool matrix, §5–§8 are smaller issues, §9 is build/CI, §10 a suggested
order of work.

---

## 1. Blocker: `HOME` is not a thing on Windows

Two places read `$HOME` and have no fallback:

- `src/sven/skills.rs:37` — `expand_tilde()` maps `~` → `$HOME`. Windows sets
  `USERPROFILE` (and sometimes `HOMEDRIVE`+`HOMEPATH`), rarely `HOME`.
- `src/sven/config.rs:99` — `SvenConfig::load(None)` bails out with
  "HOME is not set; using default config" and **never writes a config file**,
  so a Windows user cannot persist `sven.json` at all without `--config-dir`.

Consequences with the default `data_dir = "~/.config/sven"`:
`expand_tilde` returns the literal path, so skills, `statistics.json` and the
rustyline history all land in a directory literally named `~` inside the cwd.
Functional, but wrong.

**Fix (small):** one shared helper, used by both call sites —

```rust
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
```

— or add the `home` crate as a direct dependency (it is already in
`Cargo.lock` as a transitive dep and handles `HOMEDRIVE`/`HOMEPATH` too).
Optionally make the *default* `data_dir` platform-aware
(`%APPDATA%\sven` on Windows, `~/.config/sven` elsewhere) via `cfg!` in
`SvenConfig::default()`; the `dirs` crate gives `config_dir()` for that.

## 2. Blocker: `is_inside_cwd` rejects *every* existing path on Windows

`src/sven/security.rs` is the confinement check all file tools go through, and
it breaks twice on Windows:

1. **Verbatim paths.** `std::fs::canonicalize` on Windows returns
   `\\?\C:\Users\josef\project\src\main.rs`, while `current_dir()` returns the
   non-verbatim `C:\Users\josef\project`. `resolved.starts_with(&cwd)` is then
   false for *every* path whose file exists — so ReadTool, EditTool,
   ReplaceFileTool, GrepTool, FindTool, ListFiles, LoadAttachmentTool and every
   `@path` mention fail with "path … is outside the current directory". This
   alone makes the agent unusable.
2. **Drive prefixes.** `resolve()` starts absolute paths at `PathBuf::from("/")`
   and treats `Component::Prefix(_)` (the `C:` of a Windows path) as
   unauthorized. Safe, but it also rejects absolute paths that *are* inside the
   workspace — which the model passes routinely. On Unix those work.

Also worth noting: `Path::starts_with` compares bytes, but NTFS is
case-insensitive — `C:\users\...` vs `C:\Users\...` must not be a rejection.

**Fix (small but subtle):**

- Canonicalize the cwd once and compare canonical-to-canonical, or strip the
  verbatim prefix from both sides. The [`dunce`](https://crates.io/crates/dunce)
  crate (`dunce::canonicalize`) is the established one-liner for the latter.
- In `resolve()`, seed absolute paths with the prefix instead of rejecting it:
  start with `PathBuf::new()`, push the `Prefix` component, then `RootDir`.
- Compare case-insensitively on Windows (compare component iterators with
  `eq_ignore_ascii_case`, or `#[cfg(windows)]` lowercase both paths).

The existing `Component::Prefix` arm shows the author already thought about
Windows — it just needs to allow the prefix instead of failing closed.

## 3. Blocker: ANSI colors garble in classic consoles

`src/sven/term.rs` prints raw `\x1b[…m` sequences whenever stdout is a
terminal. Windows Terminal renders them fine, but a classic `conhost` console
(cmd.exe, the default in Windows Terminal-less setups, CI, some SSH servers)
does not enable Virtual Terminal processing by default — the user sees
`←[32m` noise around every tool-call header and the thinking stream.

**Fix (tiny):** enable `ENABLE_VIRTUAL_TERMINAL_PROCESSING` on the stdout
handle once at startup, `#[cfg(windows)]` only —

```rust
#[cfg(windows)]
fn enable_ansi() {
    use windows_sys::Win32::System::Console::{
        GetStdHandle, SetConsoleMode, CONSOLE_MODE,
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_OUTPUT_HANDLE,
    };
    unsafe {
        if let Ok(handle) = GetStdHandle(STD_OUTPUT_HANDLE) {
            let mut mode = CONSOLE_MODE::default();
            if GetConsoleMode(handle, &mut mode).as_bool() {
                SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}
```

(`windows-sys` is already a transitive dependency; add it as a direct one with
feature `Win32_System_Console`. The `enable-ansi-support` crate wraps exactly
this if a dependency is preferred over FFI.) `term.rs` itself needs no change —
its `is_terminal()` + `NO_COLOR` gating is already correct.

## 4. The tools: eight external binaries, one already shipped

The tool layer shells out to Unix userland. Status on a clean Windows box:

| Tool (file) | Binary | Windows situation |
| --- | --- | --- |
| `ListFiles` (`list_files.rs`) | `ls -1` | **missing.** `ls` is a PowerShell alias, not an exe; only exists if Git-for-Windows `usr\bin` is on PATH |
| `GrepTool` (`grep_tool.rs`) | GNU `grep -rniE --exclude-dir` | **missing** by default (ships with Git for Windows, off-PATH) |
| `FindTool` (`find_tool.rs`) | GNU `find -name … -not -path` | **missing** by default (same) |
| `ManPageTool` (`manpage_tool.rs`) | `man` | **does not exist** on Windows in any form |
| `WebSearch` (`web_search.rs`) | `ddgr` | not installed, but pip-installable and works on Windows |
| `WebFetch` (`web_fetch.rs`) | `curl` + `pandoc` | `curl.exe` **ships with Windows 10 1803+**; pandoc is an extra install |
| `GitDiffTool` (`git_tools.rs`) | `git` | fine if Git for Windows is installed (near-universal on dev boxes) |
| `NotifySendTool` (`notify_tool.rs`) | `notify-send` | **does not exist**; Windows equivalent is toast notifications |
| `CompileTool`/`TestTool` (`compile_tool.rs`) | `cargo`, `mvn`, `dotnet`, `python` | cargo/mvn/dotnet fine; bare `python` may hit the WindowsApps Store stub |

A missing binary is not a crash — `subprocess::run` surfaces the spawn error as
a tool result — but a model that gets "program not found" for `ls`, `grep` and
`find` cannot do its job.

**Three strategies, in rising effort:**

- **A. Document a prerequisite bundle (zero code).** "Install Git for Windows
  and add `C:\Program Files\Git\usr\bin` to PATH" supplies `ls`, `grep`,
  `find` in one go; pandoc and ddgr are single installs. Cheapest, but
  fragile and surprising.
- **B. Replace the simple ones with pure Rust (recommended).**
  - `ListFiles` → `std::fs::read_dir` — ~10 lines, no dependency, *removes* a
    binary dependency on every platform.
  - `FindTool` → `walkdir` + the `glob` crate's `Pattern::matches` (the tool's
    "Unix shell-style wildcard" contract maps 1:1).
  - `GrepTool` → `walkdir` + `regex` (the tool already promises ERE), keeping
    the `target/`/`.git` exclusions. Alternatively the `grep` crate (ripgrep's
    core) if the `-rni` semantics must be preserved exactly.
  These three are the ones the model calls constantly; making them
  dependency-free fixes Windows *and* slimms the Linux prerequisites.
- **C. `#[cfg(windows)]` variants** calling PowerShell equivalents — possible
  (`Get-ChildItem`, `Select-String`) but quoting through `powershell -Command`
  is a injection-shaped footgun; not recommended when B is easier.

Tool-by-tool notes for the rest:

- **ManPageTool** — no Windows equivalent exists. Either register it only on
  Unix (`#[cfg(unix)]` in `builtin_tools()`), or keep it and let it error
  cleanly. The `validate_name` slash/dash checks are already portable.
- **NotifySendTool** — the agent itself calls this after every run
  (`agent.rs:283` `notify_finished`), so on Windows every run ends with a
  stderr line "could not send notification: …". Harmless but noisy. Replace
  the body with the [`notify-rust`](https://crates.io/crates/notify-rust)
  crate, which does Windows toasts (and macOS) while keeping the Linux
  libnotify path — the argv-building tests would move to the crate's API.
  The embedded-logo temp-file trick (`std::env::temp_dir()`) is already
  portable.
- **WebFetch** — works on Windows 10+ as-is thanks to the bundled `curl.exe`
  (it is real curl; `--silent --show-error -L -f --max-time --` all supported).
  Only pandoc is an extra install. If even that should go: `reqwest` (already a
  dependency) + an html-to-markdown crate (`html2md`/`html2text`) removes both
  binaries — but that is a bigger behavioral change than Windows needs.
- **WebSearch** — `ddgr` installs fine on Windows (`pip install ddgr`); just
  document it.
- **`subprocess::as_operand`** — the `./` prefix trick is a Unix idiom; on
  Windows it is harmless. Moot once find/grep are in-process.

## 5. Tests: two Unix-only spots

- `src/sven/security.rs:77` — `use std::os::unix::fs::symlink;` inside the
  test module. It is `#[cfg(test)]`-only, so `cargo build` succeeds on
  Windows but `cargo test` fails to compile. Gate the module (or the symlink
  cases) with
  `#[cfg(unix)]`; a Windows variant would need
  `std::os::windows::fs::symlink_dir` (and symlink *privileges*, which
  ordinary users may lack — skip the Windows variant or make it best-effort).
- `src/sven/tools/subprocess.rs` tests spawn `sh -c` — gate with
  `#[cfg(unix)]` or use `cmd /C` on Windows.

Everything else in the test suite (fake TCP servers, temp dirs, JSON) is
portable.

## 6. MCP stdio transport: `.cmd` extension resolution

`StdioTransport::spawn` (`src/sven/mcp/transport.rs:100`) runs
`Command::new(command)`. On Windows, `npx`/`uvx` are `npx.cmd`/`uvx.exe`
batch wrappers, and Rust's `Command` does **not** resolve `.cmd`/`.bat`
extensions when searching PATH (only `.exe`). A config entry
`"command": "npx"` therefore fails with "could not start 'npx'". This is the
classic Windows MCP gotcha (VS Code hit it too).

**Fix:** document `"command": "npx.cmd"` for Windows users, or resolve
manually — if `command` has no extension, probe PATH for `command.cmd` /
`command.bat` and spawn that. The rest of the transport (piped stdio, the
reader thread, the Drop-timeout-then-kill) is portable as written; `kill()`
maps to `TerminateProcess`.

## 7. Smaller behavioral differences

- **Case-insensitive filesystem vs `starts_with`** — covered in §2; part of
  the same fix.
- **`python` in CompileTool** — on a clean Windows box `python` resolves to
  the WindowsApps stub that opens the Microsoft Store. Not sven's bug; worth a
  README note ("install Python from python.org and disable the stub").
- **`smoke-config/sven.json`** uses `data_dir: "/tmp/sven-spawn-smoke"` —
  dev-only, but it will not work for Windows smoke tests; make it a CLI arg or
  platform-aware if the smoke test is ever run there.
- **rustyline** — supported on Windows (cmd.exe, PowerShell; *not* Mintty and
  *not* PowerShell ISE). Colors need Windows 10+ (see §3). Ctrl-C →
  `ReadlineError::Interrupted` and Ctrl-Z/Ctrl-D → `Eof` behave the same, so
  `main.rs`'s REPL loop needs no change. History save/load is plain file I/O.
- **`chrono::Local`** — uses OS timezone APIs on Windows; works.
- **`reqwest` TLS** — `Cargo.lock` contains **no `openssl-sys`**: reqwest
  resolves to native-tls → `schannel` on Windows (both are in the lock). So no
  OpenSSL/vcpkg hassle; HTTPS works out of the box on the MSVC target.

## 8. What needs *no* change

Worth stating explicitly, since it is most of the codebase:

- The whole chat stack: `agent.rs`, `backend.rs`, `chat_history.rs`, `sse.rs`,
  `stats.rs` — HTTP + JSON + tokio, no platform surface.
- `tokio` on Windows (IOCP-based runtime, `process`, `time`, `sync` all
  supported).
- The `tool!` macro, `ToolRegistry`, `SpawnAgentTool` (sub-agents are
  in-process), the skills store (plain files; the YAML parser is pure string
  code), `attachment.rs` (extension-based MIME, base64).
- The MCP **HTTP** transport (reqwest-based).
- `read_prompt` non-interactive mode (plain stdin line reading).
- `term.rs`'s gating logic (`IsTerminal`, `NO_COLOR`) — only the console mode
  needs §3's help.

## 9. Building for Windows

- **On Windows:** `cargo build --release` with the `x86_64-pc-windows-msvc`
  target needs the VS C++ Build Tools (rustup prompts for them). No OpenSSL,
  no C libraries — the dependency tree is pure Rust + schannel.
- **Cross-compile from Linux:** `cargo build --target x86_64-pc-windows-gnu`
  works for this crate (no C deps); the `msvc` target needs the Windows SDK
  (e.g. via `cargo-xwin`) — only bother if MSVC-specific tooling is wanted.
- **CI:** add a `windows-latest` job running `cargo build` and
  `cargo test` once §5's gating is in; that catches cfg drift (like the
  `std::os::unix` import) permanently.

## 10. Suggested order of work

1. `home_dir()` with `USERPROFILE` fallback (§1) — unblocks config, skills,
   stats, history. ~15 lines.
2. `security.rs`: verbatim-path + prefix + case handling (§2) — unblocks every
   file tool. The subtle one; needs new unit tests that canonicalize both
   sides.
3. `ListFiles` via `fs::read_dir` (§4B) — removes the most-called missing
   binary. ~15 lines.
4. VT enablement at startup (§3) — ~15 lines behind `#[cfg(windows)]`.
5. `FindTool`/`GrepTool` in Rust via `walkdir`+`glob`/`regex` (§4B) — medium.
6. `NotifySendTool` via `notify-rust` (§4) — medium; kills the per-run stderr
   noise.
7. Gate `ManPageTool` and the Unix tests (§4, §5) — trivial.
8. MCP `.cmd` resolution + README prerequisites section (§6, §4A) — polish.
9. Optional: platform-aware default `data_dir` (`%APPDATA%\sven`), CI job
   (§7, §9).

Items 1–4 are a day of work and produce a Windows build that can hold a
conversation and read/edit files. Items 5–9 bring the tool set to parity.

---

*Research date: 2026-11-13. Verified against the tree at
`/home/josef/Dokumente/sven-rs` (sven-rs 0.4.0), `Cargo.lock` (no openssl-sys;
rustls + schannel present; `home` and `windows-sys` already transitive), the
rustyline README (Windows: cmd.exe/PowerShell supported, colors Win10+), and
Rust std behavior of `fs::canonicalize` on Windows (verbatim `\\?\` paths).*
