cd ~/dev/pdy-dev && ~/dev/iterapp/iter3/bin/iter_engine --config .iter/config.json

https://ivi88v0rwc.execute-api.us-west-2.amazonaws.com/
user:     gerald
password: gYDEij%L-VX5PApLRSr!


------------------------------------------------------------
------------------------------------------------------------
------------------------------------------------------------




Done. Both repos are committed: iter at e950724 and pdy-dev at b63d5657. Nothing is pushed.

What changed in pdy-dev/interfaces/. The 92 contract files became 657, one operation per file, all validating clean under the new rules. Every field is stated once with its type and meaning. Every refusal code is a one-line entry in the failure shape. Each file carries one success and one refusal worked example. Shared objects are their own dataset files, defined once: the attestation, the evidence reference, and 34 family records such as token claims and journal entries. The Invariants section is gone everywhere.

  ┌───────────────────────────────────┬────────┬────────┐
  │              Measure              │ Before │ After  │
  ├───────────────────────────────────┼────────┼────────┤
  │ Interface files                   │ 92     │ 657    │
  ├───────────────────────────────────┼────────┼────────┤
  │ Total bytes                       │ 996 KB │ 1.6 MB │
  ├───────────────────────────────────┼────────┼────────┤
  │ Largest file                      │ 53 KB  │ 8.7 KB │
  ├───────────────────────────────────┼────────┼────────┤
  │ Validate findings on interfaces   │ 283    │ 0      │
  ├───────────────────────────────────┼────────┼────────┤
  │ Files bundling several operations │ 56     │ 0      │
  └───────────────────────────────────┴────────┴────────┘

Total bytes went up, not down. Each file repeats about 500 bytes of frontmatter and section scaffolding, and 657 copies of that outweighs the 380 KB of examples and law that were cut. The per-file content is small and readable, which was the goal, but the corpus is bigger.

What changed around the contracts. Code nodes now link each family with a glob like pdy-core-authority-membership-*, and the new datasets are linked from the code node that owns them. About 200 prose mentions in requirements and tests name the family the same way. The techreq law now says one operation per file and retires the old one-id-per-component rule. The section guard pdy-dev built during the 64 KB incident, with its three test scripts, now mirrors the new format and passes. The Linux engine binary is rebuilt with the new validator, so Engine01 gets it on its next pull.

For your review. Every invariant bullet that cited no requirement id was kept verbatim in devops/plan/interface_rewrite_2026-09-08_dropped_invariants.md, 217 KB grouped by source file. Anything there that is a real rule not yet in the requirements should be filed there. The rest can be deleted with the file.
> I don't understand; "invariant bullet that cited no requirement id" so the content were "invariants" that appeared in some interface file, but "cited no requirement" from where? a *req.iter.md file?

Judgment calls the rewrite agents flagged.
- Where an original listed a service-wide refusal vocabulary, codes were apportioned per operation by reading the rules. 
> "original" what ?   "service-wide refusal" of what?   are you saying we removed the rules required to apportion codes?

Reads that were ambiguous got the wider list rather than the narrower one. 
> what list?  WTF are you talking about?

Total bytes went up, not down. Each file repeats about 500 bytes of frontmatter and section scaffolding, and 657 copies of that outweighs the 380 KB of examples and law that were cut. The per-file content is small and readable, which was the goal, but the corpus is bigger.
> that's fine; i'm not worried about disk space, I want to limit unneeded context.


What changed around the contracts. Code nodes now link each family with a glob like pdy-core-authority-membership-*, and the new datasets are linked from the code node that owns them. About 200 prose mentions in requirements and tests name the family the same way. The techreq law now says one operation per file and retires the old one-id-per-component rule. The section guard pdy-dev built during the 64 KB incident, with its three test scripts, now mirrors the new format and passes. The Linux engine binary is rebuilt with the new validator, so Engine01 gets it on its next pull.

For your review. Every invariant bullet that cited no requirement id was kept verbatim in devops/plan/interface_rewrite_2026-09-08_dropped_invariants.md, 217 KB grouped b

Total bytes went up, not down. Each file repeats about 500 bytes of frontmatter and section scaffolding, and 657 copies of that outweighs the 380 KB of examples and law that were cut. The per-file content is small and readable, which was the goal, but the corpus is bigger.

For your review. Every invariant bullet that cited no requirement id was kept verbatim in devops/plan/interface_rewrite_2026-09-08_dropped_invariants.md, 217 KB grouped by source file. Anything there that is a real rule not yet in the requirements should be filed there. The rest can be deleted with the file.

Judgment calls the rewrite agents flagged.
- Where an original listed a service-wide refusal vocabulary, codes were apportioned per operation by reading the rules. Reads that were ambiguous got the wider list rather than the narrower one.
- Operations with no refusal in the original carry one success example and a note, not an invented code. Placeholders like <unspecified> remain where the original had them.
- Three argv-driven tools in the infra-comm family stayed as single files because their "questions" are not a JSON discriminator. They can be split the same way if you want.
- Split names follow the rule mechanically, which yields a few redundant stems such as the account family's -account-account-create. Easy to shorten if you prefer.

Pre-existing issues I left alone. Forty shared-library interface files and the data deletion event were not linked from any code node before today, and four code node links pointed at interface files that do not exist. Neither is from this work.

`Still open. Workitem 7e860d5b is held as a question at the close gate. 
> at the workitem's close gate?   What is this telling me?

Its operations...
> "its" meaning the workitem, or the close gate?

...now exist as per-operation files, so it can close once you answer it. 
> per-operation files?   

`



No engine is running for pdy-dev right now. The recorded pid in .iter/engine.pid is dead, and no iter_engine process is alive on this Mac. The last run was the Mac engine named StephenMBP, and it left the project in the Stopped state.

To start it on this Mac, run from the pdy-dev checkout with the arm64 build from the iter repo:

cd ~/dev/pdy-dev && nohup ~/dev/iter/iter3/bin/iter_engine --config .iter/config.local.json >> .iter/engine.log 2>&1 & echo $! > .iter/engine.pid

Or in the foreground while you watch it:

cd ~/dev/pdy-dev && ~/dev/iter/iter3/bin/iter_engine --config .iter/config.local.json

A few things to know:

- Use config.local.json, not config.json. The tracked config names the engine Engine01. The local one names it StephenMBP, which is the engine record the recent runs and the log belong to. Both point at the same data URL and the same env file.
- Do not use the start_engine.sh script here. It execs .iter/iter_engine, which is a Linux x86-64 build for the Ubuntu hosts. It only warns on macOS and then fails to run. It is also the wrong shape for the daemon step, since it hardcodes that binary.
- The env file is already in place. It sets the engine token plus the three account tokens DEV1, DEV2, and DEV3.
- Set the project to Running afterwards. The engine drained pdy-dev to Stopped on its last run, so after the engine is up, flip the project's state in the web UI or the engine will heartbeat but pick nothing.
- Check it with the log, which should show a fresh "StephenMBP starting against ..." line:

tail -f ~/dev/pdy-dev/.iter/engine.log

If you meant the Ubuntu host instead, that is where start_engine.sh applies. Run .iter/start_engine.sh --daemon there after a git pull, since the FHServer engine still needs the newer build pulled and restarted.


2. Keep the name and build the "until" rule so the tag means what it says.

- blocked-by-cluster-restart = blocks workitem WHILE a cluster rebuild is happening
- blocked-until-cluster-restart = blocks workitems UNTIL the culster is rebuilt pl