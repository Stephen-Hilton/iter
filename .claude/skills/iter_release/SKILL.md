---
description: Cut a new iter release end to end - commit and push local work, pull and reconcile GitHub, test everything and fix what fails, check every installer, rebuild all 7 binaries and publish a vYYYY.MM.DD<letter> GitHub release (e.g. v2026.10.20a). Use when the user asks to release, ship, or cut a new version of iter.
disable-model-invocation: true
---

# /iter_release

Release iter from this checkout (`~/dev/iter`, GitHub `Stephen-Hilton/iter`, branch `main`). Work through
the steps in order; each one has a gate. Never skip a gate silently: if one cannot be met, stop and tell
the user exactly what is blocking and what you already did.

Read `CLAUDE.md` first: its "Keep installers/ current" and "Secrets and sign-in (do not weaken)" rules apply
to every change you make or accept here. Never `rm` (move to the scratchpad instead), never force-push,
never rewrite published history, never commit `.env`, tokens, keys or `installers/dist/`.

Keep a short running log (step, result, commits made) to report at the end.

## 0. Preflight

- `git branch --show-current` is `main` (if not, ask the user before going on).
- `gh auth status` succeeds; `docker info` succeeds (the Linux builds and the installer containers need it).
- `git status`, `git log origin/main..HEAD`, `git stash list`: see what is outstanding before touching it.

## 1. Commit and push outstanding local work

- Read the whole diff (`git diff`, `git diff --cached`, untracked files from `git status`). Understand it
  before committing; if something looks unfinished, broken or like a secret, ask the user.
- Commit in logical pieces with messages in the style of `git log` (short summary line, a body that says
  why). End each message with the attribution line from the session's instructions.
- `git push`. If the push is rejected because origin moved, go to step 2 first, then push.

## 2. Pull and reconcile GitHub

- `git fetch --tags origin`, then list what is coming in: `git log --stat HEAD..origin/main`.
- `git pull --rebase origin main` (fall back to a merge if the rebase gets tangled). Resolve every conflict
  by keeping the intent of both sides, never by taking one side wholesale; read both commits' messages.
- Review every incoming commit like a code review: does it break the rules in `CLAUDE.md` (TLS, engine
  keys, secrets, iter dir layout), drop a test, leave docs or installers behind the code? Note issues and
  fix them in step 3 or 4. Other machines (e.g. the Omarchy laptop) push fixes found in the field; keep
  them, and look for the same bug elsewhere in the code.
- Push the reconciled `main`.

## 3. Test, and fix what fails

Run everything from the repo root. Fix every failure at its cause (code or a stale test), rerun until green.
A flaky test that passes on rerun: rerun it twice more and mention it in the report.

1. `cargo build --release -p iter_engine -p iter_data` (both, never one alone or a bare `cargo build`).
2. `cargo clippy --workspace --all-targets` - fix new warnings in code you or the pulled commits touched.
3. Unit tests: `test/cargo-test-crate.sh iter_core`, `... iter_engine`, `... iter_data`.
   iter_data's tests and the e2e run need ArangoDB at 127.0.0.1:8529 (root password `iterdev`). If
   `curl -sf -u root:iterdev http://127.0.0.1:8529/_api/version` fails (the port may be held by something
   else, e.g. a kubectl port-forward), start a throwaway one and point the tests at it:
   `docker run -d --name iter-release-arango -p 18529:8529 -e ARANGO_ROOT_PASSWORD=iterdev arangodb:3.12 arangod --vector-index`
   (the GraphRAG store tests need vector indexes),
   wait for `/_api/version`, then `export ITER_TEST_ARANGO_URL=http://127.0.0.1:18529`. Stop and remove it
   at the end of the skill.
4. End to end: `./test/e2e.sh` (iter_data + one engine on the mock provider). Every check must PASS.
5. Web UI: `test/e2e/playwright/run.sh` when anything under `iter_data/webui/` or the API changed since the
   last release tag (`git diff --stat <last tag>..HEAD`).
6. Search quality: `test/iter_data/rag_eval.sh` when search, embedding or GraphRAG code changed.

Gate: everything green. Commit the fixes (step 5 pushes them).

## 4. Check every installer

Scope with `git diff --stat <last tag>..HEAD -- installers/ iter_engine/ iter_data/ iter_core/`.
`installers/` holds one self-contained install script per program and platform plus `build.sh` and
`upgrade_iter.sh`.

- `bash -n` and `shellcheck` on every `installers/**/*.sh` (skip `installers/dist/`); `cfn-lint
  installers/iter_data/aws/iter_data.cfn.yaml`; the PowerShell installer by reading it (and
  `pwsh -NoProfile -Command '$null = [scriptblock]::Create((Get-Content -Raw installers/iter_engine/windows/install.ps1))'`
  when `pwsh` exists).
- If code changed what an installed machine needs (a flag, env var, port, file in the iter dir, package,
  service setting), every affected install script and its README must say so. Fix gaps.
- `--dry-run` each changed Linux installer inside containers of the target distros: `debian:bookworm`,
  `ubuntu:24.04`, `amazonlinux:2023` (and `archlinux` for the linux installers, amd64 emulation on this
  Mac). The macOS installer: `--dry-run` here.
- `installers/README.md` and `installers/installers.code.iter.md` still describe the layout.

Gate: lints clean, dry runs succeed, docs match.

## 5. Commit and push again, if needed

If steps 3-4 changed anything: commit (logical pieces), `git push`, and confirm
`git status` is clean and `git log origin/main..HEAD` is empty. `build.sh --release` refuses otherwise.

## 6. Build all binaries

`installers/build.sh` (no flags: all 7 assets; Linux x86_64/aarch64 for both programs in the
`rust:1-bullseye` docker image, macOS aarch64/x86_64 and Windows x86_64 for the engine). Check its summary:

- all 7 assets built, none failed or skipped;
- every Linux binary needs glibc <= 2.31 (the build fails it otherwise);
- run the ones this Mac can: `installers/dist/iter_engine-macos-aarch64 --version` and the x86_64 one
  (Rosetta); the Linux ones in `debian:bullseye` containers (`--platform linux/amd64` and `linux/arm64`);
  check the Windows one with `file` (a PE32+ x86-64 executable).

Do not edit `build.sh` (or any script) while it runs: bash reads scripts as it goes.

## 7. Publish the release

`installers/build.sh --release`. It rebuilds all 7 assets from the clean, pushed HEAD, picks the tag
`v<YYYY>.<MM>.<DD><letter>` (UTC date; `a` for the day's first release, then `b`, `c`, ...), pushes the tag
and creates the GitHub release with the 7 binaries and `SHA256SUMS`.

Verify after it finishes:

- `gh release view <tag>` lists 8 assets;
- download `SHA256SUMS` and every asset into a fresh scratchpad folder and `shasum -a 256 -c SHA256SUMS`;
- `curl -sI https://github.com/Stephen-Hilton/iter/releases/latest` redirects to the new tag
  (`upgrade_iter.sh` and the installers resolve "latest" this way).

## 8. Report

Stop and remove the throwaway ArangoDB container if you started one. Then tell the user, plainly:

- the release tag and its URL;
- what was committed (hash + one line each) and what came in from GitHub;
- what was tested and the results, every fix you made and why, any flaky test;
- installer checks done, and anything not checked (e.g. the Windows installer could not be run);
- whether installed machines need anything beyond `upgrade_iter.sh` (e.g. a new option or a migration).
