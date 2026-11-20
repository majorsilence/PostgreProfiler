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

Supports PostgreSQL 17 and 18.

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
SELECT start_time, exec_time_ms, rows, query FROM pg_profiler_recent;

-- the 100 slowest executions still in the buffer
SELECT exec_time_ms, plan_time_ms, shared_blks_read, query FROM pg_profiler_slowest;

-- failed statements, with their SQLSTATE
SELECT start_time, sqlstate, query FROM pg_profiler_errors;

-- everything, newest first (the argument limits the row count, default 1000)
SELECT * FROM pg_profiler(5000) WHERE queryid = 1234567890;

-- how full is the buffer?
SELECT * FROM pg_profiler_info();

-- forget everything recorded so far (superuser by default)
SELECT pg_profiler_reset();
```

### Columns

| Column | Meaning |
|---|---|
| `seq` | Monotonic sequence number |
| `start_time` | When the statement started |
| `userid`, `dbid`, `pid` | Role, database and backend that ran it |
| `queryid` | Core query id (same as `pg_stat_statements`) |
| `cmd_type` | `select`, `insert`, `update`, `delete`, `merge` or `utility` |
| `toplevel`, `nesting` | `false`/`> 0` for statements run inside functions |
| `exec_time_ms`, `plan_time_ms` | Execution and planning time. Plan time is 0 for cached plans (prepared statements) |
| `cpu_user_ms`, `cpu_sys_ms` | CPU time of the backend process during the statement |
| `rows` | Rows returned or affected |
| `shared_blks_*`, `local_blks_*`, `temp_blks_*` | Buffer usage |
| `wal_records`, `wal_fpi`, `wal_bytes` | WAL usage |
| `is_error`, `sqlstate` | Whether the statement failed, and with which SQLSTATE |
| `query` | Statement text (only that statement of a multi-statement string) |

The `query` column shows `<insufficient privilege>` for other users' statements
unless you are a superuser or a member of `pg_read_all_stats` (the same rule as
`pg_stat_statements`).

### Configuration

| Setting | Default | Context | Meaning |
|---|---|---|---|
| `pg_profiler.enabled` | `on` | superuser | Master switch |
| `pg_profiler.track` | `top` | superuser | `none`, `top` (top-level statements) or `all` (also statements run inside functions) |
| `pg_profiler.track_utility` | `on` | superuser | Record DDL and other utility commands |
| `pg_profiler.sample_rate` | `1.0` | superuser | Fraction (0–1) of top-level statements recorded. Nested statements follow their parent's decision |
| `pg_profiler.min_duration_ms` | `0` | superuser | Do not record successful statements faster than this. Errors are always recorded |
| `pg_profiler.buffer_size` | `10000` | restart | Number of executions kept |
| `pg_profiler.max_query_len` | `2048` | restart | Bytes of query text kept per execution (`0` = no text) |

Memory: the ring is allocated once at server start, `buffer_size` ×
(184 bytes + `max_query_len`, rounded up to 8). With the defaults that is about 22 MB. When
the buffer is full, the oldest rows are overwritten (`pg_profiler_info().overwritten`
counts them).

### What is and is not recorded

- Statements run by parallel workers are not recorded separately; their work is
  part of the leader's totals.
- `EXECUTE` is recorded as the statement it runs (the `PREPARE`/`EXECUTE`/`DEALLOCATE`
  commands themselves are skipped), as in `pg_stat_statements`.
- Errors are captured when they happen during planning or execution (or in a
  utility command). Syntax errors never reach the hooks and are not recorded.
- Query text is stored as written; it is not normalised, so literals are visible
  to anyone allowed to see the text.
- Overhead is a few microseconds per statement (instrumentation, two `getrusage`
  calls, and a short exclusive lock to append the row).

## Development (Docker)

Both the Rust build and Postgres run in Docker; nothing needs to be installed
locally besides Docker.

```sh
# Lint and test inside the Rust + pgrx container
docker compose run --rm builder cargo clippy --features pg18 -- -D warnings
docker compose run --rm builder cargo pgrx test pg18
docker compose run --rm builder cargo pgrx test pg17

# Top-level behaviour tests (tests/pg_regress), with the library preloaded
docker compose run --rm builder bash scripts/regress.sh 18

# Build the extension into a Postgres image and run it
docker compose up --build db
psql postgres://postgres:postgres@localhost:5432/postgres
```

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
