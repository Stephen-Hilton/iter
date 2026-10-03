#!/usr/bin/env bash
# iter5 end-to-end (spec §11): iter_data (ArangoDB) + ONE iter_engine serving
# TWO projects, every agent turn on the deterministic `mock` provider.
#
# Usage: iter5/e2e.sh [--live] [--keep]
#   --live   also queue one tiny item to a real `claude` account (needs
#            ITER_LIVE_CLAUDE_TOKEN in the environment; skipped otherwise)
#   --keep   keep the scratch directory (the e2e database is always dropped)
#
# Uses the dev ArangoDB on :8529 and a throwaway database iter5_e2e_<random>,
# dropped on success and failure. Every check prints PASS/FAIL; a section that
# cannot continue says so and the run goes on with the next one. The tally at
# the end lists every failure; the exit code is non-zero on any.
#
# Fixtures: e2e/sample5 (a small v5 project, copied twice into scratch git
# repos) and e2e/iter4_fixture (a small iter4 tree for `iter migrate5`). Their
# node files are stored as `*.iter.md.fixture` so iter5's own node scan never
# reads them as nodes of iter5; the copies get the real names back.
set -uo pipefail

LIVE=0; KEEP=0
for a in "$@"; do
  case "$a" in
    --live) LIVE=1 ;;
    --keep) KEEP=1 ;;
    *) echo "usage: $0 [--live] [--keep]"; exit 2 ;;
  esac
done

REPO="$(cd "$(dirname "$0")" && pwd)"   # the iter5 directory
ARANGO_URL_E2E="${ITER5_TEST_ARANGO_URL:-${ARANGO_URL_E2E:-http://127.0.0.1:8529}}"
ARANGO_PW_E2E="${ITER5_TEST_ARANGO_PASSWORD:-${ARANGO_ROOT_PASSWORD:-iter4dev}}"
ARANGO_DB_E2E="iter5_e2e_$RANDOM$RANDOM"
SCRATCH="${E2E_SCRATCH:-$(mktemp -d /tmp/iter5-e2e.XXXXXX)}"
PORT=$((18400 + RANDOM % 1000))
BASE="http://127.0.0.1:$PORT"
PASS_ADMIN="e2e-admin-pw"
ENGINE=E2E                       # the one real engine
PA=e2e5_alpha; PB=e2e5_beta      # the two projects it serves
PN=e2e5_next                     # get_next probe project (served by the never-running engine "Probe")
PU=e2e5_unserved                 # served by nobody: engine-token refusals
PD=e2e5_design                   # designer project (built during the run)
DATA_PID=""; ENGINE_PID=""
T0=$(date +%s)

# ---------- output + tally ----------
NPASS=0; NFAIL=0; FAILED=(); SECTION="setup"; SECTIONS=(); SEC_P=(); SEC_F=(); SI=-1   # bash 3.2: no assoc arrays
section() { SECTION="$1"; SI=$((SI+1)); SECTIONS[$SI]="$1"; SEC_P[$SI]=0; SEC_F[$SI]=0; printf '\n\033[1;36m== %s ==\033[0m\n' "$1"; }
say()  { printf '\033[36m[e2e]\033[0m %s\n' "$*"; }
ok()   { NPASS=$((NPASS+1)); SEC_P[$SI]=$(( ${SEC_P[$SI]:-0} + 1 )); printf '\033[32mPASS\033[0m %s\n' "$*"; }
ko()   { NFAIL=$((NFAIL+1)); SEC_F[$SI]=$(( ${SEC_F[$SI]:-0} + 1 )); FAILED+=("[$SECTION] $*"); printf '\033[31mFAIL\033[0m %s\n' "$*"; }
# check "<what>" <command…>: PASS when the command succeeds
check() { local m="$1"; shift; if "$@" >/dev/null 2>&1; then ok "$m"; else ko "$m"; fi; }
# expect "<what>" <expected> <actual>
expect() { if [ "$2" = "$3" ]; then ok "$1"; else ko "$1 (expected '$2', got '$3')"; fi; }

tally() {
  printf '\n\033[1m== tally (%ss) ==\033[0m\n' "$(( $(date +%s) - T0 ))"
  local i
  for i in $(seq 0 "$SI"); do printf '  %-40s pass %3d  fail %3d\n' "${SECTIONS[$i]}" "${SEC_P[$i]:-0}" "${SEC_F[$i]:-0}"; done
  printf '  %-40s pass %3d  fail %3d\n' "TOTAL" "$NPASS" "$NFAIL"
  if [ "$NFAIL" -gt 0 ]; then printf '\033[31mFAILED:\033[0m\n'; printf '  %s\n' "${FAILED[@]}"; fi
}

drop_e2e_db() { [ -n "${E2E_KEEP_DB:-}" ] && { say "database kept: $ARANGO_DB_E2E"; return 0; }; curl -s -o /dev/null -X DELETE -u "root:$ARANGO_PW_E2E" "$ARANGO_URL_E2E/_api/database/$ARANGO_DB_E2E" || true; }
cleanup() {
  [ -n "$ENGINE_PID" ] && kill "$ENGINE_PID" 2>/dev/null
  [ -n "$DATA_PID" ] && kill "$DATA_PID" 2>/dev/null
  wait 2>/dev/null
  drop_e2e_db
  if [ "$KEEP" = 0 ] && [ "$NFAIL" = 0 ] && [ -z "${E2E_SCRATCH:-}" ]; then rm -rf "$SCRATCH"; else say "scratch kept: $SCRATCH"; fi
}
trap cleanup EXIT
fatal() { ko "$*"; tally; exit 1; }
# E2E_ONLY=a,c runs setup + those sections only (development aid)
want() { [ -z "${E2E_ONLY:-}" ] || case ",$E2E_ONLY," in *",$1,"*) true ;; *) false ;; esac; }

command -v jq >/dev/null || { echo "jq is required"; exit 2; }
mkdir -p "$SCRATCH"
say "scratch=$SCRATCH port=$PORT db=$ARANGO_DB_E2E"

