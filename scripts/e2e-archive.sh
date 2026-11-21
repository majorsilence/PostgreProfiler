#!/usr/bin/env bash
# End-to-end check of the statistics history against the running `db` service
# (docker compose up --build -d db, started with PG_PROFILER_STATS_INTERVAL=10):
# generate load, wait for the archiver, and read the history back.
set -euo pipefail
sql() { docker compose exec -T db psql -U postgres -v ON_ERROR_STOP=1 -qAt "$@"; }
fail() { echo "FAIL: $*" >&2; docker compose logs db | tail -30 >&2; exit 1; }

sql -c "CREATE EXTENSION IF NOT EXISTS pg_profiler"
# wait sampling: a one second sleep is counted in the timeout category (index 9, 1-based)
sql -c "SELECT pg_sleep(1) /* e2e_wait */" >/dev/null
w=$(sql -c "SELECT coalesce(sum(m.wait_samples[9]), 0) FROM pg_profiler_stats_memory() m JOIN pg_profiler_stats_texts() t USING (dbid, queryid) WHERE t.query LIKE '%e2e_wait%'")
[ "$w" -ge 50 ] || fail "expected wait samples for a one second sleep, found $w"

docker compose exec -T db bash -c 'pgbench -U postgres -i -q postgres >/dev/null 2>&1 && pgbench -U postgres -T 25 -c 4 -M prepared postgres >/dev/null 2>&1'
sleep 15   # the last interval closes, then the archiver's grace period and poll

n=$(sql -c "SELECT count(DISTINCT interval_start) FROM pg_profiler_hist_stats")
[ "$n" -ge 2 ] || fail "expected archived intervals, found $n"
calls=$(sql -c "SELECT coalesce(sum(calls), 0) FROM pg_profiler_runtime_stats")
[ "$calls" -gt 1000 ] || fail "runtime_stats shows only $calls calls"
q=$(sql -c "SELECT count(*) FROM pg_profiler_hist_queries WHERE query LIKE '%pgbench_accounts%'")
[ "$q" -ge 1 ] || fail "no pgbench query text archived"
p=$(sql -c "SELECT count(*) FROM pg_profiler_hist_plans")
[ "$p" -ge 1 ] || fail "no plans archived"
dup=$(sql -c "SELECT count(*) FROM (SELECT interval_start, dbid, userid, queryid, plan_id FROM pg_profiler_runtime_stats GROUP BY 1,2,3,4,5 HAVING count(*) > 1) d")
[ "$dup" -eq 0 ] || fail "runtime_stats counts $dup rows twice"
lost=$(sql -c "SELECT lost_intervals FROM pg_profiler_stats_info()")
[ "$lost" -eq 0 ] || fail "$lost intervals were lost before they were archived"
echo "archive end-to-end OK: $w wait samples, $n intervals, $calls calls, $q pgbench queries, $p plans"
