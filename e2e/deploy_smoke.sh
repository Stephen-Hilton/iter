#!/usr/bin/env bash
# deploy_smoke.sh — prove a DEPLOYED iter5 works end to end, then clean up.
#
#   ./e2e/deploy_smoke.sh [--url http://127.0.0.1:8400]
#
# Against the running server it: logs in as admin (ITER_ADMIN_PASSWORD from
# the repo .env or the environment), creates a throwaway engine user, agent, mock
# account and DESIGNED project, designs two nodes in the graph, starts a real
# engine (target/debug/iter_engine, mock provider) that builds the project into
# a scratch repo, runs one work item, checks file → node and node → file sync,
# calls MCP and loads the webui. Everything it created is removed at the end
# (records via the API; the scratch repo is moved to the scratch dir's trash).
set -uo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
BASE="http://127.0.0.1:8400"
[ "${1:-}" = "--url" ] && BASE="${2%/}"
ENV_FILE="${ITER_ENV_FILE:-$REPO/.env}"
envval() { [ -f "$ENV_FILE" ] && grep -E "^$1=" "$ENV_FILE" | tail -1 | cut -d= -f2- | sed -e 's/^["'\'']//' -e 's/["'\'']$//' || true; }
PASS_ADMIN="${ITER_ADMIN_PASSWORD:-$(envval ITER_ADMIN_PASSWORD)}"
ENGINE_BIN="${ITER_ENGINE_BIN:-$REPO/target/debug/iter_engine}"
TS=$(date +%s); P="smoke5_$TS"; ENG="smoke5_eng_$TS"; EUSER="smoke5_engine_$TS"; AG="smoke5_agent_$TS"; ACCT="smoke5_acct_$TS"
SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/iter5-smoke.XXXXXX")"; TOP="$SCRATCH/repo"; ENVF="$SCRATCH/engine.env"
PASS=0; FAIL=0; FAILED=()
ok()     { PASS=$((PASS+1)); printf '\033[32mPASS\033[0m %s\n' "$1"; }
bad()    { FAIL=$((FAIL+1)); FAILED+=("$1"); printf '\033[31mFAIL\033[0m %s\n' "$1"; }
check()  { local n="$1"; shift; if "$@" >/dev/null 2>&1; then ok "$n"; else bad "$n"; fi; }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (expected '$2', got '$3')"; fi; }
api()  { local b=(); [ $# -ge 3 ] && b=(-d "$3"); curl -sf -X "$1" -H "authorization: Bearer $TOKEN" -H content-type:application/json "$BASE$2" ${b[@]+"${b[@]}"}; }
wait_for() { local t=$1; shift; local i=0; while [ $i -lt $((t*2)) ]; do eval "$*" >/dev/null 2>&1 && return 0; sleep 0.5; i=$((i+1)); done; return 1; }
ENGINE_PID=""
cleanup() {
  [ -n "$ENGINE_PID" ] && kill "$ENGINE_PID" 2>/dev/null && wait "$ENGINE_PID" 2>/dev/null
  if [ -n "${TOKEN:-}" ]; then
    curl -s -X DELETE -H "authorization: Bearer $TOKEN" "$BASE/api/projects/$P" >/dev/null
    curl -s -X DELETE -H "authorization: Bearer $TOKEN" "$BASE/api/settings/nodes/account:$ACCT" >/dev/null
    curl -s -X DELETE -H "authorization: Bearer $TOKEN" "$BASE/api/agents/$AG" >/dev/null
    curl -s -X DELETE -H "authorization: Bearer $TOKEN" "$BASE/api/engines/$ENG?force=true" >/dev/null
    curl -s -X DELETE -H "authorization: Bearer $TOKEN" "$BASE/api/settings/nodes/iter_engine:$ENG" >/dev/null
    curl -s -X DELETE -H "authorization: Bearer $TOKEN" "$BASE/api/settings/nodes/user:$EUSER" >/dev/null
  fi
  echo "scratch kept for inspection: $SCRATCH"
}
trap cleanup EXIT

echo "== deploy smoke against $BASE (project $P)"
check "health ok" bash -c "curl -sf $BASE/health | jq -e '.ok==true'"
check "webui page served" bash -c "curl -sf $BASE/ | grep -qi '<html'"
check "engine setup script served (iter5)" bash -c "curl -sf $BASE/iter_engine_setup.sh | grep -q 'ONE engine per machine'"
TOKEN=$(curl -sf -X POST "$BASE/auth/login" -H content-type:application/json -d "{\"user\":\"admin\",\"password\":\"$PASS_ADMIN\"}" | jq -r .token)
[ -n "$TOKEN" ] && [ "$TOKEN" != null ] && ok "admin login" || { bad "admin login"; exit 1; }
[ -x "$ENGINE_BIN" ] || { bad "engine binary $ENGINE_BIN (cargo build -p iter_engine)"; exit 1; }

api PUT "/api/users/$EUSER" '{"role":"engine","password":"unused-smoke-pw","email":""}' >/dev/null
ETOKEN=$(api POST "/api/users/$EUSER/token" '{"ttl_days":1}' | jq -r .token)
[ -n "$ETOKEN" ] && [ "$ETOKEN" != null ] && ok "engine user + token" || bad "engine user + token"
api PUT "/api/agents/$AG" "{\"desc\":\"deploy smoke\",\"max\":2,\"timeoutsec\":60,\"model\":\"\",\"promptbody\":\"smoke agent\"}" >/dev/null && ok "agent created" || bad "agent created"

# designed project: no engine yet
api PUT "/api/projects/$P" '{"desc":"deploy smoke project","state":"Running","gitrepo":"","maxagents":{"else":2}}' >/dev/null && ok "designed project created" || bad "designed project created"
G=$(api GET "/api/projects/$P/graph")
expect "designer: project node seeded" 1 "$(echo "$G" | jq '[.nodes[]|select(.nodetype=="project")]|length')"
PN=$(echo "$G" | jq -r '.nodes[]|select(.nodetype=="project")|.id')
CTX=$(api POST "/api/projects/$P/graph/nodes" "{\"nodetype\":\"code\",\"level\":\"context\",\"name\":\"Smoke context\",\"desc\":\"designed in the graph\",\"attach_to\":\"$PN\",\"attach_kind\":\"codenodes\"}" | jq -r .node.id)
REQ=$(api POST "/api/projects/$P/graph/nodes" "{\"nodetype\":\"techreq\",\"name\":\"Smoke rule\",\"desc\":\"a local rule\",\"attach_to\":\"$CTX\",\"attach_kind\":\"reqs\"}" | jq -r .node.path)
expect "designed nodes are 'designed' (no repo yet)" designed "$(api GET "/api/projects/$P/graph/nodes/$CTX" | jq -r .file_state)"

# engine + mock account wired in the settings graph, then build
api PUT "/api/engines/$ENG" "{\"host\":\"smoke\",\"state\":\"Stopped\",\"ticksec\":1,\"user\":\"$EUSER\"}" >/dev/null
api POST /api/settings/nodes "{\"type\":\"account\",\"name\":\"$ACCT\",\"settings\":{\"provider\":\"mock\",\"token_envar\":\"SMOKE_ACCT_TOKEN\"}}" >/dev/null
api POST /api/settings/edges "{\"from\":\"iter_engine:$ENG\",\"to\":\"account:$ACCT\"}" >/dev/null
api POST /api/settings/edges "{\"from\":\"account:$ACCT\",\"to\":\"project:$P\",\"settings\":{\"order\":1,\"switch\":80,\"stop\":95}}" >/dev/null
printf 'ITER_ENGINE_TOKEN=%s\nSMOKE_ACCT_TOKEN=mock-token\n' "$ETOKEN" > "$ENVF"
"$ENGINE_BIN" --data-url "$BASE" --env-file "$ENVF" --name "$ENG" > "$SCRATCH/engine.log" 2>&1 &
ENGINE_PID=$!
wait_for 20 "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/engines/$ENG | jq -e '.last_seen|length>0'" && ok "engine checked in" || bad "engine checked in"
api POST "/api/projects/$P/build" "{\"engine\":\"$ENG\",\"topdir\":\"$TOP\",\"queue_plan\":false}" >/dev/null && ok "build requested" || bad "build requested"
wait_for 60 "[ \"\$(api GET /api/projects/$P/build | jq -r '.state // .build.state')\" = done ]" && ok "build done" || bad "build done ($(api GET /api/projects/$P/build | jq -c .))"
check "repo created with git" test -d "$TOP/.git"
check "designed requirement written to its reqs/ folder" test -f "${REQ/\{topdir\}/$TOP}"
check "build committed" bash -c "git -C '$TOP' log --format=%s | grep -q 'iter: build from design'"

# a work item through the mock provider
mkdir -p "$TOP/out"
echo "someone else's work" > "$TOP/unrelated.txt"   # must stay out of the item's commit
WI=$(api POST "/api/projects/$P/workitems" "{\"name\":\"smoke write\",\"agent\":\"$AG\",\"priority\":3,\"lockdirs\":[\"{topdir}/out/\"],\"request\":\"mock: write smoke.txt <<<smoke-done>>>\"}" | jq -r .id)
wait_for 60 "[ \"\$(api GET /api/projects/$P/workitems/$WI | jq -r .state)\" = complete ]" && ok "work item completed" || bad "work item completed ($(api GET /api/projects/$P/workitems/$WI | jq -r .state))"
expect "agent's file written" smoke-done "$(cat "$TOP/out/smoke.txt" 2>/dev/null)"
check "item commit is scoped to its lockdir" bash -c "git -C '$TOP' log -1 --format= --name-only --grep='smoke write' | grep -qx 'out/smoke.txt' && git -C '$TOP' status --porcelain | grep -q 'unrelated.txt'"

# file → node, node → file
CF=$(find "$TOP" -name '*.code.iter.md' | head -1)
sed -i.bak 's/^desc: .*/desc: "edited in the repo"/' "$CF" && mv "$CF.bak" "$SCRATCH/"
wait_for 20 "[ \"\$(api GET /api/projects/$P/graph/nodes/$CTX | jq -r .desc)\" = 'edited in the repo' ]" && ok "file edit → node" || bad "file edit → node"
api PATCH "/api/projects/$P/graph/nodes/$CTX" '{"desc":"edited in the graph"}' >/dev/null
wait_for 20 "grep -q '^desc: \"edited in the graph\"' '$CF'" && ok "node edit → file" || bad "node edit → file"
wait_for 10 "[ -z \"\$(git -C '$TOP' status --porcelain -- '$CF')\" ]" && ok "node edit committed" || bad "node edit committed"

# MCP
MT=$(curl -sf -X POST "$BASE/mcp" -H "authorization: Bearer $TOKEN" -H "X-Iter-Project: $P" -H content-type:application/json -H 'accept: application/json, text/event-stream' -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')
if printf '%s' "$MT" | jq -e '.result.tools|length>10' >/dev/null 2>&1; then ok "MCP tools/list ($(printf '%s' "$MT" | jq '.result.tools|length') tools)"; else bad "MCP tools/list"; fi

echo
echo "deploy smoke: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ] || { printf '  %s\n' "${FAILED[@]}"; tail -20 "$SCRATCH/engine.log"; exit 1; }
