#!/usr/bin/env bash
# A small, varied workload for the demo: steady transactions, slow reports,
# errors, lock contention behind an idle transaction, and the occasional
# deadlock. Everything uses its own application_name so the UI can tell the
# sources apart. Runs until the container stops.
set -u
export PGHOST="${PGHOST:-db}" PGUSER="${PGUSER:-postgres}" PGDATABASE="${PGDATABASE:-postgres}"

until pg_isready -q; do sleep 1; done
echo "workload: initialising pgbench tables"
PGAPPNAME=pgbench-init pgbench -i -q -s 5 >/dev/null 2>&1

q() { PGAPPNAME="$1" psql -X -q -v ON_ERROR_STOP=0 -c "$2" >/dev/null 2>&1; }

# 1. steady OLTP traffic
( while true; do PGAPPNAME=checkout pgbench -c 4 -j 2 -T 300 -R 60 >/dev/null 2>&1; done ) &

# 2. reporting queries: heavier, every few seconds
( while true; do
    q reporting "SELECT b.bid, count(*), sum(a.abalance) FROM pgbench_accounts a JOIN pgbench_branches b USING (bid) GROUP BY b.bid ORDER BY 3 DESC"
    q reporting "SELECT count(*) FROM pgbench_history h JOIN pgbench_accounts a USING (aid) WHERE h.delta > 0"
    sleep 3
  done ) &

# 3. statements that fail
( while true; do
    q errors-demo "SELECT 1 / (abalance - abalance) FROM pgbench_accounts LIMIT 1"
    q errors-demo "SELECT * FROM table_that_does_not_exist"
    q errors-demo "INSERT INTO pgbench_branches (bid, bbalance) VALUES (1, 0)"
    sleep 7
  done ) &

# 4. a long statement now and then
( while true; do q nightly-job "SELECT pg_sleep(12)"; sleep 40; done ) &

# 5. lock contention: a session idles in a transaction holding a row lock,
#    two others queue up behind it (visible in Running Now and as lock waits)
( while true; do
    sleep 25
    PGAPPNAME=batch-import psql -X -q >/dev/null 2>&1 <<'SQL' &
BEGIN;
UPDATE pgbench_branches SET bbalance = bbalance WHERE bid = 2;
\! sleep 9
COMMIT;
SQL
    sleep 1
    q checkout "UPDATE pgbench_branches SET bbalance = bbalance + 1 WHERE bid = 2" &
    q reporting "UPDATE pgbench_branches SET bbalance = bbalance - 1 WHERE bid = 2" &
    wait
  done ) &

# 6. a deadlock every couple of minutes
( while true; do
    sleep 110
    PGAPPNAME=transfer-a psql -X -q >/dev/null 2>&1 <<'SQL' &
BEGIN;
UPDATE pgbench_tellers SET tbalance = tbalance + 1 WHERE tid = 1;
SELECT pg_sleep(2);
UPDATE pgbench_tellers SET tbalance = tbalance - 1 WHERE tid = 2;
COMMIT;
SQL
    sleep 0.5
    PGAPPNAME=transfer-b psql -X -q >/dev/null 2>&1 <<'SQL' &
BEGIN;
UPDATE pgbench_tellers SET tbalance = tbalance + 1 WHERE tid = 2;
SELECT pg_sleep(2);
UPDATE pgbench_tellers SET tbalance = tbalance - 1 WHERE tid = 1;
COMMIT;
SQL
    wait
  done ) &

echo "workload: running"
wait
