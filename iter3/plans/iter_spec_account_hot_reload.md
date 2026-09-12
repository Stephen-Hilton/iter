# Build spec: hot-reload account tokens, and never substitute or fake one

Target repo: `~/dev/iter` (crate `iter3/iter_engine`, with one change each in `iter3/iter_core`
and `iter3/webui`). Written 2026-09-11 from the Mac engine "StephenMBP", project `pdy-dev`.
Every file:line below was read and checked against the tree at commit-of-today.

Terms used here, defined on first use: the **engine** is the `iter_engine` binary that polls
`iter_data` for work items and runs `claude` sessions for them. An **account** is one Claude
login the engine can bill a session to; a project record lists them as
`{name, order, stop, switch, token_envar}` (`iter_core/src/lib.rs:318-329`), where `token_envar`
is the NAME of an environment variable, never the token itself. The **env_file** is the path in
`.iter/config.json` (`env_file`, default `./.env`, `iter_engine/src/main.rs:77-81`) that holds
those variables. A **tick** is one pass of the engine loop (`iter_engine/src/engine.rs:126-176`).

## 1. Problem

Stephen added a fourth account, "Dev4" with `token_envar: DEV4_TOKEN`, to the pdy-dev project
record. The running engine saw the new account within seconds, because a tick re-fetches the
project record whenever its version sequence moves (`engine.rs:434-452`) or on the periodic full
refresh (`engine.rs:413-416`). The engine did **not** see the new token, because tokens come from
the process environment and the process environment is read from the file exactly once, at
startup: `main.rs:121` calls `load_env_file(&cfg.env_file)`, whose body (`main.rs:83-98`) does one
`std::env::set_var` per line and is never called again. Three things then went wrong and none of
them printed an error. First, the webui "test" button ran `run_test` (`engine.rs:181-217`), which
resolved Dev4's token to `None` at `engine.rs:183-190` and passed that `None` straight into
`work::nudge` (`work.rs:779-790`); with no `CLAUDE_CODE_OAUTH_TOKEN` set, the `claude` CLI fell
back to the Mac's ambient interactive login, succeeded, and the result was recorded as
`ok:true` — a false green — while `parse_claude_stream` (`work.rs:605`) handed the ambient
login's `rate_limit_event` line to `usage::record_event` (`usage.rs:192`) under the name "Dev4",
writing that other login's 7-day number into `~/.claude/iter3-usage-Dev4.json`, after which the
ladder believed Dev4 was at 99% and skipped it. Second, if a work item had been dispatched to
Dev4, the token lookup in `spawn_claude` (`work.rs:695-702`) and `spawn_claude_env`
(`work.rs:655-660`) would have run `.chain(project.accounts.iter())` and silently billed the run
to the first OTHER account whose variable happened to be set, while every log line, snapshot and
heartbeat said "Dev4". Third, `probe_stale_accounts` (`engine.rs:223-267`) skips an account with
no token at `engine.rs:234-237` with no `else` branch, so Dev4 was never probed and nothing said
why. The only honest check in the product today is the offline CLI `iter_engine --config <cfg>
--accounts` (`main.rs:346-377`), which prints SET / NOT SET per account at `main.rs:353-358`.

## 2. Goal

Adding, rotating or removing an account token in the engine's env_file takes effect without
restarting the engine; and an account whose token is missing is never silently substituted with
another account's token, never reported healthy, and never billed usage it did not incur.

## 3. Design

### R1 — an in-memory env store, reloaded from the file when the file changes

Add a module `iter_engine/src/envstore.rs` (new code inside an existing crate; no new container)
and declare it in `main.rs:5-12`. It holds one process-wide map behind a lock:

```rust
static STORE: OnceLock<RwLock<Store>> = OnceLock::new();
struct Store { vals: HashMap<String,String>, file_owned: HashSet<String>, stamp: Option<(SystemTime, u64, u64)> }
```

`stamp` is `(mtime, byte length, content hash)` of the env_file as last read.

Public surface: `envstore::init(env_file: &str)`, `envstore::get(key: &str) -> Option<String>`
(trimmed, `None` when empty), and `envstore::reload_if_changed(env_file: &str) -> Option<Changes>`
where `Changes { added: Vec<String>, updated: Vec<String>, removed: Vec<String>, total: usize }`.

