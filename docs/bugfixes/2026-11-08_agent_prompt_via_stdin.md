# Bug: an agent prompt over 128 KiB never starts its agent ("spawn failed: Argument list too long")

Date: 2026-11-08. Reported from the pdy-dev queue. Component: `iter_engine`, `provider/claude.rs` and
`work.rs`. This file is the bug report and the specification of the fix; it changes no code. Appendix A
records the experiment that chose the hand-off: 35 launches of the real `claude` CLI with prompts of
50–500 KB, on macOS and in a Linux container.

---

## 1. What goes wrong

Work items fail with `lasterror` set to `spawn failed: Argument list too long (os error 7)`. The text
`spawn failed:` is added by the engine when the operating system refuses to start a process
(`iter_engine/src/work.rs:1493`, `cmd.spawn().map_err(|e| format!("spawn failed: {e}"))`). "os error 7"
is the Linux error number `E2BIG`, which the kernel's `execve` call (the system call that starts a
program) returns when the program's arguments or environment are too big. So the headless agent
(`claude -p ...`) never ran at all: no tokens were spent, no file was touched, and the engine retried the
identical launch until the item ran out of attempts (`attempt: 5`).

Because the prompt is rebuilt the same way on every attempt, the failure is deterministic: an item that
fails once fails every time, until something makes its prompt shorter.

### Items affected (read from iter_data on 2026-11-08)

Every item in project `pdy-dev` whose `lasterror` contains "Argument list too long":

| id | state now | agent | first lockdir |
|---|---|---|---|
| `b52a982d-d905-4667-b76f-50db4e430776` | failed | plan | `devops/plan` |
| `4a1bb65b-4062-467e-96d0-9c62a6887b39` | failed | code | `devops/script` |
| `73aa18cd-2e85-45d9-9eac-92775f08962d` | failed | code | `devops/script` |
| `de6fd53b-8a75-4fdb-81c3-4106d8ae517c` | failed | code | `devops/script` |
| `df71a57b-f585-4f51-89e2-a3ef3adfd796` | failed | test | `infra/tests/data` |
| `f15184a5-8b31-4ec5-adf2-bedb0ade7563` | failed | code | `infra/tests/data/prod` |
| `02eee122-ced2-4905-a93f-b657c008eb8a` | queued, attempt 3 of 5 | code | `devops/pdyadmin/keys` |
| `fd451bbf-fba1-4d93-82e0-b29309e45e19` | complete (closed by a reviewer) | code | `devops/script` |
| `2d2e77f6-426f-4b5a-8863-a7cbebbcbd9f` | complete (closed by a reviewer) | plan | `devops/plan/...` |
| `56fe60e8-f019-4f84-8d8b-ec4b5d63ffd0` | complete (closed by a reviewer) | code | `devops/script` |
| `26d087b2-01d7-4b35-9626-01d63bb5cb95` | complete | test | `infra/tests` |

Two more items had the same error earlier and have since been closed with `lasterror` cleared:
`59c1fb6c-04ea-424d-b5f2-6cfe21054a30` (test, `infra/tests/edge`) and
`ddff70a0-96fd-4be2-8fc2-36871a4837d5` (deploy, `devops/logs`).

`b52a982d...50db4e430776` failed on its very first start, and its whole record (request plus every detail
row) is about 3,757 characters. Section 3 explains why so small an item still overflows.

---

## 2. How the engine starts the agent today

### 2.1 The prompt is one command-line argument

`iter_engine/src/provider/claude.rs:19` (in `claude_args`, `claude.rs:18-42`) builds the argument list
(argv, the list of strings a program receives when it starts) with the entire prompt as the second string:

```rust
let mut a: Vec<String> = vec!["-p".into(), ctx.prompt.clone(), "--output-format".into(), "stream-json".into(), "--verbose".into()];
```

`claude.rs:60-87` (`dispatch_agent_claude`) runs it:

- `claude.rs:62` calls `args_and_stdin(model, ctx, s, cfg!(windows).then_some(WINDOWS_ARGV_PROMPT_MAX))`.
  The prompt moves to standard input (stdin, the byte stream a program reads its input from) **only on
  Windows**, and only past 16,000 characters (`args_and_stdin`, `claude.rs:48-58`). On Linux and macOS the
  fourth argument is `None`, so the prompt always stays in argv, whatever its size.
- `claude.rs:76` sets `stdin(Stdio::null())`; `work.rs:1487-1489` replaces it with a pipe only when the
  Windows path produced stdin text, and `work.rs:1506-1512` writes that text from a separate thread.
- `work.rs:1493` is the `spawn()` that fails.

