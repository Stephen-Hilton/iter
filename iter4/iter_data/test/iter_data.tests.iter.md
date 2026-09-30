---
id: c0f3cd16-cbb8-448d-9e35-dc58a31ed0a7
name: "iter_data unit tests"
description: "Every unit test in the iter_data crate."
children:
  testpaths: ["{thisfiledir}/*.sh"]
---

# iter_data — unit tests

Two groups: the crate's whole `cargo test` run, counted per test; and GraphRAG
search quality — `rag_eval.sh` asks every question in `rag_eval.json` against a
running iter_data in the hybrid, vector and keyword modes, and is green when the
hybrid mode finds an expected node file in the top 5 for at least 80% of them.

<!-- iterapp:testgroups
{"label":"rag-search-quality","desc":"GraphRAG: hit@5 of rag_eval.json questions on a running iter_data","auto_fix":false,"lastrun":"","result":"","counts":"","testlist":[{"id":"t1","name":"search quality","desc":"hybrid hit@5 >= 0.8; vector and keyword modes reported for comparison","shell":"rag_eval.sh"}]}
-->

<!-- iterapp:testgroups
{"label":"iter_data-unit","desc":"cargo test -p iter_data","auto_fix":false,"lastrun":"2026-09-29T03:48:56Z","result":"passed","counts":"31/31","testlist":[{"id":"t1","name":"cargo unit tests","desc":"every #[test] in the iter_data crate","shell":"cargo_test.sh"}]}
-->
