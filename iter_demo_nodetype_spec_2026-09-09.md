# Specification: make `demo` a first-class node type in the iter engine (`*.demo.iter.md`)

**For:** the agent that maintains the iter engine (separate repository, `~/dev/iter`). **From:**
the pdy-dev side, 2026-09-09. A request, not a change: nothing here was written into that
repository, and nothing in pdy-dev may write there. Claims about *current* engine behaviour were
read at engine HEAD `540f414cf6da` (2026-09-09, `main`) and are cited `file:line`; claims marked
**measured** were run against pdy-dev's vendored binary `devops/iter`, version
`iter 0.1.20260826.1405`.

**One structural warning first.** The node-type machinery exists twice: the V2 binary
(`src/markers.rs`, `src/validate.rs`) and the V3 port (`iter3/iter_local/src/...`, a
near-verbatim copy — `iter3/iter_local/Cargo.toml:5`). A `demo` role added to one and not the
other means V2-side tooling silently reads `*.demo.iter.md` as a plain context doc — every
change below is a change in both.

---

## 1. Why

**What a demo is.** A use-case composed for showing to a person, with a launch script beside it:
somebody types one command, reads a plan of the steps, and watches records move through the
platform. Three exist in pdy-dev under `demos/`; each directory holds a marker file, a
`*.bizreq.iter.md`, a `*.techreq.iter.md`, a `start.sh`, and where something keeps running a
`stop.sh`.

**Why it is not a plain use-case.** A use-case is a journey through code that tests prove. A
demo also has launch scripts a person runs; named parameters with defaults (Demo 02 takes a firm
name, a staff count and an agent count); a run order (Stephen's recipe is Demo 01, Demo 03, Demo
02 three times, Demo 03 again — so a composed demo is itself a demo); and its own business and
technical requirements, saying what the demonstration must never do and what is not built yet.

**What breaks today, measured.**

1. **The file is named `<stem>.demo.usecase.iter.md` as a workaround.** `role_of` takes the
   segment after the LAST dot (`src/markers.rs:32`) and matches it exactly against seven arms
   (`src/markers.rs:33-41`), so `demo` in the middle is decoration the engine never sees. A file
   named `*.demo.iter.md` matches nothing: measured,
   `devops/iter validate --template --file /tmp/x.demo.iter.md` exits **2** with "matches no
   nodetype ... the dot rule" — the text at `src/validate.rs:592-595`.
2. **A use-case may not own requirements.** `children_keys(Role::Usecase)` is exactly
   `["codenodes", "testgroups"]` (`src/validate.rs:68`), so each demo's bizreq and techreq hang
   off a context node instead: `demos/demos.code.iter.md` declares
   `bizreqs: ["{thisfiledir}/*/*.bizreq.iter.md"]` and the techreq twin. Measured from
   `devops/iter markers`: the node `demos` carries all six requirement files and the three demo
   files carry none — a reader of one demo cannot reach its own requirements from it.
3. **`testgroups: []` has to be declared defensively.** The use-case default is
   `{thisfiledir}/{thisfilestem}/*.testgroup.iter.md` (`src/markers.rs:620-622`), and a use-case
   whose testgroups resolve to nothing is a coverage gap that births a testwriter item
   (`src/testsweep.rs:254-276`). All three demo files declare an empty list and each spends a
   paragraph on why — a demo's proof is that `start.sh` runs, not a registry.
4. **Nothing ties the launch script to the node.** `UseCase` carries `codenodes` and
   `testgroups` only (`src/markers.rs:330-347`), so `start.sh`, its parameters and the run order
   exist in prose alone.

## 2. The filename rule

`<stem>.demo.iter.md`, under the same dot rule as every other role: the role tag is the segment
after the last dot, lowercase, case-sensitive (`src/markers.rs:28-44`). A bare `demo.iter.md`
must work too — every `*.x` glob is also tried in its bare form (`src/placeholders.rs:157-167`).
Add `Demo` to `Role` (`src/markers.rs:17-26`), arms to `role_of` and `role_name`
(`src/markers.rs:60-71`), the string to the validator's two hardcoded role lists
(`src/validate.rs:130-135`, `:592-595`), `demo` to `V1_STYLE_TAILS` (`src/validate.rs:94`), and
`Role::Demo` to the `teststate`/`owner` vocabulary gate (`src/validate.rs:241`).
`iter validate --template --file x.demo.iter.md` emits a template: a `DEMO_TEMPLATE` beside
`USECASE_TEMPLATE` (`src/validate.rs:707-722`) and an arm in `template_for`
(`src/validate.rs:589-613`). Accepted child keys — the new row in `children_keys`
(`src/validate.rs:61-71`):