Semantics, all mandatory:

1. `init` seeds `vals` from the real process environment (`std::env::vars()`), then overlays the
   env_file. A key present in the process environment at startup wins over the file and is NOT
   recorded in `file_owned` — this preserves today's rule at `main.rs:93` (`std::env::var(k).is_err()`)
   and keeps `e2e.sh:900` working, where `ACCT_A_TOKEN`/`ACCT_B_TOKEN` are exported by the harness
   and never appear in the file.
2. Only `file_owned` keys are ever added, updated or removed by a reload. A key that the startup
   process environment supplied is immutable for the life of the process.
3. Within `file_owned`, refresh only keys whose name ends in `_TOKEN`, plus any name that appears
   as a `token_envar` on some account of some project this engine serves. Pass that set in as a
   `&HashSet<String>` argument from the caller, which builds it from `self.projects`
   (`engine.rs:19`, same iteration as `engine.rs:366-380`).
4. `ITER_ENGINE_TOKEN` (the `token_envar` from `.iter/config.json`, read once at `main.rs:122`) is
   explicitly excluded from refresh. The `Api` client captured it by value at `main.rs:123`
   (`client.rs`), so changing the map would change nothing and would only mislead a reader.
5. A key that disappears from the file is removed from the map (`vals.remove`), so a deleted
   account stops being usable on the next tick.
6. Log exactly one line per reload that changed something, and nothing at all when nothing
   changed: `[engine] env_file reloaded: +DEV4_TOKEN -DEV2_TOKEN ~DEV1_TOKEN (14 keys)`. Values are
   never printed, never logged at any level, and never included in an error message.

**Use the map, not `std::env::set_var`.** The crate is `edition = "2024"`
(`iter_engine/Cargo.toml:4`), where `set_var` is `unsafe`, and the engine is genuinely
multi-threaded: work sessions run on spawned threads (`engine.rs:1108-1112`), as do ELI5 explains
(`engine.rs:301-303`) and dedup triages. Mutating the process environment while those threads read
it is undefined behaviour, not merely untidy. Keep `load_env_file` as the seeding path inside
`envstore::init` but delete its `set_var` call.

Read sites that must change from `std::env::var(...)` to `envstore::get(...)`:

| file:line | what it resolves |
|---|---|
| `engine.rs:188` | the tested account's token in `run_test` |
| `engine.rs:234` | each account's token in `probe_stale_accounts` |
| `work.rs:660` | the session token in `spawn_claude_env` |
| `work.rs:701` | the session token in `spawn_claude` |
| `main.rs:355` | the SET / NOT SET column of `--accounts` |
| `usage.rs` (new, in `accounts_json`, `usage.rs:337-361`) | the per-account `token` flag of R5 |

Read sites that must NOT change: `usage.rs:33` (`ITER_USAGE_DIR`), `usage.rs:206`
(`ITER_USAGE_PROBE_URL`), `engine.rs:91` (`HOME` in `expand_topdir`), `main.rs:122`
(`ITER_ENGINE_TOKEN`), and everything in `cli.rs` (the `iter` shim is a separate short-lived
process that returns at `main.rs:100-102` before any of this runs).

Trigger: call `envstore::reload_if_changed` once per tick, at the top of
`EngineRuntime::tick` (`engine.rs:316`), before the account is picked at `engine.rs:346-356`. The
stat is one `metadata()` call per tick and the file is read only when mtime or length moved.
Additionally force a reload (ignoring the stamp) when the project reload at `engine.rs:446-452`
actually replaced a `Project`, and when `full_refresh` is true (`engine.rs:413-416`) — a stamp can
lie if the clock moves or a file is restored with an old timestamp. `EngineRuntime` gains one
field, `env_file: String`, set from `cfg.env_file` where the runtime is constructed
(`main.rs:154`, `EngineRuntime::new` at `engine.rs:102-124`).

### R2 — a named account with no token is an error, not a fallback

In `work.rs`, replace both copies of the resolution chain with one shared, testable function:

```rust
pub(crate) fn resolve_account_token(project: &Project, account: &str, env_file: &str)
    -> Result<Option<String>, String>
```

