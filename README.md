# PostgreProfiler
[![CI](https://github.com/majorsilence/PostgreProfiler/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/majorsilence/PostgreProfiler/actions/workflows/ci.yml)

`pg_profiler` is a Postgres extension, written in Rust with
[pgrx](https://github.com/pgcentralfoundation/pgrx), that records **one row per
statement execution**: duration, planning time, rows, buffer and WAL usage, CPU
time, errors and the query text. Rows are kept in a shared-memory ring buffer
and read back with plain SQL.

`pg_stat_statements` only aggregates per query. `pg_profiler` shows the
individual executions, which is what you need to investigate one slow run or one
error.

Supports PostgreSQL 17, 18 and 19.

## Installation

The extension needs shared memory and hooks, so it must be preloaded:

```
# postgresql.conf (restart required)
shared_preload_libraries = 'pg_profiler'
```

then, in each database you want to query it from:

```sql
CREATE EXTENSION pg_profiler;
```

The functions raise `pg_profiler must be loaded via shared_preload_libraries`
if the library was not preloaded.

Try it without installing anything locally:

```sh
docker compose up --build -d db
psql postgres://postgres:postgres@localhost:5432/postgres -c 'CREATE EXTENSION pg_profiler'
```

## Usage

```sql
-- the 100 most recent executions, newest first
SELECT start_time, application_name, exec_time_ms, rows, query FROM pg_profiler_recent;

-- the 100 slowest executions still in the buffer
SELECT exec_time_ms, plan_time_ms, shared_blks_read, query FROM pg_profiler_slowest;

-- failed statements, with their SQLSTATE
SELECT start_time, sqlstate, query FROM pg_profiler_errors;

-- what is running right now, with waits and blockers
SELECT pid, usename, state, wait_event, elapsed, blocked_by, query FROM pg_profiler_activity;

-- deadlocks, lock waits, cancels, logins, ... (newest first)
SELECT ts, kind, pid, duration_ms, detail FROM pg_profiler_events();

-- everything, newest first (the argument limits the row count, default 1000)
SELECT * FROM pg_profiler(5000) WHERE queryid = 1234567890;

-- incremental read for live viewers: rows after a sequence number, oldest first
SELECT * FROM pg_profiler_since(12345, 500);

-- how full are the buffers?
SELECT * FROM pg_profiler_info();
```

### Statement columns (`pg_profiler()`, `pg_profiler_since()`)

| Column | Meaning |
|---|---|
| `seq` | Monotonic sequence number |
| `start_time`, `xact_start` | When the statement and its transaction started |
| `userid`, `dbid`, `pid` | Role, database and backend that ran it |
| `application_name`, `client_addr` | Session context (`client_addr` is `[local]` for unix sockets) |
| `xid` | Transaction id, null if none had been assigned yet |
| `queryid` | Core query id (same as `pg_stat_statements`) |
| `plan_id` | Fingerprint of the plan shape (see [Plans](#plans)); 0 for utility statements |
| `cmd_type` | `select`, `insert`, `update`, `delete`, `merge` or `utility` |
| `nesting` | 0 for top-level statements, greater for statements run inside functions |
| `exec_time_ms`, `plan_time_ms` | Execution and planning time. Plan time is 0 for cached plans (prepared statements) |
| `cpu_user_ms`, `cpu_sys_ms` | CPU time of the backend process during the statement |
| `rows` | Rows returned or affected |
| `shared_blks_*`, `temp_blks_*` | Buffer usage |
| `blk_read_time_ms`, `blk_write_time_ms` | I/O time (needs `track_io_timing = on`) |
| `wal_records`, `wal_fpi`, `wal_bytes` | WAL usage |
| `sqlstate` | SQLSTATE if the statement failed, otherwise null |
| `query` | Statement text (only that statement of a multi-statement string) |
| `params` | Bind parameters, e.g. `$1 = '42'` (null unless parameter capture is on) |

The views add `toplevel` (`nesting = 0`) and `is_error` (`sqlstate IS NOT NULL`) columns.

### Plans

Every recorded statement carries a `plan_id`: a fingerprint of the *shape* of its
plan (node types, tables, indexes, join types, aggregate strategies, tree
structure). Costs, row estimates, constants and parameters are left out, so a
query keeps one `plan_id` until the planner picks something else, for example
after an index is dropped or statistics change. The value is only meaningful on
the server version that produced it.

```sql
-- queries that ran with more than one plan shape (candidates for a regression)
SELECT queryid, plans, calls, fastest_avg_ms, slowest_avg_ms, slowdown, query
  FROM pg_profiler_plan_changes ORDER BY slowdown DESC;

-- one row per (query, plan) with its timings
SELECT queryid, plan_id, calls, avg_exec_ms, max_exec_ms, last_seen FROM pg_profiler_plan_stats;

-- stored plans: EXPLAIN text (or JSON), newest first
SELECT ts, reason, exec_time_ms, max_misestimate, plan FROM pg_profiler_plans(20);
SELECT * FROM pg_profiler_plans_since(41, 100);     -- incremental, oldest first
```

A plan is stored in the plan ring when

- its shape is seen for the first time for that database and query
  (`reason = new`; estimates only, no actual rows), or
- the statement was one of the `pg_profiler.plan_sample_rate` sample, which runs
  with per-node row counting, **and** the run was slower than
  `pg_profiler.plan_outlier_ms` (`slow`), wrote temporary files (`spill`), or had
  a row estimate off by more than `pg_profiler.plan_misestimate_ratio`
  (`misestimate`). Such a plan has actual row counts (`has_actuals`) and
  `max_misestimate` is the largest factor found. Estimates of nodes below a
  `LIMIT` and of parallel-aware nodes are ignored, and tiny row counts (under 100)
  too.

Each plan is stored once per combination of reasons, until `pg_profiler_reset()`
or until it is pushed out of the small table of plans already seen, so a
recurring outlier does not flood the ring. With `plan_sample_rate = 0` (the
default) nothing runs with row counting. `pg_profiler.plan_format` chooses text
or JSON (`EXPLAIN (FORMAT JSON)` layout, suitable for plan viewers). Plan text
follows the same visibility rule as query text, and can contain constants from
the query.

### Statistics and history (Query Store)

Besides the individual executions, every tracked statement is added to
**per-interval statistics** (default: 5 minutes) per database, role, query and
plan: calls, errors, total/min/max execution time, the sum of squares (for the
standard deviation), planning time, rows, buffer, WAL, CPU and I/O counters, a
latency histogram (percentiles) and wait samples. They are kept in memory for the
last four intervals and copied by a background worker into tables of the
**archive database**, so they survive restarts and give you history.

```sql
-- statistics per interval: the archive plus intervals not yet archived
SELECT * FROM pg_profiler_runtime_stats WHERE queryid = 1234567890;

-- the report functions (all take a time range; see the argument defaults)
SELECT queryid, calls, total_exec_ms, avg_exec_ms, p95_exec_ms, query
  FROM pg_profiler_top_consumers('exec_time', now() - interval '1 day', now(), 10);
SELECT * FROM pg_profiler_overall_consumption(now() - interval '1 day', now(), interval '1 hour');
SELECT * FROM pg_profiler_high_variation();                  -- highest coefficient of variation
SELECT * FROM pg_profiler_query_history(1234567890);         -- one query, per interval and plan
SELECT * FROM pg_profiler_wait_stats();                      -- samples per wait category
-- queries that got slower than before, with a statistical test and plan-change flag
SELECT queryid, baseline_avg_ms, recent_avg_ms, ratio, z_score, plan_changed, query
  FROM pg_profiler_regressed_queries();                      -- last hour against the 24 hours before
```

`pg_profiler_top_consumers` takes a metric: `exec_time`, `avg_time`, `calls`,
`rows`, `reads`, `temp`, `wal`, `cpu`, `io_time` or `plan_time`.
`pg_profiler_regressed_queries` lists a query when its mean run time grew by at
least `min_ratio` (default 1.5) and Welch's t statistic, computed from the sums
and sums of squares, is at least `min_z` (default 3) with at least `min_calls`
calls in both windows.

**Setup.** The archiver stores everything in one database,
`pg_profiler.archive_database` (default `postgres`), which needs
`CREATE EXTENSION pg_profiler`; until it has the extension the worker only logs
a reminder. The tables are `pg_profiler_hist_stats` (partitioned by day, created
on demand), `pg_profiler_hist_queries`, `pg_profiler_hist_plans` (one row per plan
shape, at most `max_plans_per_query` per query) and `pg_profiler_hist_plan_samples`
(outlier plans with actual rows). Read access, including the views and report
functions, is granted to `profiler_reader`; the query text and plans are as
sensitive as in the trace. Retention runs hourly: daily partitions older than
`retention_days` are dropped, then the oldest day while the history exceeds
`max_archive_size_mb`, and stale queries, plans and samples are deleted
(`pg_profiler_purge()` does the same on demand). Other databases see the statistics
still in memory through the same views, which is enough for a quick look.

Things to know:

- The statistics count every tracked statement, whatever `min_duration_ms` and the
  capture rules say, so totals are complete. `sample_rate` still applies, and with
  `stats_capture = auto` a query only gets an entry once a run took at least
  `stats_auto_min_ms` (or failed).
- Errors are counted in `errors`, not in `calls`, and add nothing to the timings.
- The interval in progress is saved when the server shuts down cleanly, but not
  after a crash or on a standby, which keeps its statistics in memory only.
- If the archiver cannot keep up (or `archive = off`), an interval that falls out
  of memory unarchived is counted in `lost_intervals` (`pg_profiler_stats_info()`).
  When an interval has more distinct entries than `stats_entries`, the rest are
  counted in `dropped_entries`.
- **Wait sampling:** a worker looks every `wait_sample_ms` (default 10) at what each
  running statement is waiting for and counts it by category (`pg_profiler_wait_categories()`:
  cpu, lwlock, lock, io, ...). It is statistical, so statements shorter than the
  period show nothing, samples are attributed to the top-level statement, and it
  needs `track_running`.
- Percentiles come from a histogram with two buckets per octave, so they are
  accurate to about 20 %.
- Plan fingerprints (`plan_id`) are not comparable across server versions.

### Events (`pg_profiler_events()`, `pg_profiler_events_since()`)

| `kind` | Recorded when | Notes |
|---|---|---|
| `login`, `logout` | A backend runs its first statement / exits | `logout` carries the session length in `duration_ms` |
| `auth_failure` | A connection is rejected (SQLSTATE class 28) | |
| `deadlock` | A deadlock is detected | `detail` holds the whole cycle with the statements involved |
| `lock_wait`, `lock_acquired` | A lock wait passes `deadlock_timeout` and when it ends | needs `log_lock_waits = on`; `duration_ms` is the wait so far / in total |
| `lock_timeout` | `lock_timeout` or `NOWAIT` fails | |
| `cancel` | A statement is cancelled (`statement_timeout`, user cancel) | |
| `terminated` | A backend is terminated by an administrator | |

Events come from the server log stream, so lock-wait messages are matched on
their English text.

### Capture rules

By default every statement that passes the settings below is recorded. Rules let
you keep only what you care about; a statement is kept if it matches **any**
rule, and a rule matches when **all** of its conditions hold:

```sql
-- keep statements of apps starting with 'billing' slower than 50 ms, and any error
SELECT pg_profiler_rule_add(application_name => 'billing', min_duration_ms => 50);
SELECT pg_profiler_rule_add(errors_only => true);
SELECT * FROM pg_profiler_rules();
SELECT pg_profiler_rule_remove(1);
```

Conditions: `database`, `role`, `application_name` (prefix), `min_duration_ms`,
`query_contains` (substring) and `errors_only`. Errors ignore `min_duration_ms`.
At most 16 rules. Rules decide what enters the ring, so they reduce noise and
buffer churn but not instrumentation overhead.

### Roles and privileges

| Who | Can |
|---|---|
| anyone | read rows (query text, parameters and event detail of other users' statements show `<insufficient privilege>`) |
| superuser, `pg_read_all_stats`, **`profiler_reader`** | see all text |
| **`profiler_admin`** | also `pg_profiler_reset()` and manage capture rules |

`profiler_reader` and `profiler_admin` are created by `CREATE EXTENSION`; grant
`profiler_reader` to the account a monitoring tool uses. The SQL surface
(`pg_profiler_api_version()` = 2) is what the planned GUI relies on.

### Configuration

| Setting | Default | Context | Meaning |
|---|---|---|---|
| `pg_profiler.enabled` | `on` | superuser | Master switch |
| `pg_profiler.track` | `top` | superuser | `none`, `top` (top-level statements) or `all` (also statements run inside functions) |
| `pg_profiler.track_utility` | `on` | superuser | Record DDL and other utility commands |
| `pg_profiler.track_events` | `on` | superuser | Record events (logins, deadlocks, ...) |
| `pg_profiler.track_running` | `on` | superuser | Publish each backend's running statement |
| `pg_profiler.sample_rate` | `1.0` | superuser | Fraction (0–1) of top-level statements recorded. Nested statements follow their parent's decision |
| `pg_profiler.min_duration_ms` | `0` | superuser | Do not record successful statements faster than this. Errors are always recorded |
| `pg_profiler.capture_params` | `off` | superuser | Record bind parameter values. **Sensitive**: may contain personal data |
| `pg_profiler.buffer_size` | `10000` | restart | Number of executions kept |
| `pg_profiler.max_query_len` | `2048` | restart | Bytes of query text kept per execution (`0` = no text) |
| `pg_profiler.max_param_len` | `0` | restart | Bytes of parameter text kept per execution. `0` allocates no space, so parameters can never be captured |
| `pg_profiler.events_buffer_size` | `2000` | restart | Number of events kept |
| `pg_profiler.event_detail_len` | `2048` | restart | Bytes of detail text kept per event |
| `pg_profiler.running_query_len` | `1024` | restart | Bytes of text kept per backend for the running view |
| `pg_profiler.capture_plans` | `on` | superuser | Store the plan of each new plan shape |
| `pg_profiler.plan_format` | `text` | superuser | `text` or `json` |
| `pg_profiler.plan_buffer_size` | `500` | restart | Number of plans kept |
| `pg_profiler.max_plan_len` | `8192` | restart | Bytes of plan text kept per plan. `0` turns plan capture off and allocates no space |
| `pg_profiler.plan_sample_rate` | `0` | superuser | Fraction (0–1) of statements run with row counting so that outliers can be stored with actual rows |
| `pg_profiler.plan_outlier_ms` | `1000` | superuser | A sampled statement slower than this is stored with actual rows (`0` = off) |
| `pg_profiler.plan_misestimate_ratio` | `100` | superuser | A sampled statement with an estimate off by more than this factor is stored (`0` = off) |
| `pg_profiler.stats_capture` | `all` | superuser | `all`, `auto` or `none`: which statements go into the interval statistics |
| `pg_profiler.stats_auto_min_ms` | `1` | superuser | In `auto` mode, the run time that makes a query worth tracking |
| `pg_profiler.stats_interval` | `300` | restart | Length of a statistics interval in seconds (choose a divisor of a day) |
| `pg_profiler.stats_entries` | `5000` | restart | Distinct (database, role, query, plan) entries per interval |
| `pg_profiler.stats_text_len` | `1024` | restart | Bytes of query text kept per query for the statistics |
| `pg_profiler.wait_sample_ms` | `10` | restart | Wait sampling period in milliseconds (`0` = off) |
| `pg_profiler.archive` | `on` | reload | Store closed intervals in the archive database |
| `pg_profiler.archive_database` | `postgres` | restart | Database that holds the history; needs the extension |
| `pg_profiler.retention_days` | `30` | reload | Days of history to keep (`0` = forever) |
| `pg_profiler.max_archive_size_mb` | `1024` | reload | Size cap for the history; the oldest day is dropped first (`0` = no cap) |
| `pg_profiler.max_plans_per_query` | `20` | reload | Plans kept per query in the history (`0` = no cap) |

Memory: everything is allocated once at server start. A statement slot is 336
bytes + `max_query_len` + `max_param_len`; an event slot is 160 bytes +
`event_detail_len`; a plan slot is 88 bytes + `max_plan_len` + `max_query_len`;
the running view costs `max_connections` × (about 70 bytes + `running_query_len`);
the table of plans already seen is 128 KB; the statistics keep four intervals of
`stats_entries` entries of 488 bytes plus one `stats_text_len` text per entry
(and 48 bytes of wait counters per backend). With the defaults that is about
24 MB + 4.4 MB + 5 MB + 0.1 MB + 15 MB.
When a ring is full the oldest rows are overwritten (`pg_profiler_info()` counts
them).

### What is and is not recorded

- Statements run by parallel workers are not recorded separately; their work is
  part of the leader's totals.
- `EXECUTE` is recorded as the statement it runs (the `PREPARE`/`EXECUTE`/`DEALLOCATE`
  commands themselves are skipped), as in `pg_stat_statements`.
- Errors are captured when they happen during planning or execution (or in a
  utility command). Syntax errors never reach the statement hooks and are not
  recorded as statements, but cancels, deadlocks and authentication failures
  appear as events.
- A login event is written when the backend first runs a statement; a session
  that connects and does nothing only shows its logout.
- Query text is stored as written; it is not normalised, so literals are visible
  to anyone allowed to see the text.
- Overhead is a few microseconds per statement (instrumentation, two `getrusage`
  calls, and a short exclusive lock to append the row). On a Docker Desktop test
  box the difference to running without the extension was within run-to-run noise
  (about ±15 %), so a precise figure is not yet established.

## Development (Docker)

Both the Rust build and Postgres run in Docker; nothing needs to be installed
locally besides Docker.

```sh
# Lint and test inside the Rust + pgrx container
docker compose run --rm builder cargo clippy --features pg18 -- -D warnings
docker compose run --rm builder cargo pgrx test pg18
docker compose run --rm builder cargo pgrx test pg17
docker compose run --rm builder cargo pgrx test pg19

# Top-level behaviour tests (tests/pg_regress), with the library preloaded
docker compose run --rm builder bash scripts/regress.sh 18

# Build the extension into a Postgres image and run it
docker compose up --build db
psql postgres://postgres:postgres@localhost:5432/postgres

# The same for another major version (17, 18 or 19)
PG_MAJOR=19 docker compose up --build db
```

The builder's pgrx configuration lives in a volume. If you created it before
PG19 was added, register the new server once with
`docker compose run --rm builder cargo pgrx init --pg19 /usr/lib/postgresql/19/bin/pg_config`.

See [docs/DESIGN.md](docs/DESIGN.md) for the design and the plan for OTLP export, and
[docs/ROADMAP.md](docs/ROADMAP.md) for the plan for Profiler parity, a Query Store and a GUI.

## License

Licensed under any of the following, at your option:

- MIT license ([LICENSE-MIT](LICENSE-MIT))
- BSD 3-Clause license ([LICENSE-BSD-3-Clause](LICENSE-BSD-3-Clause))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- PostgreSQL License ([LICENSE-PostgreSQL](LICENSE-PostgreSQL))

SPDX identifier: `MIT OR BSD-3-Clause OR Apache-2.0 OR PostgreSQL`

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project shall be licensed as above, without any
additional terms or conditions.