| Key | Default | What it holds |
|---|---|---|
| `codenodes` | `[]` | the `*.code.iter.md` containers the demo exercises |
| `bizreqs` | `{thisfiledir}/*.bizreq.iter.md` | same default a code node uses (`src/markers.rs:525-527`) |
| `techreqs` | `{thisfiledir}/*.techreq.iter.md` | as `src/markers.rs:528-530` |
| `testgroups` | `[]` (empty) | a demo's proof is its launch script, not a registry |
| `launch` | `{thisfiledir}/start.sh` | the start/stop/reset scripts |
| `sequence` | `[]` | other `*.demo.iter.md` files, in order — a composed demo is a demo file |
| `prerequisites` | `[]` | demo files that must have run before this one |

Every key in that table is a **file** key and resolves through `resolve_child_files`
(`src/markers.rs:392-424`) unchanged — it globs each entry and silently drops what does not
exist, so entries pointing outside the demo's own directory want the `{topdir}` idiom.

`parameters` is **not** a file key and must not go through that path: it is a frontmatter list
of name/default/description triples, e.g. `- name: "HOUSE_NAME"`, `default: "Banksy Investing"`,
`description: "the capital-provider firm to create"`. The `description` values are prose and
must be quoted for the reason `PROSE_KEYS` already gives (`src/validate.rs:57-59`): an unquoted
colon-space reads as a nested key and a strict YAML reader then refuses the whole block.

## 3. Engine behaviour asked for

**Demos are global root children, like use-cases** — scanned from every scandir, never required
under one directory. Discovery is free: `ITERGLOB` is `**/*.iter.md` (`src/project.rs:34`),
applied per scandir (`src/markers.rs:465-470`). Add the `Some(Role::Demo)` classification arm
beside `src/markers.rs:568` and build them where the comment already says such things live:
"interfaces & use-cases: global objects, always root children" (`src/markers.rs:575`) — no
`key`, no `parent`, never in the DAG attach walk.

**`iter markers` lists them under `demos`, with codenode keys resolved.** `Scan` derives
`Serialize` (`src/markers.rs:357-366`) and `cmd_markers` pretty-prints the whole struct
(`src/main.rs:1477-1490`), so a new `demos: Vec<Demo>` field appears in the JSON with no printer
change. Resolve `codenode_keys` as use-cases get theirs (`src/markers.rs:829-838`), and have
each demo claim its own files against the Orphanage the way use-cases do
(`src/markers.rs:869-874`) — `start.sh` included, so a launch script is never called an orphan.

**The structure map shows a demo as a use-case-shaped node with its launch scripts:** model it
on `ucRows` and its heading (`src/webapp/app.html:3923-3952`, `:4000-4002`), with the launch
scripts, the parameters and the last run in the expanded row.

**A `demo` agent type.** In V2 an agent type is a file — `.iter/agents/<type>.md`, discovered by
stem (`src/agents.rs:41-75`), shipped in the embedded template list (`src/template.rs:15`); in
V3 it is a database row read at dispatch (`iter3/iter_engine/src/cli.rs:830-832`, prompt
assembly `iter3/iter_engine/src/prompt.rs:1-6`). Its prompt is the usecase agent's plus two
sentences: **it may write only launch scripts and marker files under the demo's own directory,
and everything else it needs is a work item it files.** Give it `default_codepath: "{demo_dir}"`
mirroring `{usecase_dir}` (`src/agents.rs:213-215`, resolved at `src/server.rs:1305`), export
`ITER_DEMO_DIR` beside `ITER_USECASE_DIR` (`src/scheduler.rs:1407`;
`iter3/iter_engine/src/work.rs:489`), and add a `demo_dir` field to `Head`
(`iter3/iter_engine/src/prompt.rs:51-57`).

**`demo:<name>` as an engine-owned inherited tag.** Copy the usecase mechanism exactly:
`USECASE_TAG_PREFIX` (`iter3/iter_core/src/lib.rs:208-212`), the field-to-tag conversion and
birth inheritance in `place_new_item` (`iter3/iter_data/src/api.rs:752-762`, `:768-774`), and
the CLI flag (`iter3/iter_engine/src/cli.rs:68-71`) as `iter add --demo <name>`. A priority
sweep can then find every item in support of a demo. **One improvement over the usecase
version:** nothing today checks that a `usecase:<name>` tag names a real file. For `demo:`,
validate that a `*.demo.iter.md` of that name exists and refuse the item if not — a typo'd tag
is an item nobody will ever find.