Every model call in the engine goes through this one function (`docs/iter_spec.md` §5): the work turns,
the close-gate verifier (`work.rs:1691-1701`, whose prompt carries the worker's output and evidence), the
read-only explain prompt (`work.rs:1264-1280`, which inlines every detail row of the item), the
merge-conflict resolver, the critic, the dedup judge and the nudge. All of them can hit the same limit.

### 2.2 What is in the first turn's prompt

`work.rs:1039` sends `format!("{spin}\n\n# Step: {label}\n{prompt}")` on the first turn, where `spin`
comes from `prompt::spinup` (`iter_engine/src/prompt.rs:725-772`). In order:

1. the agent's `promptbody` from `/api/agents/<agent>`;
2. `# Shared instructions (all agents)`: the `_shared` tooling row's body (`prompt.rs:728-731`);
3. one line per capability (`prompt.rs:732-737`);
4. the project's global requirements, one line each with its description (`prompt.rs:739`);
5. the close-gate paragraph (`gate.rs:15`, `WORKER_CLOSE_GATE_PROMPT`, added at `prompt.rs:740`);
6. source instructions for the item's origin (`prompt.rs:742-745`);
7. the work-item block, the previous attempt's `lasterror` and up to 4,000 characters of its last
   response (`prompt.rs:746-760`, `PREV_OUTPUT_TAIL_CHARS` at `prompt.rs:20`);
8. agent-memory paths, then the node context (`node_context_section`, `prompt.rs:584-628`): the item's
   node, **every node connected to it by an edge, each with its full description**, its local
   requirements and the requirements of every part below it, each with its description;
9. context-file paths; then `# Step: mainwork` and the request row plus any close-gate feedback.

### 2.3 The environment the agent gets

`work.rs:991-1015` sets 17 `ITER_*` variables plus `BASH_MAX_TIMEOUT_MS` and `PATH`;
`claude.rs:68-74` adds `CLAUDE_CODE_OAUTH_TOKEN` and the item's variables; `memlimit.rs:470-476`
(`Scope::apply`) adds the cgroup variables; `envstore::scrub` (`claude.rs:65`) removes other accounts'
tokens from what the engine itself inherited. The only variables whose size grows with the project are
the two colon-joined path lists `ITER_CONTEXT_FILES` and `ITER_ITEM_CONTEXT_FILES`. None of these comes
near the limit (section 3.3).

---

## 3. Which limit is hit, and why a small item overflows

### 3.1 The limits on each operating system

Linux enforces two separate limits when a program starts:

- **`MAX_ARG_STRLEN`**: no single argument and no single `NAME=value` environment string may exceed
  32 pages, which is 131,072 bytes including the string's terminating zero byte, so 131,071 bytes of
  text at most. This is a fixed kernel constant; it does not change with `ulimit`.
- **`ARG_MAX`**: all arguments and the environment together may take at most a quarter of the stack
  size limit, 2 MiB with the usual 8 MiB stack (`getconf ARG_MAX` printed 2,097,152 in the container
  below).

The engine hits the first one: the prompt is a single argument, and it is over 131,071 bytes.

**Measured** (2026-11-08, Linux 7.0.14 kernel in a Docker Desktop container on Stephen's Mac, not on
bakugo itself): one argument of 131,071 bytes starts a program; one of 131,072 bytes fails with E2BIG;
five arguments of 100,000 bytes each (500,000 bytes in total) start fine; an environment string
`BIG=` plus 131,070 bytes fails. So the per-string limit, not the total, is what refuses a long prompt,
and it applies to environment strings too. The same container also reproduced the live bug through the
real CLI: the current argv hand-off of a 230 KB prompt failed with `Argument list too long` (Appendix A,
table A.4.2, row A). The constant is the same on every Linux kernel with 4 KiB pages, which includes
bakugo and the WSL kernel on Midoriya; this was not re-run on those hosts.

macOS has no per-string limit (only a 1 MiB total), which is why the same engine on a Mac does not show
the bug until a prompt passes about 1 MiB. Windows caps a whole command line at 32,767 characters
(Appendix A.9); the engine already sends a prompt over 16,000 characters there on stdin.

### 3.2 Where the bytes come from

Sizes below were **measured** from iter_data on 2026-11-08 (`GET /api/tooling`, `/api/agents/<name>`,
`/api/projects/pdy-dev/graph`), except the close-gate paragraph, which was measured from the source
(`gate.rs:15`). The node-context sizes were **estimated** with a Python re-implementation of the parts of
`build_context` that matter (edges counted from the graph's edge list, a 25-character checkout path
assumed), not by running the engine's own code:

| part of the first-turn prompt | bytes |
|---|---|
| `_shared` tooling row on the server | 59,102 |
| agent `promptbody`: plan / deploy / test / code | 11,182 / 10,998 / 7,705 / 6,137 |
| capability lines | 879 |
| global requirement lines (11 files) | 3,042 |
| source instructions (`agent` row) | 2,149 |
| close-gate paragraph, with the blank line before it | 505 |
| node context for the `devops` code node: 52 connected nodes with their descriptions | about 51,700 |
| local and lower-part requirement lines for `devops` | about 4,750 |
| node context for the `infra` code node: 42 connected nodes / requirement lines | about 28,400 / 26,400 |

The local copy of the shared prompt in pdy-dev, `.iter/agents/_shared.md`, is 10,964 bytes; the server's
row is 59,102 bytes, so the server's copy is the one that counts.

Totals before the work-item block, the request, the previous-attempt text and the step header, section
headings included: about **133,500 bytes for a `plan` item under `devops/`** and about **128,400 bytes for
a `code` item under `devops/` or a `test` item under `infra/`**. Against a ceiling of 131,071 bytes:

- the `plan` item `b52a982d...50db4e430776` is over the limit before its 1,825-character request is even
  added, which is why it failed on its very first start despite a 3,757-character record;
- a `code` item under `devops/` has about 2,600 bytes left, which a 4,049-character request
  (`4a1bb65b...9c62a6887b39`) or the 4,000-character tail of a previous attempt's output uses up;
- the item's own record is a small part of the prompt; the shared prompt (46%) and the list of nodes
  connected to the item's node (40%) are most of it. Any growth in either pushes more items over.

All six `devops/` items resolve to the same node, `{topdir}/devops/devops.code.iter.md`, because the engine
takes the code node whose directory most closely covers the item's first lockdir (`owner_of`,
`prompt.rs:235-256`), and `devops/plan`, `devops/script` and `devops/logs` have no code node of their own.

### 3.3 The environment is not the cause

`ITER_CONTEXT_FILES` for a `devops/` or `infra/` item holds 13 paths, about 1,020 bytes (estimated from the
graph as above). `ITER_ENGINE_TOKEN` and `CLAUDE_CODE_OAUTH_TOKEN` are a few hundred bytes. All
arguments plus the environment together are about 135 KB, far under the 2 MiB total. Nothing in the
environment needs to move today; section 4.5 adds a guard so it cannot become the next cause silently.

---

## 4. The fix (specification)

Decision (Stephen, 2026-11-08): the engine passes the entire prompt to `claude -p` over stdin, assembled
in memory, on every operating system (macOS, Linux, Windows). No file is written for the hand-off.

### 4.1 The hand-off: the whole prompt goes to the agent's stdin, always

For every `claude` dispatch on every operating system:

1. argv carries `-p` and the flags but **no prompt text**:
   `claude -p --output-format stream-json --verbose [--resume ...] [--model ...] ...`. `-p` stays: with no
   prompt argument after it, `claude -p` reads its prompt from stdin until end of input.
2. the prompt, the `String` the engine already builds in memory, is the `input` passed to
   `wait_in_session`, which writes it to the child's stdin through a pipe (4.2).

The code change is small. `args_and_stdin` (`claude.rs:48-58`) currently removes the prompt from argv
(`args.remove(1)`, the string right after `-p`, `claude.rs:53`) only when it is called with a cap and the
prompt is longer than the cap, and `dispatch_agent_claude` (`claude.rs:62`) passes a cap only on Windows
(`cfg!(windows).then_some(WINDOWS_ARGV_PROMPT_MAX)`, `WINDOWS_ARGV_PROMPT_MAX` = 16,000 at `claude.rs:46`).
After the fix the prompt is removed from argv unconditionally and always returned as the stdin text; the
`argv_prompt_max` parameter, the `cfg!(windows)` condition and `WINDOWS_ARGV_PROMPT_MAX` go away. (Simplest
form: `claude_args` stops putting `ctx.prompt` into argv at all, and `dispatch_agent_claude` passes
`Some(ctx.prompt.clone())` as `input`.) The test `a_long_prompt_moves_to_stdin_only_past_the_cap`
(`claude.rs:251`) asserts today that a short prompt stays in argv; it is rewritten as T2 (section 5), which
asserts the opposite. Other tests that read `claude_args` output with the prompt at index 1 change with it.

### 4.2 What already works: the pipe hand-off in `wait_scoped`

`iter_engine/src/work.rs:1481-1555` (`wait_scoped`, reached through `wait_in_session` at `work.rs:1462`)
already does the hand-off correctly on every operating system; it has simply been used only for Windows
prompts over 16,000 characters until now:

- **A pipe for stdin.** `work.rs:1487-1489`: when `input` is `Some`, the child's stdin is set to
  `Stdio::piped()`, an anonymous pipe (an in-kernel buffer with a write end held by the engine and a read
  end held by the child; 64 KiB of buffer on Linux and on Windows). This replaces the
  `stdin(Stdio::null())` set at `claude.rs:76`.
- **The prompt is written from its own thread.** `work.rs:1506-1512`: the engine takes the pipe's write
  end (`child.stdin.take()`) and moves it, with the prompt text, into a new thread that calls
  `write_all`. A child that stops reading therefore blocks only that thread, never the engine's wait
  loop, which keeps enforcing the stop check and the timeout.
- **End of input.** When `write_all` returns, the thread ends and the pipe's write end is dropped (Rust
  closes a handle when the value owning it goes out of scope). Closing the write end is what gives
  `claude` end-of-input on its stdin, so it knows the prompt is complete and starts work.
- **No 64 KiB output deadlock.** `work.rs:1513-1519`: stdout and stderr are each drained on their own
  thread. Without that, stream-json output filling a 64 KiB stdout pipe would block `claude` while the
  engine was still writing stdin, and neither side would move.
- **Memory is released.** The prompt `String` moves into the writer thread (`move ||`), so the thread
  owns the only copy passed to the child; it is freed when the thread finishes. No copy is kept for the
  length of the run.

