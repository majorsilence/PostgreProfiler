#!/usr/bin/env bash
# Smoke test of the web UI against the running demo stack
#   docker compose -f docker-compose.yml -f docker-compose.demo.yml up --build -d
# Checks login, the read endpoints, permissions, the admin actions and the live
# stream. Needs only curl.
set -euo pipefail
base="${UI_URL:-http://localhost:8080}"
jar_viewer=$(mktemp); jar_admin=$(mktemp)
trap 'rm -f "$jar_viewer" "$jar_admin"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
H='X-Profiler-UI: 1'

# wait for the UI to answer
for i in $(seq 1 60); do [ "$(code "$base/healthz" || true)" = "200" ] && break; sleep 2; done
[ "$(code "$base/healthz")" = "200" ] || fail "the UI does not answer on $base"

# the app itself and the security headers
curl -s "$base/" | grep -q '<div id="app">' || fail "the single-page app is not served"
curl -sI "$base/" | tr -d '\r' | grep -qi '^content-security-policy:' || fail "no Content-Security-Policy header"
[ "$(code "$base/some/client/route")" = "200" ] || fail "client routes do not fall back to the app"
[ "$(code "$base/api/nonexistent")" = "404" ] || fail "unknown API paths should be a 404, not the app"

# login is required, wrong passwords are refused
[ "$(code "$base/api/me")" = "401" ] || fail "/api/me should need a login"
[ "$(code -X POST -H "$H" -H 'Content-Type: application/json' -d '{"user":"viewer","password":"nope"}' "$base/api/login")" = "401" ] || fail "a wrong password was accepted"
[ "$(code -X POST -H 'Content-Type: application/json' -d '{"user":"viewer","password":"viewer"}' "$base/api/login")" = "403" ] || fail "a POST without X-Profiler-UI was accepted"

login() { # jar user password
  [ "$(code -c "$1" -X POST -H "$H" -H 'Content-Type: application/json' -d "{\"user\":\"$2\",\"password\":\"$3\"}" "$base/api/login")" = "200" ] || fail "login as $2 failed"
  grep -q 'HttpOnly' "$1" || fail "the session cookie is not HttpOnly"
}
login "$jar_viewer" viewer viewer
login "$jar_admin" admin admin
get() { curl -s -b "$1" "$base$2"; }

grep -q '"role":"viewer"' <<<"$(get "$jar_viewer" /api/me)" || fail "viewer role not reported"
grep -q '"role":"admin"' <<<"$(get "$jar_admin" /api/me)" || fail "admin role not reported"

# read endpoints return what the extension knows
grep -q '"api_version":2' <<<"$(get "$jar_viewer" /api/health)" || fail "health does not show the SQL API version"
grep -q '"capacity":' <<<"$(get "$jar_viewer" /api/health)" || fail "health does not show the buffers"
sleep 5   # let the workload produce a few statements
trace=$(get "$jar_viewer" "/api/trace?limit=50")
grep -q '"exec_time_ms"' <<<"$trace" || fail "the trace is empty"
grep -q '"queryid":"-\?[0-9]\{16,\}"' <<<"$trace" || fail "64-bit query ids should be sent as strings"
grep -q '^\[' <<<"$(get "$jar_viewer" /api/running)" || fail "/api/running did not return a list"
grep -q '^\[' <<<"$(get "$jar_viewer" "/api/events?limit=10")" || fail "/api/events did not return a list"
grep -q '"databases"' <<<"$(get "$jar_viewer" /api/names)" || fail "names missing"
grep -q 'pg_profiler.buffer_size' <<<"$(get "$jar_viewer" /api/settings)" || fail "settings missing"

# the live stream delivers batches
stream=$(curl -s -N --max-time 6 -b "$jar_viewer" "$base/api/trace/stream" || true)   # curl ends by timeout
grep -q '^event: batch' <<<"$stream" || fail "no batch arrived on the live stream"

# permissions: a viewer cannot change anything, an admin can
for req in "POST /api/reset" "POST /api/rules" "DELETE /api/rules/1" "POST /api/backends/1/cancel"; do
  m=${req%% *}; p=${req#* }
  [ "$(code -b "$jar_viewer" -X "$m" -H "$H" -H 'Content-Type: application/json' -d '{}' "$base$p")" = "403" ] || fail "viewer was allowed to $req"
done
id=$(curl -s -b "$jar_admin" -X POST -H "$H" -H 'Content-Type: application/json' -d '{"application_name":"e2e-rule","min_duration_ms":1}' "$base/api/rules" | grep -o '"id":[0-9]*' | cut -d: -f2)
[ -n "$id" ] || fail "the admin could not add a capture rule"
grep -q 'e2e-rule' <<<"$(get "$jar_admin" /api/settings)" || fail "the new rule is not listed"
[ "$(code -b "$jar_admin" -X DELETE -H "$H" "$base/api/rules/$id")" = "200" ] || fail "the rule could not be removed"
[ "$(code -b "$jar_admin" -X POST -H "$H" -H 'Content-Type: application/json' -d '{}' "$base/api/reset")" = "200" ] || fail "the admin could not reset the buffers"

# ---- Query Store screens: wait for the archiver to store an interval (30 s intervals in the demo)
for i in $(seq 1 60); do
  grep -q '"has_history":true' <<<"$(get "$jar_viewer" /api/qs/status)" && break
  sleep 3
done
grep -q '"has_history":true' <<<"$(get "$jar_viewer" /api/qs/status)" || fail "no statistics interval was archived within three minutes"
now=$(date -u +%Y-%m-%dT%H:%M:%SZ); then=$(date -u -d '2 hours ago' +%Y-%m-%dT%H:%M:%SZ)
range="from=$then&to=$now"
overall=$(get "$jar_viewer" "/api/qs/overall?$range&bucket=60")
grep -q '"calls":' <<<"$overall" || fail "overall consumption is empty"
top=$(get "$jar_viewer" "/api/qs/top?$range&metric=exec_time&limit=5")
grep -q '"queryid":' <<<"$top" || fail "top consumers is empty"
[ "$(code -b "$jar_viewer" "$base/api/qs/top?$range&metric=bogus")" = "400" ] || fail "an unknown metric was accepted"
[ "$(code -b "$jar_viewer" "$base/api/qs/top?from=now();--&to=$now")" = "400" ] || fail "an invalid timestamp was accepted"
for ep in "variation?$range" "waits?$range" "wait-series?$range&bucket=60" \
          "regressed?recent_from=$then&recent_to=$now&baseline_from=$then&baseline_to=$now"; do
  grep -q '^\[' <<<"$(get "$jar_viewer" "/api/qs/$ep")" || fail "/api/qs/$ep did not return a list"
done
dbid=$(grep -o '"dbid":"[0-9]*"' <<<"$top" | head -1 | grep -o '[0-9]*')
qid=$(grep -o '"queryid":"\?-\?[0-9]*' <<<"$top" | head -1 | grep -o '\-\?[0-9]*$')
[ -n "$dbid" ] && [ -n "$qid" ] || fail "could not read a query id from the top list"
grep -q '"history":\[' <<<"$(get "$jar_viewer" "/api/qs/query?dbid=$dbid&queryid=$qid&$range")" || fail "no query history"
grep -q '"plans":\[' <<<"$(get "$jar_viewer" "/api/qs/query/plans?dbid=$dbid&queryid=$qid")" || fail "no plan list"

# logging out ends the session
[ "$(code -b "$jar_viewer" -X POST -H "$H" "$base/api/logout")" = "200" ] || fail "logout failed"
echo "UI end-to-end OK"