**`iter demo --run <file> [--dry-run]`.** Executes the launch script with its parameters and
records on the demo node: exit code, log path, wall-clock duration, and the parameter values
used. `--dry-run` prints the command line and writes nothing, the way `migratev2 --dry-run`
gates every side effect (`src/main.rs:254-264`). **This is new state**: the only per-node run
state today is a testgroup's `lastrun`/`result`/`counts` in an HTML-comment JSONL block in the
file body (`src/testgroups.rs:54-68`, written at `src/runtests.rs:211-218`), and that block
records no exit code, log path or duration — the exit code lives only in memory
(`src/runtests.rs:196-200`), the log path only in a naming convention (`src/runtests.rs:135`),
elapsed time nowhere. Use the same JSONL-block idiom, extended. **`--sequence`** runs a composed
demo in order and stops at the first failure, naming the entry that failed and its exit code.

**`iter demo --status`** prints, per demo: the last run, and one line per "What is missing" row
saying whether that row's work item is open or closed. The join key is the techreq table's
work-item column. **The column shape the engine parses**, so nothing has to guess: a
GitHub-style pipe table whose header row's last cell is exactly `Work item`; each data cell
holds either one or more 12-character lowercase hex ids separated by commas (optionally followed
by prose) or no id at all. A cell with no id means **no item is filed** and is reported that
way, never as closed. Both forms are live in
`demos/03_stream_employee_wages/03_stream_employee_wages.techreq.iter.md`, section 4.

## 4. Migration

**`iter migrate --demos [--project P] [--dry-run]` — one command.** There is no `iter migrate`
today: the V2 subcommand list has `Migratev2` and no `Migrate` (`src/main.rs:258`), and V3's
verb list has none either (`iter3/iter_engine/src/cli.rs:360-367`). Copy `migratev2`'s shape
(`src/main.rs:254-264`) and its `Mig` recorder, which prints every change and renames rather
than deletes (`src/migrate.rs:25-61`); its dot-rule renamer (`src/migrate.rs:515-527`) is the
code to clone. It must do all of this, or it is not one command:

1. rename each `*.demo.usecase.iter.md` to `*.demo.iter.md`;
2. move the requirement globs **off** the context node — delete `children.bizreqs` and
   `children.techreqs` from `demos/demos.code.iter.md` and write each demo's own
   `bizreqs`/`techreqs` (the defaults in §2 make this an empty write in the common case);
3. add `launch: ["{thisfiledir}/start.sh"]` where such a file exists, plus `stop.sh`/`reset.sh`;
4. leave `testgroups: []` declared, and say in the report that it did.

The validator warns on the old name: a `*.demo.usecase.iter.md` gets a `Warn` saying it reads as
a demo but the engine sees a use-case, naming `iter migrate --demos` as the fix.

## 5. Considerations the engine should get right

**An empty `testgroups` on a demo must not count as untested.** The sweep treats the existing
kinds oppositely: for a code node, absence is a choice, counted `undeclared`
(`src/testsweep.rs:231-237`); for a use-case or interface it is a coverage gap that births an
authoring item unless the empty list was **declared** (`src/testsweep.rs:254-276`). Demos take
the code-node side: empty `testgroups`, declared or defaulted, counts `undeclared` and births
nothing. (V3 has no sweep — `iter3/iter_engine/src/cli.rs:360-367` — so this binds the V2 sweep
and whatever replaces it.)

**Locks: a demo item locks only its own directory.** Codepaths are normalised to
`{topdir}`-relative lockdirs (`iter3/iter_engine/src/cli.rs:272-292`) and overlap is
prefix-with-slash (`iter3/iter_core/src/lib.rs:745-753`). Give the `demo` agent a `LockShape`
(`iter3/iter_core/src/lib.rs:70-99`) whose `allow` is the demos directory and whose `outside` is
`refuse` — absent, it inherits "anything goes" (`iter3/iter_core/src/lib.rs:157`).

**Launch-script exit codes: the convention, and the scripts do not follow it yet.** Proposed:
**0** ran to the end; **1** a step's program refused (a real, reported outcome); **2** a step's
program is unbuilt or not invocable; **3** the run refused to start (bad parameter, wrong
environment). Measured 2026-09-09, the three pdy-dev scripts differ:

- a failed step exits with whatever the step's program returned — `exit "$step_rc"` in
  `demos/01_setup_new_employer/start.sh:158` — so "unbuilt" is indistinguishable from "refused".
  Measured: a missing Python module exits 1, an unknown subcommand exits 2 (argparse).