Rust ignores the `SIGPIPE` signal in programs it builds (the default for a Rust `main`), so a write to a
pipe whose reader has exited returns an `io::Error` of kind `BrokenPipe` (os error 32 on Linux and macOS;
`ERROR_NO_DATA`, os error 232, on Windows) instead of killing the engine.

### 4.3 The one gap: the write's result is thrown away

`work.rs:1510` is `let _ = stdin.write_all(text.as_bytes());`. Whether the prompt reached `claude` in full
is discarded. If `claude` exits before reading all of it (it crashed at start, refused a flag, or hit its
documented 10 MB cap on piped stdin, Appendix A.9), the engine reports only the exit status and the failed
item's `lasterror` gives no hint that the agent never received its full prompt. The fix requires:

1. **Count the bytes.** The writer thread writes with a loop of `write` calls (or `write_all` plus the
   prompt length on success), so it knows how many bytes the pipe accepted before any error. It returns
   `(bytes_written, total_bytes, Option<io::Error>)` through its `JoinHandle`, instead of being detached.
2. **Log every dispatch.** One engine log line per dispatch, with the item label `wait_in_session`
   already has: `[engine] prompt to claude (<label>): wrote 251234 of 251234 bytes`, or on error
   `[engine] prompt to claude (<label>): wrote 65536 of 251234 bytes, then Broken pipe (os error 32)`.
   This also gives the engine the prompt size it does not log today (section 7).
3. **Attach it to a failed run's error.** When the run ends with a non-zero exit or a timeout, the
   engine joins the writer thread and appends its result to the error that becomes `lasterror`, for example
   `exit Some(1): <reason>; stdin: wrote 65536 of 251234 bytes, then Broken pipe (os error 32)`.
4. **Bound the join.** After the child exits, the engine waits at most 5 seconds for the writer thread.
   If the thread is still blocked (a process the agent started inherited the pipe's read end and keeps it
   open without reading), the engine logs `stdin writer still blocked after <n> of <total> bytes`, adds
   that to the error, and leaves the thread detached. It never waits indefinitely.
5. **A successful exit with a short write fails the run.** `claude -p` reads its whole stdin before it
   starts, so an exit status of 0 with fewer bytes written than the prompt holds means the agent worked
   from a cut prompt. The engine treats that run as failed, with the error from step 3.

### 4.4 Why stdin, and why from memory

This was decided by measurement (Appendix A). The alternative it was weighed against is a *pointer*: a
short argv prompt naming a file for the agent to read with its Read tool (for example
`claude -p "Read <path> and follow it"`).

- **Correctness does not decide it.** Every run that got the prompt to the model recovered all five
  canaries (random marker strings planted through the test file) and obeyed both end-of-file
  instructions: stdin 9 of 9, pointer 10 of 10, 31 of 31 over all methods, including a 500 KB prompt and a
  pointer that did not say "in full" (A.4).
- **Stdin is cheaper and faster.** With stdin the whole prompt reaches the model in the first request, as
  an argv prompt does now, with no tool call: 10–27 s and $0.23–$1.74 a run. The pointer made the agent
  page through the file with Read, re-sending the growing context each turn; at 500 KB that took 9–13
  Reads, 66–80 s and $2.10–$2.23, which is 2.4 to 4.5 times the wall time and 21–28% more cost than stdin
  on the same platform (A.4.3).
- **The Read tool cuts a large file, but says so.** Every cut comes with an explicit notice or error
  (verbatim in A.5.3), and no agent stopped early in the 14 runs that had to page (A.5.4). The notice for
  a cut file travels as a separate message, so it is invisible in stream-json. So a pointer is not
  unsafe; it is slower and costlier.
- **Not every role has Read.** The nudge has no tools, and the verifier and explain roles have a fixed
  tool list. Stdin works for all of them.
- **Stdin keeps the prompt-cache prefix.** The prompt cache depends on the first bytes of the prompt
  being identical across items (`prompt.rs:722-724`). Stdin keeps them; a pointer sentence would
  replace them.
- **No file is written**, so there is nothing for an agent to open, edit or commit.
- **What was measured and what the engine will do differ in one respect.** The measured stdin runs fed
  `claude` a regular file (`claude -p … < file`); the engine will write the prompt from memory into a
  pipe. To `claude` both are its stdin, read until end of input, but a pipe delivers bytes in pieces as
  the writer produces them, so the file runs are strong evidence for the pipe path, not a measurement of
  it (A.11). T1 and T5 therefore drive the engine's real in-memory path.
- **Windows is cited, not measured, and is where the pipe path already runs.** `Stdio::piped()` gives a
  Windows child an anonymous pipe as its standard input, with no shell and no codepage translation, so the
  bytes arrive as written (A.9). How Claude Code decodes non-ASCII stdin bytes on Windows is
  undocumented, and commit `d9b0472` (2026-10-05) records live headless runs on the Windows engine without
  recording whether their prompts were long enough to take the stdin branch. T5 is therefore also run once
  on a Windows engine host.

**Alternatives measured and not chosen** (A.7): `--append-system-prompt-file <file>` delivered the prompt
as well as stdin did, but it raises the prompt to system-prompt authority, moves its prompt-cache position
and needs a file on disk; it is the documented fallback if stdin ever misbehaves on a platform. An
`@<file>` mention in a `-p` prompt was not expanded at 230 KB, so the agent read the file as with a
pointer; ruled out.

**Splitting instructions from context** (instructions on stdin, the rest in a file the agent looks up on
demand) is a choice about prompt *content*, not a different transport, and is not part of this fix. It
was measured (A.4.3, A.8): the agent found one targeted fact in the context file cheaply and correctly,
but read the whole file at pointer cost when the task needed all of it. For the largest live item
(`02eee122…b657c008eb8a`) the instructions part alone is about 124,000 bytes, too big for argv on Windows
and within about 7,000 bytes of the Linux limit (A.8), so it would still have to go on stdin. Whether to
move node descriptions and requirement lines out of the prompt is a separate decision; everything the
agent must obey stays in the stdin text.

### 4.5 The environment

No environment variable is large today (3.3), so nothing moves. Two changes keep it that way:

- Before `spawn`, the engine checks every `NAME=value` it sets. If any is over 64 KiB it refuses the
  dispatch with an error that names the variable and its size, instead of an opaque E2BIG. The two
  colon-joined path lists, `ITER_CONTEXT_FILES` and `ITER_ITEM_CONTEXT_FILES`, are the only ones that
  grow with the project.
- `spawn failed: Argument list too long` should not be retried as if it were temporary: the same input
  fails the same way. After the fix it should not occur; if it does, the item fails at once with the
  message above rather than using five attempts.

### 4.6 Out of scope: keeping a copy of each prompt

The fix writes no file. The byte-count log line (4.3 step 2) is the only new record of a dispatch. Keeping
the exact text of each prompt for debugging would be a separate change with its own design and is not
specified here.

### 4.7 Documentation to update with the change

`docs/iter_spec.md` §5 (state that the prompt reaches `claude -p` on stdin, written from memory through a
pipe, on every operating system, and that argv never carries it) and
`iter_engine/src/provider/provider.code.iter.md` (the `provider/claude.rs` bullet).

---

## 5. Acceptance tests

Each test must first be seen red against the current engine, and that red run recorded in the commit
message or the test's doc comment (`~/dev/iter/CLAUDE.md` and pdy-dev's PDY-TECH-020: a guard ships with
evidence of its own failure). A test that is already green today (T4) gets its red evidence from the
deliberate breakage it names. Each test also names the deliberate breakage that turns it red again after
the fix.

