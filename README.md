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
| `cmd_type` | `select`, `insert`, `update`, `delete`, `merge` or `utility` |
| `nesting` | 0 for top-level statements, greater for statements run inside functions |
| `exec_time_ms`, `plan_time_ms` | Execution and planning time. Plan time is 0 for cached plans (prepared statements) |
| `cpu_user_ms`, `cpu_sys_ms` | CPU time of the backend process during the statement |
| `rows` | Rows returned or affected |
| `shared_blks_*`, `temp_blks_*` | Buffer usage |
| `blk_read_time_ms`, `blk_write_time_ms` | I/O time (needs `track_io_timing = on`) |
| `wal_records`, `wal_fpi`, `wal_bytes` | WAL usage |
| `is_error`, `sqlstate` | Whether the statement failed, and with which SQLSTATE |
| `query` | Statement text (only that statement of a multi-statement string) |
| `params` | Bind parameters, e.g. `$1 = '42'` (null unless parameter capture is on) |

The views add a `toplevel` column (`nesting = 0`).

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
(`pg_profiler_api_version()` = 1) is what the planned GUI relies on.

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

Memory: everything is allocated once at server start. A statement slot is 328
bytes + `max_query_len` + `max_param_len`; an event slot is 160 bytes +
`event_detail_len`; the running view costs `max_connections` × (about 70 bytes +
`running_query_len`). With the defaults that is about 24 MB + 4.4 MB + 0.1 MB.
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