- the scripts themselves use 2 for a bad parameter and 3 for a non-`dev` environment
  (`demos/01_setup_new_employer/start.sh:69,97`), and Demo 03 uses 4 when its rate arithmetic
  fails (`demos/03_stream_employee_wages/start.sh:258`).
- **the environment refusal is inert.** `PDY_ENV=prod demos/01_setup_new_employer/start.sh` did
  **not** refuse: the script sources the repository `.env` with `set -a` (`start.sh:83-87`),
  overwriting `PDY_ENV` with `dev` before the check at `:93`; the run went on and exited
  non-zero at step 1 because `pdyadmin env status` is red on this cluster today. That is a
  pdy-dev defect, named here for the pdy-dev side to file. Until the scripts adopt it the engine
  must not infer "unbuilt" from an exit code, and `iter migrate --demos` must not pretend they
  have.

**Never run a demo outside `dev` unless the demo file says otherwise.** The engine has no
environment model: `grep -rn "PDY_ENV" src iter3` at HEAD returns **0 matches** and there is no
`dev|test|qa|prod` vocabulary anywhere. So the demo file declares `environment: ["dev"]`, the
engine stores that word verbatim, `iter demo --run` refuses when the caller's word is not in the
list, and the engine never interprets the word.

**Secrets never appear in parameters.** They are printed in `iter markers` JSON and in the
structure map. A parameter is a name, a default and a description; a credential is none of
those. Have `iter validate` refuse a non-empty `default` on a parameter whose name ends in
`KEY`, `TOKEN`, `SECRET`, `PASSWORD` or `CREDENTIAL`. Secrets reach a run through the
environment the operator already has.

## 6. Acceptance — the checks that prove each behaviour

1. **Role recognised.** `iter validate --template --file /tmp/x.demo.iter.md` prints a template,
   exit 0. (Today: exit 2, "matches no nodetype".)
2. **Dot rule.** Beside `dot_rule_decides_the_role` (`src/markers.rs:1270-1291`; V3 copy at
   `iter3/iter_local/src/markers.rs:1252`): `role_of("a.b.demo.iter.md") == Some(Role::Demo)`,
   `role_of("mydemo.iter.md") == None`, `role_of("a.Demo.iter.md") == None` (case-sensitive).
3. **Template/validator guard.** Add `"a.demo.iter.md"` to the loop at `src/validate.rs:945`;
   `templates_match_the_validator` passes unchanged.
4. **Discovery.** After migration `iter markers | jq '.demos[].file'` lists the three pdy-dev
   demo files, and `jq '.usecases[].file'` no longer does.
5. **NEGATIVE CONTROL — an unknown child key is refused.** A `.demo.iter.md` declaring
   `children.frobnitz: []` makes `iter validate --file` report `unknown-child-key` naming the
   valid list, exit 1 (`src/validate.rs:305-321`). A clean run means the `Demo` arm of
   `children_keys` was never consulted and everything else here is unproven. Second control that
   the table is what refuses: drop `launch` from that row and a demo's own `launch:` key must
   then be reported unknown; restore it and the finding must clear.
6. **Sweep.** `iter testsweep` counts each demo `undeclared` and births no testwriter item.
   Control: remove `testgroups: []` from a demo file and confirm §5's behaviour, not the
   use-case one.
7. **Run and record.** `iter demo --run <demo file> --dry-run` prints the command line and
   changes no file. Without `--dry-run` the node's run block gains a real exit code, log path
   and duration. Control: point `launch` at a two-line script that exits 7 — the recorded exit
   code reads 7, not 0 and not 1.
8. **Sequence.** A composed demo whose second entry exits non-zero records that failure and
   never runs the third, whose run block is unchanged.
9. **Tag.** `iter add --demo demo03 ...` produces an item carrying `demo:demo03`, and an item
   created **by** it carries the tag too (`iter3/iter_data/src/api.rs:768-774`). Control:
   `--demo nosuchdemo` is refused.
10. **Locks.** `iter add --agent demo --codepath demos/03_stream_employee_wages/` is accepted;
    `--codepath core/repos/` is refused by the lockshape check
    (`iter3/iter_data/src/api.rs:930-971`).
11. **Migration.** On a copy of pdy-dev: `--dry-run` prints every rename and frontmatter edit
    and writes nothing; the real run leaves `iter markers` showing three demos each owning its
    own `bizreqs`/`techreqs`, those globs gone from `demos/demos.code.iter.md`, `iter validate`
    clean, and a leftover `*.demo.usecase.iter.md` carrying the rename warning.