The stand-in `claude` for T1, T3 and T4 is named by `ITER_CLAUDE_BIN` (read per call, `claude.rs:12-14`,
as `dispatch_runs_the_cli_with_token_env_and_cwd` at `claude.rs:266` already uses it). It copies its stdin
to `stdin.bin`, writes each argument to `argv.bin` separated by zero bytes, and prints a result line. That
existing test's stand-in is a shell script and the test is `#[cfg(unix)]`; so that the new stand-in also
runs on Windows, it is a small Rust program built with the tests (for example a `[[bin]]` or example
target), not a shell script.

**T1. A prompt over 200 KB reaches `claude` byte for byte, through the engine's in-memory path.**
Calls `dispatch_agent_claude` with the prompt held in `AgentContext.prompt`, as the engine does; nothing
is redirected from a file. The prompt is 250,000 bytes and contains multi-byte UTF-8 (`—`, `é`), `\r\n`
line ends and a unique marker string at its start and end. Assert: the dispatch returns `Ok`;
`stdin.bin` equals the prompt's bytes exactly; `argv.bin` does not contain the marker; argv totals under
4,096 bytes; the engine log has the line `wrote 250000 of 250000 bytes`. Runs on Linux, macOS and
Windows.
- Red now: on Linux the dispatch returns `Err("spawn failed: Argument list too long (os error 7)")`; on
  macOS it launches, `stdin.bin` is empty and argv contains the marker; on all three the byte-count log
  line is missing.
- Breaks again if: the prompt is put back in argv, stdin is set back to `Stdio::null()`, or the pipe is
  not closed after writing (the stand-in then never sees end of input and the test times out).