Rules: if `account` is empty, return `Ok(None)` (the ambient login; the single-account,
no-accounts-configured case that `dispatch` already handles at `engine.rs:588-591`). If `account`
names an entry in `project.accounts`, return `Ok(Some(tok))` when `envstore::get(&a.token_envar)`
yields a non-empty value, and otherwise
`Err(format!("account '{account}' has no token: {envar} is not set in {env_file}"))` — literally
`account 'Dev4' has no token: DEV4_TOKEN is not set in /Users/stephen.hilton/dev/pdy-dev/.iter/.env`.
If `account` names nothing in `project.accounts`, return the same `Err` shape with
`(no token_envar configured)` in place of the variable name. **Delete the
`.chain(project.accounts.iter())` at `work.rs:659` and `work.rs:699`.** Both `spawn_claude`
(`work.rs:674-709`) and `spawn_claude_env` (`work.rs:637-672`) already return
`Result<String,String>`, so the `Err` propagates to the caller and the item fails loudly through
the existing path.

The item should not be claimed at all, though. In `dispatch` (`engine.rs:563`), compute
`tokenless: Vec<String>` — every account of this project whose `envstore::get(token_envar)` is
empty — alongside the usage map at `engine.rs:592`, and extend `iter_core::pick_account`
(`iter_core/src/lib.rs:889-909`) with a fourth parameter `tokenless: &[String]` whose members are
filtered out in both passes, exactly as `in_use` is at `lib.rs:895-899`. Update both call sites,
`engine.rs:352` and `engine.rs:593`, and the five existing unit calls at `lib.rs:1125, 1133, 1143,
1146, 1150` (pass `&[]`).

`pick_account` returning `None` currently always means "all accounts at stop%"
(`engine.rs:596-606`). Distinguish the new case: when every account of the project is in
`tokenless`, log
`[engine] {project}: no account token is set — {n} accounts configured, none usable` and set
`hold = Some("no account token".into())` instead of `"accounts at stop%"`. `reconcile_waits`
(`engine.rs:919-961`) then writes the tag `blocked by: no account token` onto each queued item
through the existing mechanism at `engine.rs:926` and `engine.rs:955-957`; no new tag plumbing is
needed. Mirror the distinction in the heartbeat: `self.holding` at `engine.rs:381` and the `hold`
string at `engine.rs:385` must say `"no account token"` when the reason is a missing token, not
`"all accounts at stop%"`.

### R3 — the connectivity test tells the truth

Rewrite the head of `run_test` (`engine.rs:181-217`). Resolve the token by the R2 rule across
`self.projects` (the current lookup at `engine.rs:183-190` is correct in scope, only its failure
handling is wrong). Then:

- `account` empty: run the nudge with `None`, and set `"account": "default (ambient CLI login)"`
  in the result JSON at `engine.rs:203` and `engine.rs:208`, so a reader can never mistake an
  ambient-login green for an account's green.
- `account` named and its token present: unchanged behaviour.
- `account` named and its token absent: do **not** call `work::nudge`. Post
  `{"ok": false, "account": <name>, "error": "no token for account 'Dev4' (DEV4_TOKEN unset)"}`
  through the same heartbeat at `engine.rs:212-216`, and print
  `[engine] connectivity test FAILED: no token for account 'Dev4' (DEV4_TOKEN unset)`.

Usage attribution: `work::nudge` passes its `account` argument to `parse_claude_stream`
(`work.rs:789`), which hands it to `usage::record_event` (`work.rs:614`). Because a named account
with no token can no longer reach `nudge`, the ambient login's numbers can no longer be filed
under a real account's name. Make that structural rather than incidental: keep `nudge`'s
signature `nudge(token: Option<String>, account: &str, cwd: &str)` as it is, but assert
inside — `debug_assert!(token.is_some() || account.is_empty())` — and in the `None`-token case
pass `""` (the default snapshot key) to `parse_claude_stream`, never the account name. The same
holds for the heartbeat's `usage` field at `engine.rs:215`.

### R4 — the probe says why it skipped

In `probe_stale_accounts`, the `if let Some(tok)` at `engine.rs:235-237` gains an `else` branch.
Track skipped accounts in a `Vec<(String,String)>` of `(name, envar)` and, for each one that is
`due` by the same rule as a real probe (`engine.rs:241-245`, so the line appears once per stale
period rather than every tick), insert into `self.last_probe` (`engine.rs:40`) and print
`[engine] usage probe 'Dev4' skipped: DEV4_TOKEN not set`.

