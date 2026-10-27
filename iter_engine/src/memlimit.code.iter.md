---
id: bba8bd8f-ffbe-44f3-8671-129f12fe4dc6
name: "Memory limits"
desc: "Runs each agent session and each test script in a Linux cgroup of its own with a memory limit, and says in plain words when that limit killed something, so that one runaway process ends its own session instead of the whole engine service and every other agent with it."
creator: "agent.code"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_engine/src/memlimit.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-27 16:41:21Z", last_modified: "2026-10-27 16:41:21Z", last_tested: ""}
---

# Long Description

## Summary

Gives every agent and every test a memory budget, so one program that eats all the memory stops only its own work.

The Memory limits part caps how much memory one agent session (the `claude` process and everything it starts) and one test script may use. A cgroup is the Linux kernel's group of processes that share limits and counters; cgroup v2 is its current form. It exists because of 2026-10-27: a mutation-testing run turned a loop into an endless list, its test binary grew to 13 GB in seconds, the kernel's out-of-memory killer took it, and systemd then stopped the whole `iter-engine` service, killing every other session, once an hour while the item re-ran.

How it works (`iter_engine/src/memlimit.rs`): at startup `init` checks that it runs on Linux, finds its own cgroup v2 (`parse_proc_cgroup`, `parse_mountinfo_cgroup2`), that the memory controller is there and that it is alone in that cgroup, then moves itself into `<unit>/engine` and turns on the memory controller for the unit's children. cgroup v2 lets only a cgroup holding no processes give its children limits, which is why the engine moves out and why a session's processes sit in a `session` leaf. `session` makes `<unit>/agent-<n>-<item>/` with `memory.max`, `memory.swap.max` and `memory.oom.group=1` (the kernel kills the whole session together) and the leaf; `Scope::apply` puts the child in it before it runs anything, through `pre_exec` writing to a `cgroup.procs` file opened before the fork. Inside the session, `iter runtests` finds that cgroup in `$ITER_MEM_CGROUP` and `test_scope` gives each script `test-…` with the test-script limit; there `oom.group` stays 0, so only the runaway process dies and the script goes on. After the run `Scope::report` reads `memory.events` (`oom_kill`, `oom_group_kill`, `max`) and the scope removes its cgroups.

The Work runner calls it through `work::wait_in_session` for every model call the Model providers make and for exec items; the Test runner calls `test_scope` in `run_script`. Settings come from the engine's `--session-memory-max`, `--session-swap-max`, `--test-memory-max` and `--test-swap-max` flags or `ITER_*` variables (defaults 40% of memory, 1G, 30%, 512M).

Why it matters: without it one bad test takes every running agent down with the service. Without delegation, on cgroup v1, macOS or Windows it is off, says so in one startup line, and everything runs as before.

Example: a session's test binary grows past 6.2 GiB; the kernel kills that session, its item's last error reads "agent session killed: its processes reached the 6.2 GiB memory limit for one agent session (memory.events oom_kill=2, oom_group_kill=1, max=37)", and the other sessions keep running.