# ---------- HTTP helpers ----------
# api <METHOD> <path> [json]  -> body (admin token); fails on HTTP >= 400
# (bash 3.2 joins `${x:+-d "$x"}` into ONE word: bodies go through an array)
api()  { local b=(); [ $# -ge 3 ] && b=(-d "$3"); curl -sf -X "$1" -H "authorization: Bearer $TOKEN" -H content-type:application/json "$BASE$2" ${b[@]+"${b[@]}"}; }
eapi() { local b=(); [ $# -ge 3 ] && b=(-d "$3"); curl -sf -X "$1" -H "authorization: Bearer $ENGINE_TOKEN" -H content-type:application/json "$BASE$2" ${b[@]+"${b[@]}"}; }
# code <token> <METHOD> <path> [json] -> HTTP status
code() { local b=(); [ $# -ge 4 ] && b=(-d "$4"); curl -s -o /dev/null -w '%{http_code}' -X "$2" -H "authorization: Bearer $1" -H content-type:application/json "$BASE$3" ${b[@]+"${b[@]}"}; }
# wait_for <seconds> <shell condition>  — polls every 0.25 s; WAITED = seconds taken
wait_for() {
  local lim=$1; shift; local s; s=$(date +%s.%N 2>/dev/null || date +%s)
  local n=$(( lim * 4 ))
  for _ in $(seq 1 "$n"); do
    if eval "$*" >/dev/null 2>&1; then WAITED=$(echo "$(date +%s.%N 2>/dev/null || date +%s) - $s" | bc 2>/dev/null || echo "?"); return 0; fi
    sleep 0.25
  done
  WAITED="timeout"; return 1
}
# work items
wi_new()   { api POST "/api/projects/$1/workitems" "$2" | jq -r .id; }
wi()       { api GET "/api/projects/$1/workitems/$2"; }
wi_state() { wi "$1" "$2" | jq -r .state; }
wi_set()   { local J; J=$(wi "$1" "$2"); echo "$J" | jq "$3" | curl -sf -X PUT -H "authorization: Bearer $TOKEN" -H content-type:application/json "$BASE/api/projects/$1/workitems/$2?expect_version=$(echo "$J" | jq -r .version)" -d @- >/dev/null; }
details()  { api GET "/api/projects/$1/workitems/$2/details"; }
# project record edits (iter4 shape: maxagents, failure, state, maxdailycost)
proj_set() { api GET "/api/projects/$1" | jq "$2" | curl -sf -X PUT -H "authorization: Bearer $TOKEN" -H content-type:application/json "$BASE/api/projects/$1" -d @- >/dev/null; }
# graph
graph()    { api GET "/api/projects/$1/graph"; }
gnode()    { api GET "/api/projects/$1/graph/nodes/$2"; }
gnode_f()  { gnode "$1" "$2" | jq -r "$3"; }
# git
commits_touching() { git -C "$1" log --format=%s -- "$2" 2>/dev/null; }
clean_in_git()     { [ -z "$(git -C "$1" status --porcelain -- "$2")" ] && git -C "$1" ls-files --error-unmatch "$2" >/dev/null 2>&1; }

# ---------- engine control ----------
ENVF="$SCRATCH/engine.env"
ENGINE_LOG="$SCRATCH/engine.log"
engine_start() {
  : >> "$ENGINE_LOG"; echo "---- engine start $(date -u +%H:%M:%S) ----" >> "$ENGINE_LOG"
  "$ENGINE_BIN" --data-url "$BASE" --env-file "$ENVF" --name "$ENGINE" >> "$ENGINE_LOG" 2>&1 &
  ENGINE_PID=$!
}
engine_stop() {
  [ -n "$ENGINE_PID" ] || return 0
  kill "$ENGINE_PID" 2>/dev/null; wait "$ENGINE_PID" 2>/dev/null; ENGINE_PID=""
  echo "---- engine stop $(date -u +%H:%M:%S) ----" >> "$ENGINE_LOG"
}
log_mark() { wc -l < "$ENGINE_LOG" | tr -d ' '; }
log_since() { tail -n +"$(( $1 + 1 ))" "$ENGINE_LOG"; }

# usage snapshots live under $HOME/.claude by default: isolate them so the
# developer's real snapshots never gate this engine
export ITER_USAGE_DIR="$SCRATCH/usage"; mkdir -p "$ITER_USAGE_DIR"
FUT=$(( $(date +%s) + 86400 ))
snap() { printf '{"ts":"%s","rate_limits":{"five_hour":{"used_percentage":%s,"resets_at":%s},"seven_day":{"used_percentage":%s,"resets_at":%s}}}\n' \
  "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$2" "$FUT" "$3" "$FUT" > "$ITER_USAGE_DIR/iter3-usage-$1.json"; }

# =====================================================================
section "setup"
# =====================================================================
say "building debug binaries"
(cd "$REPO" && ~/.cargo/bin/cargo build -p iter_data -p iter_engine > "$SCRATCH/build.log" 2>&1) || { tail -30 "$SCRATCH/build.log"; fatal "cargo build"; }
DATA_BIN="$REPO/target/debug/iter_data"
ENGINE_BIN="$REPO/target/debug/iter_engine"
ok "debug binaries built"

ITER_ADMIN_PASSWORD=$PASS_ADMIN ARANGO_URL="$ARANGO_URL_E2E" ARANGO_PASSWORD="$ARANGO_PW_E2E" "$DATA_BIN" \
  --arango-db "$ARANGO_DB_E2E" --listen "127.0.0.1:$PORT" --secret-file "$SCRATCH/jwt.secret" \
  --env-file /dev/null --webui-dir "$REPO/webui" > "$SCRATCH/iter_data.log" 2>&1 &
DATA_PID=$!
wait_for 60 "curl -sf $BASE/health" || { tail -30 "$SCRATCH/iter_data.log"; fatal "iter_data did not start"; }
ok "iter_data up on :$PORT ($(curl -sf "$BASE/health" | jq -r .backend))"

TOKEN=$(curl -sf -X POST "$BASE/auth/login" -H content-type:application/json -d "{\"user\":\"admin\",\"password\":\"$PASS_ADMIN\"}" | jq -r .token)
[ -n "$TOKEN" ] && [ "$TOKEN" != null ] || fatal "admin login"
ok "admin login"
expect "bad password rejected" 401 "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/auth/login" -H content-type:application/json -d '{"user":"admin","password":"wrong"}')"
expect "unauthenticated request rejected" 401 "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/projects")"
check "webui static page served" bash -c "curl -sf $BASE/ | grep -qi '<html'"

api PUT /api/users/engine01 '{"role":"engine","password":"unused-login-pw","email":""}' >/dev/null || fatal "create engine user"
ENGINE_TOKEN=$(api POST /api/users/engine01/token '{"ttl_days":365}' | jq -r .token)
[ -n "$ENGINE_TOKEN" ] && [ "$ENGINE_TOKEN" != null ] || fatal "mint engine token"
ok "engine user + long-lived token"

# agents first: a project created afterwards gets a `runs` edge per agent
for ag in code plan test; do
  api PUT "/api/agents/$ag" "{\"desc\":\"e2e $ag\",\"max\":4,\"timeoutsec\":120,\"model\":\"\",\"promptbody\":\"You are the $ag agent.\"}" >/dev/null || fatal "agent $ag"
done
ok "agents code/plan/test created"

# two scratch repos from the sample5 fixture
mkrepo() { # mkrepo <dir>
  cp -R "$REPO/e2e/sample5" "$1"
  find "$1" -name '*.iter.md.fixture' | while read -r f; do mv "$f" "${f%.fixture}"; done
  git -C "$1" init -q && git -C "$1" config user.email e2e@iter && git -C "$1" config user.name e2e
  git -C "$1" add -A && git -C "$1" commit -qm seed
  # an agent session runs in its item's first lockdir when that exists (iter4
  # behaviour), so the lock scopes used below exist up front and every mock
  # path is relative to its lockdir
  (cd "$1" && mkdir -p out stop gate ask g1 g2 g3)
}
RA="$SCRATCH/alpha"; RB="$SCRATCH/beta"
mkrepo "$RA"; mkrepo "$RB"
ok "two sample5 repos git-initialized ($(find "$RA" -name '*.iter.md' | wc -l | tr -d ' ') node files each)"

for p in $PA $PB $PN $PU; do
  api PUT "/api/projects/$p" '{"desc":"e2e project","state":"Running","gitrepo":"","maxagents":{"else":3},"failure":{"maxattempts":2,"first_retry_second":1,"retry_backoff_exponent":2}}' >/dev/null || fatal "project $p"
done
ok "projects $PA $PB $PN $PU created"

# engine records: the real one (ticks every second) and the probe engine that
# never runs — both owned by engine01, so the engine token speaks for both
for e in $ENGINE Probe; do
  api PUT "/api/engines/$e" "{\"host\":\"e2e\",\"state\":\"Stopped\",\"ticksec\":1,\"full_refresh_minutes\":360,\"probe_stale_min\":0,\"queuelock\":{\"retryms\":50,\"breaksec\":60},\"user\":\"engine01\"}" >/dev/null || fatal "engine record $e"
done
edge() { api POST /api/settings/edges "$1" | jq -r .id; }
SERVE_A=$(edge "{\"from\":\"iter_engine:$ENGINE\",\"to\":\"project:$PA\",\"tag\":\"alpha_default\",\"settings\":{\"topdir\":\"$RA\",\"read_only\":false}}")
SERVE_B=$(edge "{\"from\":\"iter_engine:$ENGINE\",\"to\":\"project:$PB\",\"settings\":{\"topdir\":\"$RB\",\"read_only\":false}}")
SERVE_N=$(edge "{\"from\":\"iter_engine:Probe\",\"to\":\"project:$PN\",\"settings\":{\"topdir\":\"$SCRATCH/none\"}}")
[ -n "$SERVE_A" ] && [ "$SERVE_A" != null ] && [ -n "$SERVE_B" ] && [ "$SERVE_B" != null ] && [ -n "$SERVE_N" ] && [ "$SERVE_N" != null ] || fatal "serves edges"
ok "serves edges: $ENGINE → $PA, $PB; Probe → $PN"

# mock accounts: alpha bills acct_a (order 1) then acct_a2 (order 2); beta bills acct_b
for a in acct_a acct_a2 acct_b; do
  api POST /api/settings/nodes "{\"type\":\"account\",\"name\":\"$a\",\"settings\":{\"provider\":\"mock\",\"token_envar\":\"$(echo "$a" | tr a-z A-Z)_TOKEN\"}}" >/dev/null || fatal "account $a"
  edge "{\"from\":\"iter_engine:$ENGINE\",\"to\":\"account:$a\"}" >/dev/null
done
BILL_A=$(edge "{\"from\":\"account:acct_a\",\"to\":\"project:$PA\",\"settings\":{\"order\":1,\"switch\":80,\"stop\":95}}")
BILL_A2=$(edge "{\"from\":\"account:acct_a2\",\"to\":\"project:$PA\",\"settings\":{\"order\":2,\"switch\":80,\"stop\":95}}")
BILL_B=$(edge "{\"from\":\"account:acct_b\",\"to\":\"project:$PB\",\"settings\":{\"order\":1,\"switch\":80,\"stop\":95}}")
[ "$BILL_A" != null ] && [ "$BILL_A2" != null ] && [ "$BILL_B" != null ] || fatal "bills edges"
SG=$(api GET /api/settings/graph)
expect "settings graph: accounts are 'of' the mock provider" 3 "$(echo "$SG" | jq '[.edges[]|select(.type=="of" and .to=="provider:mock")]|length')"
expect "settings graph: runs edges for code/plan/test on $PA" "code plan test" "$(echo "$SG" | jq -r --arg p "project:$PA" '[.edges[]|select(.type=="runs" and .to==$p and .active)|.from|ltrimstr("agent:")]|map(select(.=="code" or .=="plan" or .=="test"))|sort|join(" ")')"
expect "settings graph: allows edges (one per work-item state) on $PA" 8 "$(echo "$SG" | jq --arg p "project:$PA" '[.edges[]|select(.type=="allows" and .from==$p)]|length')"
expect "settings graph: placeholders for every stored node type" "account agent agent_tools iter_engine project provider user workitem_type" "$(echo "$SG" | jq -r '[.nodes[]|select(.placeholder)|.type]|unique|join(" ")')"

cat > "$ENVF" <<EOF
ITER_ENGINE_TOKEN=$ENGINE_TOKEN
ACCT_A_TOKEN=mock-token-a
ACCT_A2_TOKEN=mock-token-a2
ACCT_B_TOKEN=mock-token-b
EOF
AS=$(eapi GET "/api/engines/$ENGINE/assignments")
expect "assignments: $ENGINE serves both projects" "$PA $PB" "$(echo "$AS" | jq -r '[.projects[].project]|join(" ")')"
expect "assignments: $PA bills acct_a then acct_a2, provider mock" "acct_a:mock:80:95 acct_a2:mock:80:95" "$(echo "$AS" | jq -r --arg p "$PA" '[.projects[]|select(.project==$p)|.accounts[]|"\(.name):\(.provider):\(.switch):\(.stop)"]|join(" ")')"
expect "assignments: topdir from the serves edge" "$RA" "$(echo "$AS" | jq -r --arg p "$PA" '.projects[]|select(.project==$p)|.topdir')"
api PUT /api/engines/Other '{"host":"x","state":"Stopped","ticksec":5,"user":"admin"}' >/dev/null
expect "assignments: another user's engine is refused" 403 "$(code "$ENGINE_TOKEN" GET /api/engines/Other/assignments)"

engine_start
say "engine $ENGINE started (pid $ENGINE_PID)"

# =====================================================================
section "a0. server-side get_next"
# =====================================================================
if want a0; then
# the probe engine never runs: every claim below is made by hand with the engine token
NX() { eapi POST "/api/projects/$PN/next" "{\"engine\":\"${1:-Probe}\",\"lease_ttl_sec\":120}"; }
X=$(wi_new $PN '{"name":"x p5","agent":"exec","exec_shell":"true","priority":5}')
Y=$(wi_new $PN '{"name":"y p2","agent":"exec","exec_shell":"true","priority":2}')
Z=$(wi_new $PN "{\"name\":\"z p1 after y\",\"agent\":\"exec\",\"exec_shell\":\"true\",\"priority\":1,\"blockedby\":[\"$Y\"]}")
AP=$(wi_new $PN '{"name":"needs approval p0","agent":"exec","exec_shell":"true","priority":0,"needs_approval":true}')
R=$(NX); expect "next #1: priority 2 (the p1 item waits on it, the p0 needs approval)" "$Y" "$(echo "$R" | jq -r .item.id)"
expect "next claims: in-progress, engine, attempt 1, lease" "in-progress Probe 1 true" "$(wi $PN "$Y" | jq -r '"\(.state) \(.engine) \(.attempt) \(.lease!="")"')"
R=$(NX); expect "next #2: priority 5 (dependency still open)" "$X" "$(echo "$R" | jq -r .item.id)"
R=$(NX); expect "next #3: nothing claimable" "null" "$(echo "$R" | jq -r .item)"
expect "next #3: reason blocked" "blocked" "$(echo "$R" | jq -r .reason)"
wi_set $PN "$Y" '.state="complete"'
R=$(NX); expect "next #4: the dependent once its blocker completed" "$Z" "$(echo "$R" | jq -r .item.id)"
L1=$(wi_new $PN '{"name":"lock holder","agent":"exec","exec_shell":"true","priority":3,"lockdirs":["{topdir}/s/"]}')
L2=$(wi_new $PN '{"name":"lock waiter","agent":"exec","exec_shell":"true","priority":4,"lockdirs":["{topdir}/s/x/"]}')
R=$(NX); expect "next #5: the lock holder" "$L1" "$(echo "$R" | jq -r .item.id)"
expect "the claim took its lock row" "$L1" "$(api GET "/api/projects/$PN/locks" | jq -r '[.[]|select(.path=="{topdir}/s/" and .kind!="reserve")][0].workid')"
R=$(NX); expect "next #6: overlapping scope is locked" "null locked" "$(echo "$R" | jq -r '"\(.item) \(.reason)"')"
R=$(NX "$ENGINE"); expect "next for an engine that does not serve the project" "null not-served" "$(echo "$R" | jq -r '"\(.item) \(.reason)"')"
NB="{\"engine\":\"$ENGINE\"}"
expect "engine token on an unserved project: 403" 403 "$(code "$ENGINE_TOKEN" POST "/api/projects/$PU/next" "$NB")"
expect "engine token reading an unserved project's queue: 403" 403 "$(code "$ENGINE_TOKEN" GET "/api/projects/$PU/workitems")"
proj_set $PN '.state="Stopped"'
R=$(NX); expect "next on a Stopped project" "project-stopped" "$(echo "$R" | jq -r .reason)"

# ---- iter4 work-item API behaviours kept ----
W1=$(wi_new $PN '{"name":"api checks","agent":"exec","exec_shell":"true","state":"paused","request":"please do x"}')
expect "request in the create body becomes detail row 0" "request please do x" "$(details $PN "$W1" | jq -r '.[0]|"\(.key) \(.value)"')"
expect "malformed question widget bounced (400)" 400 "$(code "$TOKEN" PUT "/api/projects/$PN/workitems/$W1/details/5" '{"key":"question","valuetype":"json","value":{"title":"","fields":[{"key":"x","type":"nope"}]}}')"
J=$(wi $PN "$W1"); V=$(echo "$J" | jq -r .version)
echo "$J" | jq '.priority=7' | curl -sf -X PUT -H "authorization: Bearer $TOKEN" -H content-type:application/json "$BASE/api/projects/$PN/workitems/$W1?expect_version=$V" -d @- >/dev/null
expect "versioned write: stale expect_version → 409" 409 "$(echo "$J" | jq '.priority=9' | curl -s -o /dev/null -w '%{http_code}' -X PUT -H "authorization: Bearer $TOKEN" -H content-type:application/json "$BASE/api/projects/$PN/workitems/$W1?expect_version=$V" -d @-)"
# (bodies built outside "$(…)": bash 3.2 keeps the \" escapes inside a quoted command substitution)
LB="{\"path\":\"{topdir}/q/\",\"workid\":\"$W1\"}"
expect "lock for an item that is not running: 409" 409 "$(code "$TOKEN" POST "/api/projects/$PN/locks/acquire" "$LB")"
expect "schedules are users-only (engine token 403)" 403 "$(code "$ENGINE_TOKEN" POST "/api/projects/$PN/workitems" '{"name":"rogue schedule","agent":"exec","state":"scheduled","sched":{"kind":"every","every_min":1}}')"
fi

# =====================================================================
section "a. work items on one engine, two projects"
# =====================================================================
if want a; then
WA1=$(wi_new $PA '{"name":"write alpha","agent":"code","priority":3,"lockdirs":["{topdir}/out/"],"request":"mock: write alpha.txt <<<alpha-done>>>\nmock: run echo \"$ITER_ACCOUNT\" > acct_alpha.txt\nmock: say wrote alpha"}')
WA2=$(wi_new $PA "{\"name\":\"alpha after alpha\",\"agent\":\"code\",\"priority\":4,\"lockdirs\":[\"{topdir}/out/\"],\"blockedby\":[\"$WA1\"],\"request\":\"mock: run test -f alpha.txt && echo dep-ok > alpha2.txt\"}")
WB1=$(wi_new $PB '{"name":"write beta","agent":"code","priority":3,"lockdirs":["{topdir}/out/"],"request":"mock: write beta.txt <<<beta-done>>>\nmock: run echo \"$ITER_ACCOUNT\" > acct_beta.txt"}')
WX=$(wi_new $PA '{"name":"exec in alpha","agent":"exec","exec_shell":"echo exec-ok > out_exec.txt","priority":5,"lockdirs":["{topdir}/out_exec.txt"]}')
wait_for 90 "[ \"\$(wi_state $PA $WA2)\" = complete ] && [ \"\$(wi_state $PB $WB1)\" = complete ] && [ \"\$(wi_state $PA $WX)\" = complete ]"
say "round trips done in ${WAITED}s"
for w in "$WA1" "$WA2" "$WX"; do expect "$PA item ${w: -12} complete" complete "$(wi_state $PA "$w")"; done
expect "$PB item ${WB1: -12} complete" complete "$(wi_state $PB "$WB1")"
expect "mock write landed in $PA" alpha-done "$(cat "$RA/out/alpha.txt" 2>/dev/null)"
expect "mock write landed in $PB" beta-done "$(cat "$RB/out/beta.txt" 2>/dev/null)"
expect "dependent ran after its blocker" dep-ok "$(cat "$RA/out/alpha2.txt" 2>/dev/null)"
check "$PA: the work was committed" test -n "$(commits_touching "$RA" out/alpha.txt)"
check "$PB: the work was committed" test -n "$(commits_touching "$RB" out/beta.txt)"
expect "$PA ran on its first account" acct_a "$(cat "$RA/out/acct_alpha.txt" 2>/dev/null)"
expect "$PB ran on its own account" acct_b "$(cat "$RB/out/acct_beta.txt" 2>/dev/null)"
check "response detail row written" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PA/workitems/$WA1/details | jq -e '[.[]|select(.key==\"response\" and (.value|tostring|test(\"wrote alpha\")))]|length>=1'"
check "spend: a spend detail row with tokens" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PA/workitems/$WA1/details | jq -e '[.[]|select(.key==\"spend\" and .value.input_tokens>0)]|length>=1'"
check "spend: project rows recorded" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PA/spend | jq -e 'length>=1'"
expect "locks released in $PA" 0 "$(api GET "/api/projects/$PA/locks" | jq '[.[]|select(.kind!="reserve")]|length')"
EJ=$(api GET "/api/engines/$ENGINE")
check "heartbeat: last_seen set" test -n "$(echo "$EJ" | jq -r '.last_seen // empty')"
expect "heartbeat: engine record owned by engine01" engine01 "$(echo "$EJ" | jq -r .user)"
check "engine created one paused test sweep per project" bash -c "for p in $PA $PB; do curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/\$p/workitems | jq -e '[.[]|select(.system==\"test-sweep\" and .state==\"paused\")]|length==1' || exit 1; done"

# ---- deep / shallow / failed blockers ----
DP=$(wi_new $PA '{"name":"deep plan","agent":"exec","exec_shell":"true","state":"complete","lockdirs":["{topdir}/dp/"]}')
DC=$(wi_new $PA "{\"name\":\"deep child\",\"agent\":\"exec\",\"exec_shell\":\"echo child > out_dchild.txt\",\"createdby\":\"$DP\",\"state\":\"parked\",\"lockdirs\":[\"{topdir}/dc/\"]}")
DD=$(wi_new $PA "{\"name\":\"deep dependent\",\"agent\":\"exec\",\"exec_shell\":\"echo dep > out_ddep.txt\",\"blockedby\":[\"$DP\"],\"lockdirs\":[\"{topdir}/dd/\"]}")
DS=$(wi_new $PA "{\"name\":\"shallow dependent\",\"agent\":\"exec\",\"exec_shell\":\"echo shallow > out_dshallow.txt\",\"blockedby\":[\"$DP\"],\"blockedby_shallow\":true,\"lockdirs\":[\"{topdir}/ds/\"]}")
DF=$(wi_new $PA '{"name":"failed blocker","agent":"exec","exec_shell":"false","state":"failed","lockdirs":["{topdir}/df/"]}')
DG=$(wi_new $PA "{\"name\":\"depends on failed\",\"agent\":\"exec\",\"exec_shell\":\"echo never > out_dnever.txt\",\"blockedby\":[\"$DF\"],\"lockdirs\":[\"{topdir}/dg/\"]}")
wait_for 20 "[ -f '$RA/out_dshallow.txt' ]"; sleep 2
check "shallow dependent ran" test -f "$RA/out_dshallow.txt"
check "deep dependent waited for the blocker's open child" test ! -f "$RA/out_ddep.txt"
check "a failed blocker holds its dependent" test ! -f "$RA/out_dnever.txt"
wi_set $PA "$DC" '.state="queued"'
wait_for 20 "[ -f '$RA/out_ddep.txt' ]"
check "deep dependent released once the child completed" test -f "$RA/out_ddep.txt"
api POST "/api/projects/$PA/workitems/$DF/reopen" '{"reason":"retry"}' >/dev/null
wi_set $PA "$DF" '.exec_shell="true"'
wait_for 20 "[ -f '$RA/out_dnever.txt' ]"
check "retrying the failed blocker released its dependent" test -f "$RA/out_dnever.txt"

# ---- stop mid-run ----
SL=$(wi_new $PA '{"name":"long runner to stop","agent":"code","priority":1,"lockdirs":["{topdir}/stop/"],"request":"mock: sleep 30000\nmock: write never.txt <<<never>>>"}')
wait_for 20 "[ \"\$(wi_state $PA $SL)\" = in-progress ]"
wi_set $PA "$SL" '.stop_requested=true'
wait_for 20 "[ \"\$(wi_state $PA $SL)\" = parked ]"
SJ=$(wi $PA "$SL")
expect "stop mid-run: item parked" parked "$(echo "$SJ" | jq -r .state)"
check "stop mid-run: the rest of the turn never ran" test ! -f "$RA/stop/never.txt"
check "stop mid-run: lasterror says STOPPED" bash -c "echo '$(echo "$SJ" | jq -r .lasterror | tr -d "'")' | grep -q STOPPED"
expect "stop mid-run: flag cleared" false "$(echo "$SJ" | jq -r .stop_requested)"

# ---- retry backoff ----
proj_set $PA '.failure={"maxattempts":3,"first_retry_second":600,"retry_backoff_exponent":2}'
RBK=$(wi_new $PA '{"name":"always fails","agent":"exec","exec_shell":"exit 3","priority":1,"lockdirs":["{topdir}/rb/"]}')
wait_for 20 "[ \"\$(wi $PA $RBK | jq -r .attempt)\" = 1 ] && [ \"\$(wi_state $PA $RBK)\" = queued ]"; sleep 2
RJ=$(wi $PA "$RBK")
expect "backoff: exactly one attempt, back in the queue" "1 queued" "$(echo "$RJ" | jq -r '"\(.attempt) \(.state)"')"
check "backoff: retry_after in the future" test "$(echo "$RJ" | jq -r .retry_after)" \> "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
wi_set $PA "$RBK" '.state="parked"'
proj_set $PA '.failure={"maxattempts":2,"first_retry_second":1,"retry_backoff_exponent":2}'

# ---- run now ----
proj_set $PA '.maxagents={"else":1}'
RL=$(wi_new $PA '{"name":"long occupant","agent":"exec","exec_shell":"sleep 6; echo long > out_long.txt","priority":1,"lockdirs":["{topdir}/ra/"]}')
wait_for 15 "[ \"\$(wi_state $PA $RL)\" = in-progress ]"
RN=$(wi_new $PA '{"name":"run now please","agent":"exec","exec_shell":"echo now > out_now.txt","priority":9,"lockdirs":["{topdir}/rb2/"],"run_now":true}')
RW=$(wi_new $PA '{"name":"ordinary waiter","agent":"exec","exec_shell":"echo wait > out_wait.txt","priority":2,"lockdirs":["{topdir}/rc/"]}')
wait_for 5 "[ -f '$RA/out_now.txt' ]"
check "run now: started past a full cap" test -f "$RA/out_now.txt"
check "run now: ordinary work waited for the slot" test ! -f "$RA/out_wait.txt"
expect "run now: flag consumed on claim" false "$(wi $PA "$RN" | jq -r .run_now)"
wait_for 20 "[ \"\$(wi_state $PA $RW)\" = complete ]"
expect "run now: the waiter ran once the slot freed" complete "$(wi_state $PA "$RW")"
proj_set $PA '.maxagents={"else":3}'

# ---- closed items: immutable, doc appends, reopen ----
expect "closed item: summary PUT refused (403)" 403 "$(wi $PA "$WA1" | jq -c '.priority=1' | curl -s -o /dev/null -w '%{http_code}' -X PUT -H "authorization: Bearer $TOKEN" -H content-type:application/json "$BASE/api/projects/$PA/workitems/$WA1?expect_version=$(wi $PA "$WA1" | jq -r .version)" -d @-)"
expect "closed item: non-doc append refused (403)" 403 "$(code "$TOKEN" POST "/api/projects/$PA/workitems/$WA1/details" '{"key":"response","valuetype":"text","value":"fake"}')"
check "closed item: doc append allowed" api POST "/api/projects/$PA/workitems/$WA1/details" '{"key":"doc","valuetype":"text","value":"closeout note"}'
expect "reopen: engine token refused (403)" 403 "$(code "$ENGINE_TOKEN" POST "/api/projects/$PA/workitems/$WA1/reopen" '{"reason":"rogue"}')"
rm -f "$RA/out/alpha.txt"
api POST "/api/projects/$PA/workitems/$WA1/reopen" '{"reason":"rerun"}' >/dev/null
wait_for 30 "[ -f '$RA/out/alpha.txt' ] && [ \"\$(wi_state $PA $WA1)\" = complete ]"
expect "reopen: ran again and closed" complete "$(wi_state $PA "$WA1")"

# ---- approval (signed) ----
WD=$(wi_new $PA '{"name":"sensitive change","agent":"exec","exec_shell":"echo approved-ran > out_approved.txt","state":"question","needs_approval":true,"lockdirs":["{topdir}/ap/"]}')
mkdir -p "$SCRATCH/keys"
(cd "$SCRATCH/keys" && "$ENGINE_BIN" --data-url "$BASE" --env-file "$ENVF" --adduser steve > "$SCRATCH/adduser.log" 2>&1)
check "--adduser wrote the key and registered the pubkey" test -f "$SCRATCH/keys/.iter/users/steve.pem"
(cd "$SCRATCH/keys" && ITER_APPROVE_KEYPATH="$SCRATCH/keys/.iter/users/steve.pem" "$ENGINE_BIN" --data-url "$BASE" --env-file "$ENVF" --approve "${WD:0:13}" > "$SCRATCH/approve.log" 2>&1)
wait_for 20 "[ -f '$RA/out_approved.txt' ]"
check "signed approval re-queued the item and it ran" test -f "$RA/out_approved.txt"

# ---- viewer role ----
api PUT /api/users/gerald '{"role":"viewer","password":"view-only-pw","email":""}' >/dev/null
api POST /api/settings/edges "{\"from\":\"user:gerald\",\"to\":\"project:$PA\",\"settings\":{\"role\":\"viewer\"}}" >/dev/null
VTOK=$(curl -sf -X POST "$BASE/auth/login" -H content-type:application/json -d '{"user":"gerald","password":"view-only-pw"}' | jq -r .token)
expect "viewer member: reads the queue" 200 "$(code "$VTOK" GET "/api/projects/$PA/workitems")"
expect "viewer member: queue write refused" 403 "$(code "$VTOK" POST "/api/projects/$PA/workitems" '{"name":"viewer write","agent":"exec","exec_shell":"true"}')"
expect "viewer: a project it is no member of is refused" 403 "$(code "$VTOK" GET "/api/projects/$PB/workitems")"

# ---- schedules ----
TPL=$(wi_new $PB '{"name":"sched: heartbeat file","agent":"exec","exec_shell":"echo sched-ran >> out_sched.txt","state":"scheduled","priority":8,"sched":{"kind":"every","every_min":5,"last_fired":"2026-09-01T00:00:00Z"}}')
wait_for 20 "[ -f '$RB/out_sched.txt' ]"
check "schedule fired and its clone ran" test -f "$RB/out_sched.txt"
expect "schedule: exactly one clone" 1 "$(api GET "/api/projects/$PB/workitems" | jq --arg t "$TPL" '[.[]|select(.source_schedule==$t)]|length')"
expect "schedule: template kept" scheduled "$(wi_state $PB "$TPL")"

# ---- close gate: mock: gate incomplete bounces, then a question ----
GI=$(wi_new $PB '{"name":"gate says incomplete","agent":"code","priority":2,"lockdirs":["{topdir}/gate/"],"request":"mock: write try.txt <<<tried>>>\nmock: gate incomplete"}')
wait_for 40 "[ \"\$(wi_state $PB $GI)\" = question ]"
GJ=$(wi $PB "$GI")
check "close gate: the verifier's verdict is recorded" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PB/workitems/$GI/details | jq -e '[.[]|select(.key==\"verify\" and (.value|tostring|test(\"gate incomplete\")))]|length>=1'"
expect "close gate: bounced (max_bounces 1), then a question" "question 2" "$(echo "$GJ" | jq -r '"\(.state) \(.gate_bounces)"')"
# ---- question via mock: ask ----
QA=$(wi_new $PB '{"name":"asks a question","agent":"code","priority":2,"lockdirs":["{topdir}/ask/"],"request":"mock: ask Which greeting should the CLI use?"}')
wait_for 30 "[ \"\$(wi_state $PB $QA)\" = question ]"
expect "mock: ask parks the item in question" question "$(wi_state $PB "$QA")"
check "the question text is on the item" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PB/workitems/$QA/details | jq -e '[.[]|select(.key==\"question\" and (.value|tostring|test(\"Which greeting\")))]|length>=1'"

# ---- usage gating + account switch/stop via the bills edge ----
G1M=$(log_mark)
snap acct_a 85 10; snap acct_a2 5 5
G1=$(wi_new $PA '{"name":"over switch","agent":"code","priority":2,"lockdirs":["{topdir}/g1/"],"request":"mock: run echo \"$ITER_ACCOUNT\" > acct.txt"}')
wait_for 30 "[ \"\$(wi_state $PA $G1)\" = complete ]"
expect "acct_a over its switch% → acct_a2 billed" acct_a2 "$(cat "$RA/g1/acct.txt" 2>/dev/null)"
api PATCH "/api/settings/edges/$BILL_A" '{"settings":{"switch":90}}' >/dev/null
snap acct_a 85 10; snap acct_a2 5 5
G2=$(wi_new $PA '{"name":"switch raised","agent":"code","priority":2,"lockdirs":["{topdir}/g2/"],"request":"mock: run echo \"$ITER_ACCOUNT\" > acct.txt"}')
wait_for 30 "[ \"\$(wi_state $PA $G2)\" = complete ]"
expect "bills edge switch raised to 90 → acct_a billed again" acct_a "$(cat "$RA/g2/acct.txt" 2>/dev/null)"
snap acct_a 97 10; snap acct_a2 97 5
G3=$(wi_new $PA '{"name":"all at stop","agent":"code","priority":2,"lockdirs":["{topdir}/g3/"],"request":"mock: run echo \"$ITER_ACCOUNT\" > acct.txt"}')
GB=$(wi_new $PB '{"name":"beta keeps going","agent":"exec","exec_shell":"echo beta-ran > out_beta_hold.txt","priority":2,"lockdirs":["{topdir}/bh/"]}')
wait_for 20 "[ -f '$RB/out_beta_hold.txt' ]"; sleep 3
expect "every account at stop%: $PA holds" queued "$(wi_state $PA "$G3")"
check "the other project is not held" test -f "$RB/out_beta_hold.txt"
check "engine log names the hold" bash -c "tail -n +$((G1M+1)) '$ENGINE_LOG' | grep -q '$PA: all accounts at stop%'"
api PATCH "/api/settings/edges/$BILL_A2" '{"settings":{"stop":99}}' >/dev/null
wait_for 30 "[ \"\$(wi_state $PA $G3)\" = complete ]"
expect "bills edge stop raised to 99 → acct_a2 billed past its switch" acct_a2 "$(cat "$RA/g3/acct.txt" 2>/dev/null)"
api PATCH "/api/settings/edges/$BILL_A2" '{"settings":{"stop":95}}' >/dev/null
api PATCH "/api/settings/edges/$BILL_A" '{"settings":{"switch":80}}' >/dev/null
snap acct_a 0 0; snap acct_a2 0 0

# ---- daily budget ----
proj_set $PB '.maxdailycost=0'
BC=$(wi_new $PB '{"name":"budget blocked","agent":"exec","exec_shell":"echo b > out_budget.txt","priority":0,"lockdirs":["{topdir}/bud/"]}')
sleep 4
check "maxdailycost=0 holds all picks" test ! -f "$RB/out_budget.txt"
proj_set $PB 'del(.maxdailycost)'
wait_for 20 "[ -f '$RB/out_budget.txt' ]"
check "budget removed: the item ran" test -f "$RB/out_budget.txt"

# ---- Draining settles to Stopped ----
proj_set $PB '.state="Draining"'
DR=$(wi_new $PB '{"name":"work during drain","agent":"exec","exec_shell":"echo drained > out_drain.txt","priority":1}')
wait_for 15 "[ \"\$(api GET /api/projects/$PB | jq -r .state)\" = Stopped ]"
expect "Draining settled to Stopped" Stopped "$(api GET "/api/projects/$PB" | jq -r .state)"
check "no new picks while draining/stopped" test ! -f "$RB/out_drain.txt"
proj_set $PB '.state="Running"'
wait_for 20 "[ -f '$RB/out_drain.txt' ]"
check "Running again: the held item ran" test -f "$RB/out_drain.txt"

# ---- deactivate the serves edge (move the engine end onto the placeholder) ----
M=$(log_mark)
api PATCH "/api/settings/edges/$SERVE_B" '{"from":"iter_engine:_deactivated"}' >/dev/null
expect "serves edge on the placeholder is inactive" false "$(api GET "/api/settings/edges/$SERVE_B" | jq -r .active)"
expect "assignments drop $PB" "$PA" "$(eapi GET "/api/engines/$ENGINE/assignments" | jq -r '[.projects[].project]|join(" ")')"
SV=$(wi_new $PB '{"name":"while unserved","agent":"exec","exec_shell":"echo served-again > out_unserved.txt","priority":1}')
wait_for 10 "log_since $M | grep -q 'serving 1 project'"; sleep 3
check "engine noticed: serving 1 project" bash -c "tail -n +$((M+1)) '$ENGINE_LOG' | grep -q 'serving 1 project'"
expect "deactivated: $PB not run" queued "$(wi_state $PB "$SV")"
expect "deactivated: engine token refused on $PB" 403 "$(code "$ENGINE_TOKEN" GET "/api/projects/$PB/workitems")"
api PATCH "/api/settings/edges/$SERVE_B" "{\"from\":\"iter_engine:$ENGINE\"}" >/dev/null
expect "reactivated: edge active with its settings kept" "true $RB" "$(api GET "/api/settings/edges/$SERVE_B" | jq -r '"\(.active) \(.settings.topdir)"')"
wait_for 20 "[ -f '$RB/out_unserved.txt' ]"
check "reactivated: $PB runs again" test -f "$RB/out_unserved.txt"
fi

# =====================================================================
section "b. file → node"
# =====================================================================
if want b; then
LIB=5a000000-0000-4000-8000-00000000000a
# the first sync may still be running when earlier sections are skipped
wait_for 30 "[ \"\$(gnode_f $PA $LIB .desc)\" = 'greet.sh: builds the greeting text.' ] && [ \"\$(graph $PA | jq -r '[.nodes[]|select(.nodetype==\"project\")][0].id')\" = 5a000000-0000-4000-8000-000000000001 ] && clean_in_git $RA src/greeter/lib/lib.code.iter.md"
check "repo nodes synced: lib node present with its fixture desc" test "$(gnode_f $PA $LIB .desc)" = "greet.sh: builds the greeting text."
expect "repo's project node replaced the server seeds" "1 5a000000-0000-4000-8000-000000000001" "$(graph $PA | jq -r '[.nodes[]|select(.nodetype=="project")]|"\(length) \(.[0].id)"')"
GJ=$(graph $PA)
check "edges derived: connection supplies/connects" bash -c "echo '$(echo "$GJ" | jq -c '[.edges[]|.kind]')' | jq -e 'index(\"supplies\") and index(\"connects\") and index(\"drives\") and index(\"touches\") and index(\"uses\") and index(\"tests\") and index(\"reqs\") and index(\"codenodes\")'"
sed -i.bak 's/^desc: "greet.sh: builds the greeting text."/desc: "edited in the repo"/' "$RA/src/greeter/lib/lib.code.iter.md" && rm -f "$RA/src/greeter/lib/lib.code.iter.md.bak"
wait_for 20 "[ \"\$(gnode_f $PA $LIB .desc)\" = 'edited in the repo' ]"
expect "file edit → node desc (latency ${WAITED}s)" "edited in the repo" "$(gnode_f $PA $LIB .desc)"
say "file → node latency: ${WAITED}s"
wait_for 10 "commits_touching $RA src/greeter/lib/lib.code.iter.md | grep -q '^iter: '"
check "the hand edit's conform rewrite was committed" bash -c "git -C '$RA' log --format=%s -- src/greeter/lib/lib.code.iter.md | grep -q '^iter: '"
check "last_modified bumped on the hand edit" bash -c "grep -q 'last_modified: \"2026-10-02 12:00:00Z\"' '$RA/src/greeter/lib/lib.code.iter.md' && exit 1 || exit 0"
NEWID=5a000000-0000-4000-8000-0000000000e1
mkdir -p "$RA/src/greeter/extra"
cat > "$RA/src/greeter/extra/extra.code.iter.md" <<EOF
---
id: $NEWID
name: "Extra"
desc: "a node added by hand"
creator: "e2e"
teststate: inherit
level: component
children:
  codedirs:  ["{thisfiledir}/**"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 12:00:00Z", last_modified: "2026-10-02 12:00:00Z", last_tested: ""}
---
Extra.
EOF
wait_for 20 "gnode $PA $NEWID"
expect "new file → node (latency ${WAITED}s)" "{topdir}/src/greeter/extra/extra.code.iter.md" "$(gnode_f $PA $NEWID .path)"
rm -f "$RA/src/greeter/extra/extra.code.iter.md"
wait_for 20 "! graph $PA | jq -e --arg i $NEWID '.nodes[]|select(.id==\$i)'"
check "deleted file → node gone (latency ${WAITED}s)" bash -c "! curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PA/graph | jq -e --arg i $NEWID '.nodes[]|select(.id==\$i)'"
mkdir -p "$RA/src/greeter/messy"
printf -- '---\nname: "Messy"\nlevel: component\ndescription: "legacy key"\n---\n# Messy\n' > "$RA/src/greeter/messy/messy.code.iter.md"
wait_for 20 "graph $PA | jq -e '.nodes[]|select(.name==\"Messy\")'"
MJ=$(graph $PA | jq -c '[.nodes[]|select(.name=="Messy")][0]')
expect "messy file → node (legacy description → desc)" "legacy key" "$(echo "$MJ" | jq -r .desc)"
wait_for 15 "clean_in_git $RA src/greeter/messy/messy.code.iter.md"
MF="$RA/src/greeter/messy/messy.code.iter.md"
check "messy file rewritten conformed: id" grep -q "^id: $(echo "$MJ" | jq -r .id)" "$MF"
check "messy file rewritten conformed: desc/creator/teststate/children/timestamps" bash -c "grep -q '^desc: \"legacy key\"' '$MF' && grep -q '^teststate: inherit' '$MF' && grep -q '^creator: ' '$MF' && grep -q '^children:' '$MF' && grep -q '^timestamps: ' '$MF' && ! grep -q '^description:' '$MF'"
check "messy file: conformed text committed (tracked, clean)" clean_in_git "$RA" src/greeter/messy/messy.code.iter.md
fi

# =====================================================================
section "c. node → file"
# =====================================================================
if want c; then
LIBF="$RA/src/greeter/lib/lib.code.iter.md"
api PATCH "/api/projects/$PA/graph/nodes/$LIB" '{"desc":"patched via the api"}' >/dev/null
expect "PATCH is visible in the graph at once" "patched via the api" "$(gnode_f $PA $LIB .desc)"
wait_for 20 "grep -q '^desc: \"patched via the api\"' '$LIBF'"
check "node edit → file (latency ${WAITED}s)" grep -q '^desc: "patched via the api"' "$LIBF"
wait_for 10 "[ \"\$(gnode_f $PA $LIB .file_state)\" = synced ] && clean_in_git $RA src/greeter/lib/lib.code.iter.md"
check "node edit committed by the engine (iter: graph edit)" bash -c "git -C '$RA' log -1 --format=%s -- src/greeter/lib/lib.code.iter.md | grep -q '^iter: graph edit'"
expect "node back to synced after the ack" synced "$(gnode_f $PA $LIB .file_state)"
GREETER=5a000000-0000-4000-8000-000000000008
CR=$(api POST "/api/projects/$PA/graph/nodes" "{\"nodetype\":\"code\",\"level\":\"component\",\"name\":\"Formatter\",\"desc\":\"formats greetings\",\"attach_to\":\"$GREETER\"}")
FID=$(echo "$CR" | jq -r .node.id)
expect "create node: planned path (§2.5)" "{topdir}/src/greeter/formatter/formatter.code.iter.md" "$(echo "$CR" | jq -r .node.path)"
FF="$RA/src/greeter/formatter/formatter.code.iter.md"
wait_for 20 "[ -f '$FF' ] && clean_in_git $RA src/greeter/formatter/formatter.code.iter.md"
check "create node → file written + committed" clean_in_git "$RA" src/greeter/formatter/formatter.code.iter.md
check "file carries the node id" grep -q "^id: $FID" "$FF"
wait_for 10 "grep -q 'formatter/formatter.code.iter.md' '$RA/src/greeter/greeter.code.iter.md'"
check "the parent's file lists the new child" grep -q 'formatter/formatter.code.iter.md' "$RA/src/greeter/greeter.code.iter.md"
UC=5a000000-0000-4000-8000-000000000005
api POST "/api/projects/$PA/graph/edges" "{\"from\":\"$UC\",\"to\":\"$FID\",\"kind\":\"uses\"}" >/dev/null
check "add edge: visible at once" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PA/graph | jq -e --arg f $FID '.edges[]|select(.from==\"$UC\" and .kind==\"uses\" and .to==\$f)'"
wait_for 20 "grep -q 'formatter/formatter.code.iter.md' '$RA/global/usecases/greet_someone.usecase.iter.md'"
check "add edge → owning node's children list in the file" grep -q 'formatter/formatter.code.iter.md' "$RA/global/usecases/greet_someone.usecase.iter.md"
curl -sf -X DELETE -H "authorization: Bearer $TOKEN" -H content-type:application/json "$BASE/api/projects/$PA/graph/nodes/$FID" -d '{"reason":"e2e delete"}' >/dev/null
wait_for 20 "[ ! -f '$FF' ] && ! grep -q 'formatter/formatter.code.iter.md' '$RA/src/greeter/greeter.code.iter.md'"
check "delete node → file removed" test ! -f "$FF"
check "delete node → removed from its parents' files" bash -c "! grep -q 'formatter/formatter.code.iter.md' '$RA/src/greeter/greeter.code.iter.md' '$RA/global/usecases/greet_someone.usecase.iter.md'"
wait_for 10 "[ -z \"\$(git -C $RA status --porcelain -- src/greeter)\" ]"
check "delete committed" bash -c "git -C '$RA' log --diff-filter=D --format=%s -- src/greeter/formatter/formatter.code.iter.md | grep -q '^iter: '"
# conflict: both sides edited while the engine was down — the newer edit wins
engine_stop
CLI=5a000000-0000-4000-8000-000000000009; CLIF="$RA/src/greeter/cli/cli.code.iter.md"
api PATCH "/api/projects/$PA/graph/nodes/$CLI" '{"desc":"server side (older)"}' >/dev/null
sleep 1.2
sed -i.bak 's/^desc: .*/desc: "file side (newer)"/' "$CLIF" && rm -f "$CLIF.bak"
CAT=$(gnode_f $PB 5a000000-0000-4000-8000-00000000000a .name) # keep the alpha/beta split honest: beta untouched
LIBB="$RB/src/greeter/lib/lib.code.iter.md"
sed -i.bak 's/^desc: .*/desc: "beta file side (older)"/' "$LIBB" && rm -f "$LIBB.bak"
sleep 1.2
api PATCH "/api/projects/$PB/graph/nodes/5a000000-0000-4000-8000-00000000000a" '{"desc":"beta server side (newer)"}' >/dev/null
engine_start
wait_for 30 "[ \"\$(gnode_f $PA $CLI .desc)\" = 'file side (newer)' ] && [ \"\$(gnode_f $PA $CLI .file_state)\" = synced ]"
expect "conflict, file newer: the file wins" "file side (newer)" "$(gnode_f $PA $CLI .desc)"
check "conflict, file newer: file kept" grep -q '^desc: "file side (newer)"' "$CLIF"
wait_for 30 "grep -q '^desc: \"beta server side (newer)\"' '$LIBB'"
expect "conflict, node newer: the node wins" "beta server side (newer)" "$(gnode_f $PB 5a000000-0000-4000-8000-00000000000a .desc)"
check "conflict, node newer: file rewritten with the node's text" grep -q '^desc: "beta server side (newer)"' "$LIBB"
check "conflicts recorded (graph/conflicts)" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PA/graph/conflicts | jq -e '[.. | objects | select(.id? == \"$CLI\")] | length >= 1'"
fi

# =====================================================================
section "d. designer → build"
# =====================================================================
if want d; then
api PUT "/api/projects/$PD" '{"desc":"designed in the graph","state":"Stopped","gitrepo":""}' >/dev/null
DG=$(graph $PD)
expect "designed project: project node + 3 default reqs, all designed" "4 designed" "$(echo "$DG" | jq -r '"\(.nodes|length) \([.nodes[].file_state]|unique|join(","))"')"
PDID=$(echo "$DG" | jq -r '.nodes[]|select(.nodetype=="project")|.id')
C1=$(api POST "/api/projects/$PD/graph/nodes" '{"nodetype":"code","level":"context","name":"Billing","desc":"takes the money"}' | jq -r .node.id)
C2=$(api POST "/api/projects/$PD/graph/nodes" "{\"nodetype\":\"code\",\"level\":\"container\",\"name\":\"Invoice API\",\"attach_to\":\"$C1\"}" | jq -r .node.id)
T1=$(api POST "/api/projects/$PD/graph/nodes" "{\"nodetype\":\"test\",\"name\":\"Invoice tests\",\"attach_to\":\"$C2\"}" | jq -r .node.id)
CN=$(api POST "/api/projects/$PD/graph/nodes" '{"nodetype":"code","level":"connection","name":"HTTP API call"}' | jq -r .node.id)
api POST "/api/projects/$PD/graph/edges" "{\"from\":\"$C1\",\"to\":\"$CN\",\"kind\":\"supplies\"}" >/dev/null
api POST "/api/projects/$PD/graph/edges" "{\"from\":\"$CN\",\"to\":\"$C2\",\"kind\":\"connects\"}" >/dev/null
expect "designed nodes stay designed (no repo yet)" "designed" "$(graph $PD | jq -r '[.nodes[].file_state]|unique|join(",")')"
check "a project without an engine is not served" bash -c "! curl -sf -H 'authorization: Bearer $ENGINE_TOKEN' $BASE/api/engines/$ENGINE/assignments | jq -e '.projects[]|select(.project==\"$PD\")'"
api POST /api/settings/edges "{\"from\":\"account:acct_b\",\"to\":\"project:$PD\",\"settings\":{\"order\":1,\"switch\":80,\"stop\":95}}" >/dev/null
RD="$SCRATCH/designed_repo"
api POST "/api/projects/$PD/build" "{\"engine\":\"$ENGINE\",\"topdir\":\"$RD\",\"queue_plan\":true}" >/dev/null || ko "build request"
check "build: serves edge created" bash -c "curl -sf -H 'authorization: Bearer $ENGINE_TOKEN' $BASE/api/engines/$ENGINE/assignments | jq -e '.projects[]|select(.project==\"$PD\" and .topdir==\"$RD\")'"
wait_for 40 "[ \"\$(api GET /api/projects/$PD/build | jq -r '.state // .build.state')\" = done ]"
say "build done in ${WAITED}s"
expect "build: state done" done "$(api GET "/api/projects/$PD/build" | jq -r '.state // .build.state')"
check "build: repo created with git" test -d "$RD/.git"
check "build: commit 'iter: build from design'" bash -c "git -C '$RD' log --format=%s | grep -q '^iter: build from design'"
check "build: .gitignore keeps .iter/ out" grep -q '^\.iter/' "$RD/.gitignore"
DG=$(graph $PD)
MISSING=$(echo "$DG" | jq -r '.nodes[].path' | sed "s#{topdir}#$RD#" | while read -r p; do [ -f "$p" ] || echo "$p"; done)
check "build: every node has its file at its path" test -z "$MISSING"
expect "build: every node synced" synced "$(echo "$DG" | jq -r '[.nodes[].file_state]|unique|join(",")')"
check "build: files carry their node ids" bash -c "grep -q '^id: $C2' '$RD/src/billing/invoice_api/invoice_api.code.iter.md'"
check "build: connection file lists supplier and target" bash -c "f=\$(grep -l '^id: $CN' -r '$RD/global'); grep -q 'billing.code.iter.md' \"\$f\" && grep -q 'invoice_api.code.iter.md' \"\$f\""
expect "build: tree clean after the build commit" "" "$(git -C "$RD" status --porcelain | grep -v '^?? .iter' )"
check "build: plan item queued at priority 5" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PD/workitems | jq -e '[.[]|select(.agent==\"plan\" and .priority==5 and .state==\"queued\")]|length==1'"
fi

# =====================================================================
section "e. tests: standard JSON"
# =====================================================================
if want e; then
TN=5a000000-0000-4000-8000-00000000000b
RT=$(api POST "/api/projects/$PA/graph/run_tests" "{\"node\":\"$TN\"}" | jq -r .id)
check "run_tests filed a test work item" test -n "$RT" -a "$RT" != null
wait_for 40 "[ \"\$(wi_state $PA $RT)\" = complete ]"
expect "test work item ran" complete "$(wi_state $PA "$RT")"
wait_for 10 "api GET '/api/projects/$PA/testlogs?node=$TN' | jq -e '.logs|length>=1'"
TL=$(api GET "/api/projects/$PA/testlogs?node=$TN&limit=1")
expect "testlog row: pass, normal 2/2" "pass 2 2" "$(echo "$TL" | jq -r '.logs[0]|"\(.outcome) \(.normal.total) \(.normal.pass)"')"
expect "test node carries the result" "green 2" "$(gnode $PA $TN | jq -r '"\(.test.result) \(.last_result.normal.pass // .front.last_result.normal.pass)"')"
wait_for 20 "grep -q 'last_result' '$RA/src/greeter/greeter.test.iter.md'"
check "test node file got last_result + last_tested" bash -c "grep -q 'last_result' '$RA/src/greeter/greeter.test.iter.md' && ! grep -q 'last_tested: \"\"' '$RA/src/greeter/greeter.test.iter.md'"
# break the library: a red run files a code work item
echo 'echo "Bye, ${1:-World}"' > "$RA/src/greeter/lib/greet.sh"
git -C "$RA" commit -qam "e2e: break the greeting"
RT2=$(api POST "/api/projects/$PA/graph/run_tests" "{\"node\":\"$TN\"}" | jq -r .id)
wait_for 40 "[ \"\$(api GET '/api/projects/$PA/testlogs?node=$TN&limit=1' | jq -r '.logs[0].outcome')\" = fail ]"
expect "failing script → fail logged" fail "$(api GET "/api/projects/$PA/testlogs?node=$TN&limit=1" | jq -r '.logs[0].outcome')"
check "failing details kept" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' '$BASE/api/projects/$PA/testlogs?node=$TN&limit=1' | jq -e '.logs[0].details|map(select(.pass==false))|length==2'"
expect "test node shows red" red "$(gnode $PA $TN | jq -r .test.result)"
check "a code work item was filed for the red test" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PA/workitems | jq -e '[.[]|select(.agent==\"code\" and (.tags|map(.text)|index(\"check:tests-failing\")))]|length==1'"
echo 'echo "Hello, ${1:-World}"' > "$RA/src/greeter/lib/greet.sh"
git -C "$RA" commit -qam "e2e: fix the greeting"
fi

# =====================================================================
section "f. MCP"
# =====================================================================
if want f; then
mcp() { # mcp <token> <project> <json-rpc body>
  curl -s -X POST "$BASE/mcp" -H "authorization: Bearer $1" -H content-type:application/json -H "X-Iter-Project: $2" -d "$3"; }
tool() { mcp "$ENGINE_TOKEN" "$PA" "{\"jsonrpc\":\"2.0\",\"id\":$RANDOM,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}"; }
tool_json() { tool "$1" "$2" | jq -r '.result.content[0].text' | jq -c . 2>/dev/null; }
INIT=$(mcp "$ENGINE_TOKEN" "$PA" '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e","version":"1"}}}')
expect "initialize: server info" iter_data "$(echo "$INIT" | jq -r .result.serverInfo.name)"
TL=$(mcp "$ENGINE_TOKEN" "$PA" '{"jsonrpc":"2.0","id":2,"method":"tools/list"}')
check "tools/list: graph_node_create, graph_node_update, graph_edge_add, workitem_create, status, testlogs" bash -c "echo '$(echo "$TL" | jq -c '[.result.tools[].name]')' | jq -e 'index(\"graph_node_create\") and index(\"graph_node_update\") and index(\"graph_edge_add\") and index(\"workitem_create\") and index(\"status\") and index(\"testlogs\")'"
check "tools/list: no interface tools" bash -c "! echo '$(echo "$TL" | jq -c '[.result.tools[].name]')' | grep -qi interface"
PROJID=5a000000-0000-4000-8000-000000000001
# (a bizreq/techreq is one file per attachment point (§2.8): the project's
# techreq exists already, so a new node file here is a philosophy)
MC=$(tool_json graph_node_create "{\"nodetype\":\"philosophy\",\"name\":\"MCP rule\",\"desc\":\"added over MCP\",\"attach_to\":\"$PROJID\",\"attach_kind\":\"reqs\"}")
MID=$(echo "$MC" | jq -r .node.id)
expect "graph_node_create: global req path" "{topdir}/global/requirements/mcp_rule.philosophy.iter.md" "$(echo "$MC" | jq -r .node.path)"
tool_json graph_node_update "{\"id\":\"$MID\",\"desc\":\"updated over MCP\"}" >/dev/null
expect "graph_node_update applied" "updated over MCP" "$(gnode_f $PA "$MID" .desc)"
tool_json graph_edge_add "{\"from\":\"$LIB\",\"to\":\"$MID\",\"kind\":\"reqs\"}" >/dev/null
check "graph_edge_add applied" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PA/graph | jq -e '.edges[]|select(.from==\"$LIB\" and .kind==\"reqs\" and .to==\"$MID\")'"
wait_for 20 "grep -q '^desc: \"updated over MCP\"' '$RA/global/requirements/mcp_rule.philosophy.iter.md'"
check "MCP node → file in the repo" grep -q '^desc: "updated over MCP"' "$RA/global/requirements/mcp_rule.philosophy.iter.md"
# a requirement (one ## section) over MCP: node + type → the project's existing techreq file
MR=$(tool_json req_create "{\"node\":\"$PROJID\",\"type\":\"techreq\",\"key\":\"E2E-1\",\"title\":\"Added over MCP\",\"text\":\"One section, one req node.\"}")
MRID=$(echo "$MR" | jq -r .req.id)
expect "req_create: into the existing techreq file" "5a000000-0000-4000-8000-000000000004 contains" "$(echo "$MR" | jq -r '"\(.req.file) \(if .req.nodetype=="req" then "contains" else "?" end)"')"
check "req node + contains edge in the graph" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PA/graph | jq -e '.edges[]|select(.from==\"5a000000-0000-4000-8000-000000000004\" and .kind==\"contains\" and .to==\"$MRID\")'"
wait_for 20 "grep -q '^<!-- req: id=$MRID status=draft -->' '$RA/global/requirements/shell.techreq.iter.md'"
check "req → its ## section in the repo file" bash -c "grep -q '^## E2E-1 — Added over MCP' '$RA/global/requirements/shell.techreq.iter.md' && grep -q '^<!-- req: id=$MRID status=draft -->' '$RA/global/requirements/shell.techreq.iter.md'"
MW=$(tool_json workitem_create '{"title":"from mcp","agent":"code","request":"mock: write mcp.txt <<<mcp-done>>>","codepaths":["{topdir}/out/"],"priority":3}')
MWID=$(echo "$MW" | jq -r '.id // .item.id')
check "workitem_create filed an item" test -n "$MWID" -a "$MWID" != null
wait_for 30 "[ \"\$(cat '$RA/out/mcp.txt' 2>/dev/null)\" = mcp-done ]"
expect "the MCP item ran on the engine" mcp-done "$(cat "$RA/out/mcp.txt" 2>/dev/null)"
check "status tool answers" bash -c "[ \"\$(curl -s -X POST $BASE/mcp -H 'authorization: Bearer $ENGINE_TOKEN' -H content-type:application/json -H 'X-Iter-Project: $PA' -d '{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/call\",\"params\":{\"name\":\"status\",\"arguments\":{}}}' | jq -r '.result.isError // false')\" = false ]"
REF=$(mcp "$ENGINE_TOKEN" "$PU" '{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"status","arguments":{}}}')
check "engine token on an unserved project: refused" bash -c "echo '$(echo "$REF" | jq -c . | tr -d "'")' | jq -e '(.error != null) or (.result.isError == true)'"
check "the refusal names the authz rule" bash -c "echo '$(echo "$REF" | jq -c . | tr -d "'")' | grep -qi 'forbidden\|serves\|403'"
fi

# =====================================================================
section "g. iter migrate5"
# =====================================================================
if want g; then
F4="$SCRATCH/iter4_src"; F5="$SCRATCH/iter4_to5"
cp -R "$REPO/e2e/iter4_fixture" "$F4"
find "$F4" -name '*.iter.md.fixture' | while read -r f; do mv "$f" "${f%.fixture}"; done
git -C "$F4" init -q && git -C "$F4" add -A && git -C "$F4" -c user.email=e2e@iter -c user.name=e2e commit -qm iter4
BEFORE=$(cd "$F4" && find . -type f -not -path './.git/*' -exec shasum {} + | sort)
"$ENGINE_BIN" cli migrate5 --from "$F4" --to "$F5" --json > "$SCRATCH/migrate5.json" 2> "$SCRATCH/migrate5.err"
MRC=$?
expect "migrate5 exit code" 0 "$MRC"
check "migrate5: --from untouched" test "$BEFORE" = "$(cd "$F4" && find . -type f -not -path './.git/*' -exec shasum {} + | sort)"
check "migrate5: project file" test -f "$F5/global/demo_shop.project.iter.md"
check "migrate5: main.iter.md gone" test ! -f "$F5/main.iter.md"
check "migrate5: no interfaces left" test -z "$(find "$F5" -name '*.interface.iter.md' -not -path '*/.git/*')"
check "migrate5: connection nodes per interface kind" bash -c "grep -l '^level: connection' -r '$F5/global/connections' | wc -l | grep -q 2"
check "migrate5: testgroup → test node" test -f "$F5/api/tests/api.test.iter.md"
check "migrate5: global bizreq → one file, one ## section per bullet (§2.8)" bash -c "[ \$(grep -c '^<!-- req: id=[0-9a-f-]\\{36\\} status=agreed -->' '$F5/global/requirements/demo_shop.bizreq.iter.md') -eq 2 ] && grep -q '^## SHOP-B001 — ' '$F5/global/requirements/demo_shop.bizreq.iter.md'"
check "migrate5: original bizreq moved, project names the new file" bash -c "[ ! -f '$F5/reqs/bizreq.iter.md' ] && grep -q 'global/requirements/demo_shop.bizreq.iter.md' '$F5'/global/demo_shop.project.iter.md"
check "migrate5: actor from actors.yaml" test -f "$F5/global/usecases/buyer.actor.iter.md"
check "migrate5: agentmemory → agentmem" test -f "$F5/web/web.agentmem.iter.md"
NC=$(find "$F5" -name '*.iter.md' -not -path '*/.git/*' -not -name '*.agentmem.iter.md' | while read -r f; do head -1 "$f" | grep -q '^---$' && grep -q '^id: ' "$f" && grep -q '^desc: ' "$f" && grep -q '^children:' "$f" && grep -q '^timestamps: ' "$f" || echo "$f"; done)
check "migrate5: every node file conforming (id, desc, children, timestamps)" test -z "$NC"
check "migrate5: no interface keys or links left (inputs/outputs/*.interface.iter.md)" bash -c "! grep -rqE '^ *(inputs|outputs|interfaces?):|interface\.iter\.md' --include='*.iter.md' '$F5'"
check "migrate5: connects.from/to filled from the old producers/consumers" bash -c "grep -A2 '^connects:' '$F5/global/connections/api_call.code.iter.md' | grep -q 'api/api.code.iter.md' && grep -A3 '^connects:' '$F5/global/connections/api_call.code.iter.md' | grep -q 'web/web.code.iter.md'"
check "migrate5: report says not_idempotent is empty" bash -c "jq -e '(.not_idempotent // [])|length==0' '$SCRATCH/migrate5.json'"
fi

# =====================================================================
section "h. live smoke (--live)"
# =====================================================================
if want h; then
if [ "$LIVE" = 1 ] && [ -n "${ITER_LIVE_CLAUDE_TOKEN:-}" ]; then
  PL=e2e5_live; RL5="$SCRATCH/live"; mkrepo "$RL5"
  api PUT "/api/projects/$PL" '{"desc":"live smoke","state":"Running","gitrepo":"","maxagents":{"else":1}}' >/dev/null
  api POST /api/settings/nodes '{"type":"account","name":"live_claude","settings":{"provider":"claude","token_envar":"ITER_LIVE_CLAUDE_TOKEN"}}' >/dev/null
  edge "{\"from\":\"iter_engine:$ENGINE\",\"to\":\"account:live_claude\"}" >/dev/null
  edge "{\"from\":\"account:live_claude\",\"to\":\"project:$PL\",\"settings\":{\"order\":1,\"switch\":90,\"stop\":98}}" >/dev/null
  echo "ITER_LIVE_CLAUDE_TOKEN=$ITER_LIVE_CLAUDE_TOKEN" >> "$ENVF"
  edge "{\"from\":\"iter_engine:$ENGINE\",\"to\":\"project:$PL\",\"settings\":{\"topdir\":\"$RL5\"}}" >/dev/null
  LW=$(wi_new $PL '{"name":"live pong","agent":"code","priority":1,"lockdirs":["{topdir}/live/"],"request":"Reply with the single word pong. Do not change any files."}')
  wait_for 300 "[ \"\$(wi_state $PL $LW)\" = complete ]"
  expect "live claude item completed" complete "$(wi_state $PL "$LW")"
  check "live response recorded" bash -c "curl -sf -H 'authorization: Bearer $TOKEN' $BASE/api/projects/$PL/workitems/$LW/details | jq -e '[.[]|select(.key==\"response\")]|length>=1'"
else
  say "skipped (pass --live with ITER_LIVE_CLAUDE_TOKEN set to run it)"
fi
fi

engine_stop
grep -q "panicked" "$ENGINE_LOG" && ko "the engine log shows a panic: $(grep -m1 panicked "$ENGINE_LOG")"
tally
[ "$NFAIL" = 0 ]