### R5 — the heartbeat and the webui carry the flag

In `usage::accounts_json` (`usage.rs:337-361`), add one field to the row built at
`usage.rs:344-355`: `"token": if envstore::get(&a.token_envar).is_some() { "set" } else { "unset" }`.
This is additive; `next_json` (`usage.rs:366-374`) is untouched.

In `webui/index.html`, the engine panel builds each account chip at lines 482-486. Extend
`acctTitle` (line 483-484) with `+(a.token==="unset"?" · NO TOKEN in the engine's env_file":"")`,
and the chip text in `acctList` (line 485) so an account with `token==="unset"` renders its name
with a warning marker (e.g. `⚠ Dev4`) instead of a duration. No other webui change.

### R6 — the offline check stays

`iter_engine --config <cfg> --accounts` (`main.rs:346-377`) keeps its behaviour and output. Its
only change is reading through `envstore::get` at `main.rs:355`, so the offline check and the
running engine can never disagree about what is set. `--probe` (`main.rs:360-370`) is unchanged.

## 4. Tests

Every test below must be shown failing before it is shown passing: apply the named mutation, run
the test, record that it went red, revert, run again.

| # | Test (location) | Setup | Assertion | Mutation that must turn it red |
|---|---|---|---|---|
| T1 | `envstore.rs` `#[cfg(test)] reload_picks_up_an_appended_token` | write a temp env_file with `DEV1_TOKEN=a`; `init`; append a line `DEV9_TOKEN=xyz`; call `reload_if_changed` | returns `Changes{added:["DEV9_TOKEN"],..}` and `get("DEV9_TOKEN")==Some("xyz")` | drop the `added` branch of the diff, or make `reload_if_changed` a no-op |
| T2 | `envstore.rs` `reload_removes_a_deleted_token` | continue from T1's file; rewrite it without the `DEV9_TOKEN` line; reload | `Changes.removed == ["DEV9_TOKEN"]` and `get("DEV9_TOKEN")==None` | delete the `vals.remove` call — the stale token survives and the test fails |
| T3 | `envstore.rs` `unchanged_file_is_not_re_read` | init on a temp file; call `reload_if_changed` twice with no edit | second call returns `None`; a read counter incremented inside the reader is still 1 | remove the `stamp` comparison — the file is re-read, the counter reaches 2 |
| T4 | `envstore.rs` `a_length_preserving_edit_is_still_seen` | rewrite the file with the SAME byte length but a different value (`DEV1_TOKEN=a` → `DEV1_TOKEN=b`) | `Changes.updated == ["DEV1_TOKEN"]` | drop the content hash from `stamp`, leaving mtime+length — on a coarse-mtime filesystem the edit is missed |
| T5 | `envstore.rs` `process_env_wins_and_is_never_removed` | seed the store with a simulated process env containing `ACCT_A_TOKEN=proc`; env_file sets `ACCT_A_TOKEN=file` and is later emptied | `get("ACCT_A_TOKEN")==Some("proc")` before and after | drop the `file_owned` set — the file overwrites or deletes the exported value, breaking `e2e.sh:900` |
| T6 | `envstore.rs` `the_reload_line_never_prints_a_value` | reload that adds `DEV9_TOKEN=supersecret`; capture the rendered log line | line contains `+DEV9_TOKEN` and does **not** contain `supersecret` | format the value into the line |
| T7 | `work.rs` `#[cfg(test)] a_named_account_without_a_token_is_an_error` | `Project` with accounts A (`A_TOKEN` seeded) and B (`B_TOKEN` absent) | `resolve_account_token(p,"A",f)` is `Ok(Some("..."))`; `resolve_account_token(p,"B",f)` is `Err` whose text equals `account 'B' has no token: B_TOKEN is not set in <f>` | restore `.chain(project.accounts.iter())` — B resolves to A's token and the test fails |
| T8 | `iter_core/src/lib.rs` `pick_account_skips_a_tokenless_account` | accounts Dev1 (order 1) and Dev2 (order 2), both at 0% usage, `tokenless=["Dev1"]` | picks Dev2; with `tokenless=["Dev1","Dev2"]` returns `None` | ignore the `tokenless` parameter in the filter at `lib.rs:895-899` |
| T9 | `engine.rs` `#[cfg(test)] hold_reason_names_the_missing_token` (extract a pure `fn hold_reason(all_tokenless: bool) -> &'static str`) | — | `hold_reason(true)=="no account token"`, `hold_reason(false)=="accounts at stop%"` | collapse both to `"accounts at stop%"` |
| T10 | `engine.rs` `#[cfg(test)] test_result_for_a_tokenless_account_is_not_ok` (extract `fn test_outcome(token: Result<Option<String>,String>, account: &str) -> Value` from `run_test`) | `Err("…")` for "Dev4" | result `ok == false` and `error == "no token for account 'Dev4' (DEV4_TOKEN unset)"`; an empty account yields `account == "default (ambient CLI login)"` | let the error case fall through to `nudge(None, …)` |
| T11 | `usage.rs` `#[cfg(test)] accounts_json_carries_the_token_flag` | two accounts, one variable seeded | the two rows carry `token:"set"` and `token:"unset"` | drop the field — the `unwrap` on it fails |
| T12 | `e2e.sh`, a new block after the account-rotation section that ends at line 928 | with the engine's env_file at `$SAMPLE/.env`: configure an account `Dev9`/`DEV9_TOKEN` with no value anywhere; run `--ticks 3`; assert the log carries `usage probe 'Dev9' skipped: DEV9_TOKEN not set` and the item carries `blocked by: no account token`; then `echo 'DEV9_TOKEN=tok-dev9' >> "$SAMPLE/.env"`; run `--ticks 3` again; assert `env_file reloaded: +DEV9_TOKEN` in the log, `token=tok-dev9` in `$GATE_PROMPTS/tokens.txt` (the fake claude's recorder, `e2e.sh:591`), and `.accounts[]|select(.name=="Dev9").token == "set"` on the engine record | as above | revert R1 (`init`-only) — the second run still shows no token |