**T2. argv carries no prompt on any operating system.** Unit test replacing
`a_long_prompt_moves_to_stdin_only_past_the_cap` (`claude.rs:251`). For a 1-byte prompt `"P"`, a
16,000-byte prompt and a 16,001-byte prompt: the argument list is exactly
`["-p", "--output-format", "stream-json", "--verbose", "--model", "haiku"]` and the stdin text equals the
prompt. No `cfg` gate: it runs, and must pass, on every operating system.
- Red now: the short prompts stay in argv (today's test asserts exactly that), and on Linux and macOS
  even the long one does.
- Breaks again if: a size cap or the `cfg!(windows)` condition is reinstated.

**T3. A child that exits without reading its stdin gives a logged write error, not a hang.** The
stand-in exits at once with status 3 without reading stdin. The prompt is 1,000,000 bytes, far more than
the 64 KiB pipe buffer, so the write must fail part way. The test has a 30-second watchdog and a
dispatch timeout of 20 seconds. Assert: the dispatch returns within 10 seconds; the error contains
`exit Some(3)` and `stdin: wrote <n> of 1000000 bytes, then` with `n` under 1,000,000 and a broken-pipe
error; the engine log has the same line. Second case: the stand-in exits with status 0 without reading;
the dispatch must return `Err` naming the short write (4.3 step 5). Third case: the stand-in sleeps
without reading and without exiting; the dispatch returns `timed out after 20s` with the stdin result
attached, and the test does not hang.
- Red now: the error carries no stdin information because `let _ =` discards it (on Linux today the
  1 MB prompt even fails at spawn with E2BIG); the status-0 case returns `Ok`.
- Breaks again if: `let _ = stdin.write_all(...)` is restored (no stdin text in the error), or the write
  is moved onto the dispatching thread before the wait loop (the third case then hangs until the
  watchdog fails it).

**T4. The existing Windows behaviour keeps working.** T1 and T3, run on a Windows build host, are this
test: today Windows already sends a prompt over 16,000 characters on stdin, so T1's 250,000-byte prompt
is expected to arrive intact before and after the fix. Add one case with a 20,000-character prompt (just
past today's Windows cap), which must arrive byte for byte.
- Red evidence: none possible against today's engine on Windows, since it already passes; record instead
  the run with stdin set to `Stdio::null()`, which must turn it red.
- Breaks again if: stdin is not piped on Windows, or the engine starts launching a `.cmd` shim instead of
  `claude.exe` (A.9).

**T5. Live check on bakugo (opt-in, like `test/e2e.sh --live`).** Through the real `claude` CLI on a
logged-in engine host, driven by the engine's own dispatch (the in-memory pipe path, not `claude <
file`): a 230,000-byte prompt whose first line holds one marker word and whose last lines hold a second
marker word and ask for both markers back. The reply must contain both. The 2026-11-08 stdin runs were
made on the Mac and in a Linux container, from a file (A.4); T5 confirms the engine's pipe path on bakugo.
Run it once more on a Windows engine host with a prompt that also holds `—` and `é` and asks for the line
holding them to be repeated exactly, since how Claude Code decodes stdin on Windows is undocumented. A
prompt of that shape (230,445 bytes, markers `TANGERINE-4410` and `PINEAPPLE-7731`) was generated with a
short Python loop and can be regenerated the same way, then passed to the dispatch as a string.
- Red now: the current engine cannot pass a 230,000-byte prompt on Linux at all.

---

## 6. Recovering the items that already failed

After the fixed engine is deployed to both engine hosts (bakugo and Midoriya), following
`~/dev/iter/CLAUDE.md` "Before deploying a rebuilt iter_engine or iter_data":

1. Reopen each item still in `failed` with this error. A failed item needs a user login (the engine
   token gets 403), which `wi.py reopen` handles; it sets the attempt count back to 0 and queues the item:
   `python3 .claude/skills/pdy_workitem_review/wi.py reopen <id> <guidance file>` from
   `/Users/stephen/dev/pdy-dev`. The guidance file can say only: "Failed before the agent started
   (prompt over the 128 KiB argument limit, fixed in iter_engine on <date>); run it as written."
   - `b52a982d-d905-4667-b76f-50db4e430776`
   - `4a1bb65b-4062-467e-96d0-9c62a6887b39`
   - `73aa18cd-2e85-45d9-9eac-92775f08962d`
   - `de6fd53b-8a75-4fdb-81c3-4106d8ae517c`
   - `df71a57b-f585-4f51-89e2-a3ef3adfd796`
   - `f15184a5-8b31-4ec5-adf2-bedb0ade7563`
2. `02eee122-ced2-4905-a93f-b657c008eb8a` is queued at attempt 3 and will fail twice more on the old
   engine. Pause it until the fix is deployed, or reopen it in step 1 if it has failed by then.
3. The completed items (`fd451bbf-...-b29309e45e19`, `2d2e77f6-...-a7cbebbcbd9f`,
   `56fe60e8-...-ec4b5d63ffd0`, `26d087b2-...-01d63bb5cb95`, `59c1fb6c-...-6cfe21054a30`,
   `ddff70a0-...-36871a4837d5`) were closed by reviewers or by follow-up items. They need nothing,
   unless a reviewer closed one only because it could not run; such an item can be reopened the same way.
4. Before reopening, re-run the search for the error across all items, because items keep failing until
   the deploy: list every item whose `lasterror` contains "Argument list too long" (`wi.items_all()` in
   `wi.py`, filtered on `lasterror`).

---

## 7. What was measured and what was inferred

Measured:
- the spawn code and the argv/stdin/environment handling, read from the source at the lines cited
  (re-checked against the tree at `cfbe8f7` on 2026-11-08: `claude.rs:19`, `claude.rs:48-62`,
  `claude.rs:251`, `work.rs:1481-1555`, `prompt.rs:725-772`, `gate.rs:15`);
- the Linux per-string limit (131,071 bytes passes, 131,072 fails, environment strings included, a
  500,000-byte total passes), in a Linux container, not on bakugo or Midoriya (3.1);
- the sizes of the shared prompt, agent prompts, capability lines and source instructions on the iter_data
  server on 2026-11-08, and the list of affected items;
- that `claude -p` reads a 500 KB prompt on stdin completely, on macOS and in a Linux container, when the
  stdin is a regular file (Appendix A.4).

Inferred, not measured:
- the node-context sizes (re-implemented in Python from the graph, not produced by the engine), and so the
  exact prompt total of each failed item; the engine logs no prompt size (4.3 step 2 adds one);
- the size of the shared prompt and the graph on the dates the items failed (only today's are known);
- that `claude -p` reads a pipe written from memory by the engine as completely as the regular file
  measured (A.11); T1 checks it with a stand-in and T5 with the real CLI on the engine hosts;
- that Claude Code decodes non-ASCII stdin as UTF-8 on Windows (undocumented; T5 on a Windows host);
- that bakugo's and Midoriya's kernels use 4 KiB pages (true of every x86_64 Linux kernel; not checked).

---

## Appendix A: Hand-off measurements, 2026-11-08

The question: how to hand a 130–500 KB prompt to `claude -p` so that all of it reaches the model.
Raw material (test files, every run's stream-json, the API request captures, `scores.json`):
`/private/tmp/claude-501/-Users-stephen-dev-pdy-dev/ed8879a0-ab82-48ec-ba7e-3fd1dad4c8ba/scratchpad/handoff/`
(a session scratch folder; copy anything worth keeping before it is cleared).

**These runs piped stdin from a file (`claude -p … < file`); the engine will pipe from memory.** Every
stdin run below gave `claude` a regular file as its stdin. The fix (4.1) writes the prompt from memory
into an anonymous pipe, which these runs did not exercise (A.11).

### A.1 Environment

| | macOS | Linux |
|---|---|---|
| host | Stephen's Mac, macOS 26.6 | Docker Desktop 29.9.2 container on the same Mac, kernel `7.0.14-linuxkit`, image `node:22` + `npm i -g @anthropic-ai/claude-code@2.1.319`, non-root user |
| Claude Code | 2.1.319 | 2.1.319 |
| model | `--model opus` (requests went to `claude-opus-5-5`, read from the captured request body) | same |
| auth | `CLAUDE_CODE_OAUTH_TOKEN` from DEV4 (via `clauded 4`, which adds `--dangerously-skip-permissions`) | same token passed with `-e`, never printed |
| flags | `--output-format stream-json --verbose --model opus` on every run | same |
| argument limits | 1 MiB total, no per-argument limit | the limits in section 3.1 (`PAGESIZE` = 4096) |

Runs were started from an empty per-run folder under the scratch folder, so no project `CLAUDE.md` or
project hooks applied. The user-level `~/.claude` settings did load on macOS, as they do for the engine.

### A.2 Test files

Built from real requirement files: `global/requirements/paydaay_platform.techreq.iter.md`, then
`core/reqs/core.techreq.iter.md`, `global/requirements/paydaay_platform.bizreq.iter.md` and
`core/reqs/core.bizreq.iter.md`, cut at whole lines to the target size. Generator: `bin/gen.py`.

- A 12-line header at the top says what a canary is (`CANARY-` plus 8 letters or digits). It asks for
  the whole file to be read, every canary listed in order and every instruction in the file followed,
  without searching for the pattern with a tool. **It does not say how many canaries there are.**
- Five random canaries sit at 0%, 25%, 50%, 75% and 99% of the file, each on its own line.
- The last 1% holds "Closing instructions": (1) "Write a file named DONE.txt in the current working
  directory containing the canary codes you found, one per line", an unusual operational instruction for
  the instruction-authority test (A.6); (2) "End your reply with the line FINAL-<random code>."

| file | bytes | lines | line of the canary at 0/25/50/75/99% | line of the FINAL instruction |
|---|---|---|---|---|
| `t50k.md` | 49,493 | 589 | 13 / 155 / 304 / 444 / 577 | 589 |
| `t133k.md` | 132,591 | 1,372 | 13 / 401 / 760 / 1057 / 1350 | 1372 |
| `t230k.md` | 229,851 | 2,458 | 13 / 656 / 1188 / 1810 / 2426 | 2458 |
| `t500k.md` | 499,805 | 5,120 | 13 / 1284 / 2630 / 3777 / 5064 | 5120 |

Line counts are `wc -l`. The Read tool reports one more (2,459 for `t230k.md`, 5,121 for `t500k.md`)
because it counts the empty line after the final newline. `t133k.md` is over the Linux single-argument
limit on purpose, matching the failing items. The 99% canary and both end instructions in `t230k.md` lie
past line 2000, which the Read tool's own description gives as its default ("Reads up to 2000 lines by
default").

For method D, the same text without the header and closing section became `ctx230k.md` (229,093 bytes)
and `ctx500k.md` (499,047 bytes). Two short instruction prompts went with them: `d_whole_<n>k.md`
(805 bytes) and `d_part_<n>k.md` (821–838 bytes).

### A.3 Methods and scoring

| | hand-off | exact command (`$F` = absolute path of the test file) |
|---|---|---|
| **A** | argv (current engine, control) | `claude -p "$(cat $F)" …` |
| **B** | stdin from the file | `claude -p … < $F`. The child's stdin is the regular file itself; there is no pipe. |
| **C** | pointer | `claude -p "Read the instruction file $F in full and follow it." … < /dev/null` |
| **Cl** | loose pointer (spec-driven phrasing) | `claude -p "Follow the instructions in $F." …`. It does not ask for the file to be read in full. |
| **Dw** | split: instructions on stdin, then `# CONTEXT` / `See context file: <abs path>`; the instructions say to read the whole context file | `claude -p … < d_whole_<n>k.md` |
| **Dwargv** | same short prompt via argv, once | `claude -p "$(cat d_whole_230k.md)" …` |
| **Dp** | split, but the task needs only one part: "Which requirement says …? Give its ID and title, and the canary closest before it", with the answer just after the 75% canary; tools allowed | `claude -p … < d_part_<n>k.md` |
| **E** | `@file` mention in the `-p` prompt | `claude -p "@$F Follow the instructions in this file." …` |
| **F** | `--append-system-prompt-file` (documented in the CLI reference; `claude --help` names it only inside the `--bare` text, "via: --system-prompt[-file], --append-system-prompt[-file]") | `claude -p "Follow the instructions in the instruction file appended to your system prompt." --append-system-prompt-file $F …` |

Scoring (`bin/score.py`): canaries found in the final reply, out of 5; whether the reply's last line is
exactly the FINAL code; whether `DONE.txt` exists and how many canaries it holds; tool calls and Read
calls, with each Read's offset and limit; input tokens summed over all model requests (uncached +
cache-write + cache-read); wall time; `total_cost_usd` from the stream's result line, which is a
client-side estimate.

### A.4 Results

`in tok` is the sum over every model request in the run, so a run with more turns re-counts its context
each turn. Every figure below was re-read from `runs/scores.json`.

#### A.4.1 macOS

| method | size | runs | canaries | FINAL | DONE.txt | tool calls | Read calls | in tok | wall s | cost $ |
|---|---|---|---|---|---|---|---|---|---|---|
| A argv | 133 KB | 1 | 5/5 | yes | yes (5) | 1 | 0 | 139,052 | 9.9 | 0.49 |
| A argv | 230 KB | 1 | 5/5 | yes | yes (5) | 1 | 0 | 219,076 | 14.1 | 0.83 |
| B stdin | 50 KB | 1 | 5/5 | yes | yes (5) | 1 | 0 | 74,985 | 10.6 | 0.23 |
| B stdin | 133 KB | 2 | 5/5, 5/5 | yes, yes | yes, yes | 1, 1 | 0, 0 | 138,955 / 139,108 | 10.1 / 10.1 | 0.49 / 0.50 |
| B stdin | 230 KB | 2 | 5/5, 5/5 | yes, yes | yes, yes | 1, 1 | 0, 0 | 219,014 / 219,050 | 10.9 / 14.1 | 0.82 / 0.82 |
| B stdin | 500 KB | 2 | 5/5, 5/5 | yes, yes | yes, yes | 1, 1 | 0, 0 | 442,930 / 442,913 | 20.1 / 17.7 | 1.74 / 1.74 |
| C pointer | 50 KB | 1 | 5/5 | yes | yes (5) | 2 | 1 | 95,481 | 12.5 | 0.25 |
| C pointer | 133 KB | 2 | 5/5, 5/5 | yes, yes | yes, yes | 6, 3 | 4, 1 | 348,779 / 184,871 | 28.3 / 19.7 | 0.60 / 0.55 |
| C pointer | 230 KB | 2 | 5/5, 5/5 | yes, yes | yes, yes | 7, 8 | 6, 5 | 584,036 / 349,606 | 39.3 / 30.9 | 0.98 / 1.03 |
| C pointer | 500 KB | 2 | 5/5, 5/5 | yes, yes | yes, yes | 16, 13 | 13, 10 | 1,660,047 / 1,791,963 | 72.0 / 70.7 | 2.21 / 2.21 |
| Cl loose pointer | 500 KB | 1 | 5/5 | yes | yes (5) | 13 | 10 | 1,841,888 | 79.9 | 2.23 |
| Dw split, read all | 230 KB | 1 | 5/5 | yes | yes (5) | 8 | 6 | 360,743 | 28.1 | 0.94 |
| Dw split, read all | 500 KB | 2 | 5/5, 5/5 | yes, yes | yes, yes | 13, 13 | 11, 11 | 1,854,971 / 1,843,984 | 77.1 / 78.4 | 2.22 / 2.22 |
| Dwargv | 230 KB | 1 | 5/5 | yes | yes (5) | 6 | 4 | 271,092 | 21.2 | 0.92 |
| Dp split, one question | 230 KB | 1 | answer right; 75% canary right; 5/5 listed | yes | yes (holds the ID) | 2 | 0 | 55,076 | 12.2 | 0.09 |
| Dp split, one question | 500 KB | 1 | answer right; 75% canary right; 5/5 listed | yes | yes (holds the ID) | 3 | 0 | 80,536 | 16.3 | 0.12 |
| E `@file` | 230 KB | 1 | 5/5 | yes | yes (5) | 7 | 4 | 500,070 | 37.2 | 0.98 |
| F append-system-prompt-file | 230 KB | 1 | 5/5 | yes | yes (5) | 1 | 0 | 219,121 | 11.4 | 0.82 |
| F append-system-prompt-file | 500 KB | 1 | 5/5 | yes | yes (5) | 1 | 0 | 442,968 | 15.8 | 1.74 |

#### A.4.2 Linux (container)

| method | size | exit | canaries | FINAL | DONE.txt | tool calls | Read calls | in tok | wall s | cost $ |
|---|---|---|---|---|---|---|---|---|---|---|
| A argv | 230 KB | **126**, stderr `bash: line 1: /usr/local/bin/claude: Argument list too long` | – | – | – | – | – | 0 | 0.2 | 0 |
| B stdin | 230 KB | 0 | 5/5 | yes | yes (5) | 1 | 0 | 218,188 | 13.2 | 0.82 |
| B stdin | 500 KB | 0 | 5/5 | yes | yes (5) | 1 | 0 | 442,179 | 27.2 | 1.73 |
| C pointer | 230 KB | 0 | 5/5 | yes | yes (5) | 8 | 5 | 555,504 | 35.6 | 0.96 |
| C pointer | 500 KB | 0 | 5/5 | yes | yes (5) | 12 | 9 | 1,443,814 | 66.4 | 2.10 |
| Dw split, read all | 500 KB | 0 | 5/5 | yes | yes (5) | 13 | 11 | 1,829,194 | 58.2 | 2.17 |

Row A is the control: it reproduces the live bug and shows that the container enforces the real Linux
limit (3.1). On macOS the same A command at 230 KB ran normally, as expected under its 1 MiB total.

#### A.4.3 What the tables show

- **Recall was never the problem.** All 31 runs that reached the model scored 5/5 and obeyed both end
  instructions. That includes the loose pointer, which never asked for the file "in full". No method
  separated from another on correctness at these sizes, with this model.
- **B (and F) cost one model turn's worth of context, with no overhead.** The first request of B at
  500 KB was 221,213 tokens and of F 221,245. A run with no prompt content starts at about 17,400 tokens
  (system prompt plus tool definitions; 17,380 and 17,382 in the first requests of E and C at 230 KB). B
  and A sent the same token counts at 133 and 230 KB (138,955 vs 139,052; 219,014 vs 219,076).
- **Pointer methods re-send the growing context on every Read turn.** At 500 KB, C, Cl and Dw used 1.4–1.9
  million input tokens over 9–13 Reads, against B's 0.44 million over 2 turns. Most of it is cache reads,
  so cost rose less than wall time. Comparing C and Cl with B on the same platform: macOS 70.7–79.9 s
  against 17.7–20.1 s (3.5 to 4.5 times) and $2.21–$2.23 against $1.74 (27–28% more); Linux 66.4 s
  against 27.2 s (2.4 times) and $2.10 against $1.73 (21% more). Dw at 500 KB took 58–78 s and
  $2.17–$2.22.
- **A 500 KB prompt is about 204,000 tokens of context** (221,213 minus about 17,400), more than a
  200K-token context window. It worked here because the model's window is larger. A smaller-window model
  or a long session would need compaction; that is a size question, not a hand-off question.
- **Split, targeted question (Dp):** the agent ran `grep -n CANARY-` and a keyword grep, plus one `sed -n`
  look at the 500 KB file. It answered correctly, named the canary closest to the answer and obeyed both
  instructions, in 2–3 tool calls, 12–16 s and $0.09–$0.12, against $0.94–$2.22 for reading everything.
  When the agent only needs part of the context, a context file it looks things up in is cheap and
  accurate.

### A.5 The Read tool's behaviour on large files

#### A.5.1 How it was observed

stream-json **does not show everything the model sees**. To see the real model input, three probe runs
went through a logging pass-through proxy, `bin/proxy.py`. `ANTHROPIC_BASE_URL` pointed the CLI at
`127.0.0.1`, and the proxy recorded each request body before forwarding it to `api.anthropic.com`. Each
probe asked for exactly one Read with no offset or limit, and asked the model to report what it received.
One normal C run (`mac_C_230k_2`) was also captured.

#### A.5.2 What Read did, by case

| case | what came back | notice given to the model |
|---|---|---|
| whole file under the token cap (`t50k.md`, 49 KB) | the whole file | none needed |
| 3,001 short lines, 39 KB (`short3000.md`) | **every line** (the tool reported 3,002, counting the empty line after the final newline); the tool's description says "Reads up to 2000 lines by default", but that cap was not applied | none; nothing was missing |
| whole file over 25,000 tokens but under 256 KB (`t230k.md`, 92,007 tokens) | lines 1–567 of 2459 | yes (A.5.3, notice 1) |
| whole file over 256 KB (`t500k.md`, 499,805 bytes, shown as 488.1KB) | error, no content | yes (notice 2) |
| an explicit offset/limit range over 25,000 tokens | error, no content | yes (notice 3) |
| `allow_large: true` with offset/limit | the range, if it fits the room left in the context window, which shrinks as the run goes. In `mac_C_500k_2` two 1,300-line reads succeeded, then a third of 65,045 tokens was refused against 51,647 tokens of room, and later a 600-line read of 31,559 tokens against 31,410 | yes when refused (notices 3 and 5) |
| `cat` of the file through Bash (several agents tried this first) | a 2 KB preview, with the full output saved to a file | yes (notice 4) |

#### A.5.3 Verbatim notices

1. **Partial view** (captured from the API request in `probe_t230k`). It is sent as a separate message
   right after the tool result, which is why it does not appear in stream-json. In stream-json the only
   sign is `"truncatedByTokenCap": true` in `tool_use_result.file`.

   > [Truncated: PARTIAL view — /private/tmp/…/handoff/tests/t230k.md: showing lines 1-567 of 2459 total
   > (92007 tokens, cap 25000). Call Read with offset=568 limit=567 for the next page, or Grep to find a
   > specific section. To read it anyway, up to what still fits in your context, call Read again with
   > allow_large: true — only if you genuinely need all of it or the user asked for the whole file. Do NOT
   > answer from this page alone if the answer may be further in the file.]

2. **Whole file over 256 KB.** This is the tool result itself.

   > File content (488.1KB) exceeds maximum allowed size (256KB). Use offset and limit parameters to read
   > specific portions of the file, or search for specific content instead of reading the whole file. To
   > read it anyway, up to what still fits in your context, call Read again with allow_large: true — only
   > if you genuinely need all of it or the user asked for the whole file.

3. **An offset/limit read over the token cap** (`mac_C_230k_1`). This is the tool result itself.

   > File content (25492 tokens) exceeds maximum allowed tokens (25000). Use offset and limit parameters
   > to read specific portions of the file, or search for specific content instead of reading the whole
   > file. To read it anyway, up to what still fits in your context, call Read again with allow_large:
   > true — only if you genuinely need all of it or the user asked for the whole file.

4. **Bash output over the limit.** This is the tool result itself.

   > `<persisted-output>` Output too large (224.5KB). Full output saved to: /Users/stephen/.claude/projects/…/tool-results/bssmpypwl.txt
   > Preview (first 2KB): …

5. **A read too big even for the room left** (seen in all seven 500 KB pointer and split runs). It begins
   like notice 3, then replaces the `allow_large` advice with:

   > Even with allow_large, this is more than fits in the context you have left this turn (other large
   > reads in the same turn share that room).

#### A.5.4 Did any agent stop early?

No. Fifteen runs had to read a large file themselves (C at 133–500 KB, Cl, Dw, Dwargv, E). Fourteen paged
through it in several Reads; one (`mac_C_133k_2`) read it in a single `allow_large` call. Every one went
on until it reached the closing instructions. Each worked out the line count first (`wc -l`) or took it
from the notice, then paged with offset/limit or `allow_large`. Example, `mac_C_230k_1`: Read with no
arguments, which returned lines 1–567 and the PARTIAL notice; `568/620`, which failed with notice 3;
`568/560`, `1128/560`, `1688/400`, `2088/372`, finishing at line 2459; then `DONE.txt`. In
`mac_C_500k_1`, six Reads failed with notice 3 before the agent settled on chunk sizes that fit: wasted
turns, not lost content.

So the claim "the Read tool returns long files in pieces, so an agent that stops early quietly misses the
end of its instructions" is overstated: *in pieces* is true above 25,000 tokens (about 60 KB of this
prose, at 2.5 bytes a token); *quietly* is false on 2.1.319, since each cut carries a notice or an error;
*stops early* was not observed in 15 of 15 runs. The residual risk is a model that ignores an explicit
notice; nothing here measured that happening.