T12 is the guard that would have caught today's incident end to end; T1/T2 are the two the brief
names explicitly.

## 5. Migration and rollout

No change to `iter_data`, to its storage schema, or to any persisted record shape, with one
additive exception: the engine heartbeat's `accounts[]` rows gain a `token` string. `iter_data`
stores that array as opaque JSON, so it needs no change and needs no restart. `webui/index.html`
is served static and picks the field up on reload; an older engine that does not send it renders
exactly as today because the webui reads `a.token` defensively. Both engines need the new binary —
the Mac engine "StephenMBP" and the Linux engine — and each picks it up on its own next restart;
until then each behaves as it does today. `.iter/config.json` is unchanged.

## 6. Out of scope

Rotating `ITER_ENGINE_TOKEN` (the engine's own credential to `iter_data`) while the engine runs:
the `Api` client captures it by value at `main.rs:123` and reconnecting is a separate change.
Re-reading `.iter/config.json` at runtime. The `iter` CLI shim (`cli.rs`), which is a separate
short-lived process. Changing how `claude` itself discovers an ambient login.

## 7. Acceptance

Done by hand on a running engine, with the log in view:

1. With the engine running and the project listing an account "Dev4" (`token_envar: DEV4_TOKEN`)
   that has no value anywhere, confirm the engine logs
   `[engine] usage probe 'Dev4' skipped: DEV4_TOKEN not set`, that the webui engine panel shows
   Dev4 with the no-token marker, and that a queued item carries `blocked by: no account token`
   if Dev4 is the only account.
2. Press "test" in the webui with Dev4 as the engine's account. The result must be red, with
   `no token for account 'Dev4' (DEV4_TOKEN unset)`, and `~/.claude/iter3-usage-Dev4.json` must
   not have been rewritten.
3. Append `DEV4_TOKEN=<the token>` to the env_file. Within one tick the log shows
   `[engine] env_file reloaded: +DEV4_TOKEN (n keys)` with no value printed.
4. The next usage probe for Dev4 runs and logs its 5h/7d numbers; the webui shows Dev4 with
   `token: set`; a work item dispatches on Dev4 and the session's `rate_limit_event` lands in
   Dev4's own snapshot — all with no engine restart.
5. Delete the `DEV4_TOKEN` line from the env_file. Within one tick the log shows
   `[engine] env_file reloaded: -DEV4_TOKEN (n keys)`, Dev4 stops being picked, and the reason is
   logged and tagged rather than silently substituted.