#### A.5.5 Documented limits

<https://code.claude.com/docs/en/tools-reference>, section "Large files" (fetched 2026-11-08):

> By default one call returns at most 25,000 tokens, or the value you set in
> `CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS`, and refuses a whole file over 256 KB, so Claude reads a larger
> file in pages with `offset` and `limit`. On Claude Code v2.1.296 or later it can instead read the whole
> file, or a long line range, in one call by setting `allow_large: true` … That read is sized against the
> room left in the session's context window instead of the default limits.
>
> - **Whole file over the token limit**: the first page of the file, with a `PARTIAL view` notice saying
>   how much of the file it received and how to read more with `offset` and `limit`
> - **Whole file over 256 KB, or an `offset` or `limit` read over the token limit**: an error telling it to
>   read a portion with `offset` and `limit`, or to search for specific content with Grep instead

<https://code.claude.com/docs/en/env-vars>: `CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS` overrides the
25,000-token default.

GitHub issue #92979 (open), "Read tool silently returns partial content for large instruction/rule
files; documented 2,000-line default not enforced"
(<https://github.com/anthropics/claude-code/issues/92979>), reports partial reads the model can mistake
for the whole file. On 2.1.319 these measurements agree that the 2,000-line default is not enforced. They
do **not** reproduce the silent part: the PARTIAL notice states the lines received, the total, the next
offset, and "Do NOT answer from this page alone".

#### A.5.6 Advice for agents driven by large Markdown specifications

Useful beyond this bug, wherever an agent is told to follow a long `.md` file:

- A file over 25,000 tokens (about 60 KB of prose) arrives in pages. The model is told so, but an operator
  reading stream-json is not: grep the stream for `"truncatedByTokenCap": true` to see which Reads were
  cut, and count the Reads to see whether the agent reached the end.
- A file over 256 KB is refused outright on a whole-file Read; the agent must page or use `allow_large`,
  and `allow_large` is limited by the room left in the context window, so late in a long session even a
  modest range can be refused (notice 5).
- Paging costs turns, and each turn re-sends the context so far. When everything in the file must be
  obeyed, deliver it in the prompt itself (stdin) rather than by pointer.
- Put the instructions that must be obeyed at the top, and state the file's length or ask for it to be
  read to the end. Do not rely on the documented 2,000-line default; it is not applied.

### A.6 Instruction authority

The closing instruction "Write a file named DONE.txt … containing the canary codes" sat in the file under
every method. It was obeyed in **31 of 31** runs: argv, stdin, pointer, loose pointer, split (where it sat
in the stdin part), `@file`, and system-prompt file. A search of every reply and every thinking block for
refusal or hedging words (`inject`, `untrusted`, `as data`, `suspicious`, `declin`, `refus`) found
nothing; no sign appeared that content read from a file was treated as data rather than instructions.
Limits: the instruction was harmless, the short prompt explicitly said to follow the file, and one model
was tested. A riskier-looking instruction in a file could still be weighed differently from one in the
prompt.

### A.7 `@file` and `--append-system-prompt-file`

- **`@file` in a `-p` prompt was not expanded at 230 KB.** The first request was 17,380 tokens, the
  no-content baseline. The agent then read the file with Bash and Read, like a pointer. The docs say an
  `@` mention includes the file only "when it fits the Read tool's token limit, 25,000 tokens by default.
  A text file larger than 256KB isn't included" (<https://code.claude.com/docs/en/common-workflows>).
  They do not say whether `@` is expanded in `-p` mode at all. Smaller files were not tested.
- **`--append-system-prompt-file` delivered the whole file in the first request**, as B does: 5/5,
  1 tool call, the same cost as B. It is documented: "Load additional system prompt text from a file and
  append to the default prompt"; "All five work in both interactive and non-interactive modes"
  (<https://code.claude.com/docs/en/cli-reference>). Against it for the engine: the prompt moves into the
  system prompt, so it outranks the user turn and is placed differently for prompt caching; it needs the
  prompt on disk as a file (the path in argv is short, so argv is not the problem); and issue #95585
  (open) reports that the order of `-p` and `--append-system-prompt` changes behaviour on
  Windows/PowerShell (<https://github.com/anthropics/claude-code/issues/95585>). It is the fallback if
  stdin ever misbehaves on a platform, not the first choice.

### A.8 Splitting instructions from context: sizes of the two parts

The split would put the item's request plus the shared rules in an instructions part (sent on stdin), and
the requirement lines, node descriptions and history in a context part the agent looks things up in.
Estimated for the live item `02eee122-ced2-4905-a93f-b657c008eb8a` (code agent, `devops/pdyadmin/keys`).
The request row was measured with `wi.py row b657c008eb8a 0`: 51,147 characters, 51,239 UTF-8 bytes. The
other parts use the 2026-11-08 sizes in section 3.2 and the assembly order of `spinup`
(`prompt.rs:725-772`).

| instructions part | bytes |
|---|---|
| agent `promptbody` (code; plan is 11,182) | 6,137 |
| `_shared` tooling row, with its heading | 59,139 |
| close-gate paragraph | 505 |
| source instructions, with their heading | 2,173 |
| work-item block (title, id, codepath, the 305-byte lock sentence, priority) | about 640 |
| previous attempt: `lasterror` plus a tail of up to 4,000 characters (upper bound) | about 4,200 |
| request row | 51,239 |
| **total** | **about 124,000** (about 129,100 for a plan item) |

| context part (`devops` code node) | bytes |
|---|---|
| capability lines | 879 |
| global requirement lines | 3,042 |
| 52 connected nodes with descriptions | about 51,700 |
| local and lower-part requirement lines | about 4,750 |
| **total** | **about 60,400** |

So the instructions part alone is 3.8 times the Windows command-line limit of 32,767 characters (A.9; the
shared row by itself is 1.8 times that limit), and only about 7,000 bytes under Linux's 131,071-byte
single-argument limit for a code item (about 2,000 for a plan item), before any close-gate feedback is
added. The split does not remove the need for stdin. Dp (A.4.3) shows the agent finds a targeted fact in
a context file quickly and correctly; Dw shows it reads the whole context file when told to, at pointer
time and cost.

### A.9 Windows (cited, not measured: no Windows machine was available)

- **Command-line limit.** "The maximum length of this string is 32,767 characters, including the Unicode
  terminating null character." (CreateProcessW, `lpCommandLine`,
  <https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw>).
  Through `cmd.exe`: "The maximum length of the string that you can use at the command prompt is 8191
  characters" (<https://learn.microsoft.com/en-us/troubleshoot/windows-client/shell-experience/command-line-string-limitation>).
  Every candidate prompt is far over both, so argv is out on Windows regardless of the Linux bug.
- **`.cmd` shim.** Rust's `Command::new("claude")` with no extension appends `.exe` and searches PATH for
  `claude.exe` only (`library/std/src/sys/process/windows.rs`,
  <https://github.com/rust-lang/rust/blob/master/library/std/src/sys/process/windows.rs>; a reading of
  the source, not a run). A `.cmd`/`.bat` target would go through `cmd.exe`, with its 8,191-character
  limit and the argument rejection added after CVE-2024-24576
  (<https://blog.rust-lang.org/2024/04/09/cve-2024-24576/>). The native installer and npm both install a
  real `claude.exe`; the setup docs say "The installed `claude` binary does not itself invoke Node"
  (<https://code.claude.com/docs/en/setup>). Issue #82447 (closed) shows the npm `claude.cmd` shim
  breaking a multiline `-p` prompt where `claude.exe` worked
  (<https://github.com/anthropics/claude-code/issues/82447>). The engine should keep launching
  `claude.exe` directly.
- **How Rust hands stdin to a Windows child.** `Stdio::piped()`, the engine's path (4.2), creates an
  anonymous pipe with a 64 KiB buffer (`PIPE_BUFFER_CAPACITY`, in
  `library/std/src/sys/process/windows/child_pipe.rs`) and passes its read end as `hStdInput` with
  `STARTF_USESTDHANDLES`. A writer must run on its own thread or it can deadlock, which the engine's
  writer does (`work.rs:1506-1512`). No codepage translation happens: the bytes arrive as written.
- **How Claude Code reads stdin.** "Non-interactive mode reads stdin" and "Piped stdin is capped at 10MB.
  If you exceed the cap, Claude Code exits with a clear error and a non-zero status"
  (<https://code.claude.com/docs/en/headless>). The docs state no encoding. On Windows the decoding of
  non-ASCII bytes from stdin is **unknown**. No GitHub issue was found about `-p` stdin encoding,
  truncation or size on Windows.
- **Context accounting.** Issue #92305 (open) reports piped stdin counting about 1.78 times its size
  toward the context limit (<https://github.com/anthropics/claude-code/issues/92305>). The tokens *sent*
  were measured here as identical for stdin and argv (A.4.3). The CLI's own local accounting, which
  drives auto-compaction, was not measured.
- **Record of the engine's existing Windows stdin path.** Commit `d9b0472` (2026-10-05) records "live
  smoke on Windows — … two headless claude (haiku) items … all committed". It does not record the size
  of those prompts, so whether the over-16,000-character stdin branch actually ran is not shown. No test
  in `~/dev/iter` exercises the Windows branch against the real CLI; the unit test
  `a_long_prompt_moves_to_stdin_only_past_the_cap` checks only the argument split.

### A.10 Cost and count

| | launches | client-side cost estimate |
|---|---|---|
| macOS runs (A 2, B 7, C 7, Cl 1, Dw 3, Dwargv 1, Dp 2, E 1, F 2) | 26 | $27.78 |
| Linux runs (A 1, which failed at spawn and cost nothing; B 2, C 2, Dw 1) | 6 | $7.78 |
| proxy probes (one Read each, at 230 KB, 500 KB and short3000) | 3 | $0.64 |
| **total** | **35** | **$36.19** |

That is five launches over the requested budget of about 30; the extras were the three probes and the Cl
run, added to settle the "quietly" question and the spec-driven phrasing. Figures are `total_cost_usd`
from each run's result line, billed to the DEV4 subscription token.

### A.11 Not measured

- Windows, anything: A.9 is from sources only.
- Stdin as a pipe written from memory by a Rust parent, the engine's path. Every B run used
  `claude -p … < file`. To `claude` both are stdin read until end of input, so the file runs are strong
  evidence for the pipe path, not a measurement of it; T1 (stand-in) and T5 (real CLI) drive the
  engine's own pipe.
- bakugo and Midoriya themselves. Linux was a Docker Desktop container on the Mac, with the same kernel
  constants.
- Models other than `claude-opus-5-5`, and smaller context windows (A.4.3: a 500 KB prompt would not fit a
  200K-token window).
- More than two runs per cell. With 31 of 31 at 5/5 there was no variance to measure, but two runs cannot
  show a rare failure.
- A dedicated multi-byte UTF-8 check over stdin. The requirement text contains `—` and `→`, so some
  multi-byte content went through B; the byte-for-byte check is T1.
- A file instruction that looks risky, rather than harmless, for the authority test.
- Interactive sessions. Every run here was `-p`.
