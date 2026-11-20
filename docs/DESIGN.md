# pg_profiler — v1 design & implementation plan

## Context

PostgreProfiler (github.com/majorsilence/PostgreProfiler) is a new Rust/pgrx Postgres
extension, `pg_profiler`, that records **one row per statement execution**: duration,
planning time, rows, buffer/WAL usage, CPU, errors and query text. It keeps them in a
shared-memory ring buffer and reads them back with SQL. `pg_stat_statements` only
aggregates per query; this extension shows individual executions, which is what you need
to investigate a slow run or an error.

Starting point (commit "Scaffold pg_profiler pgrx extension"):
- `cargo pgrx new` scaffold at the repo root (pgrx **0.20.0**, latest; features `pg17`
  and `pg18`, default `pg18`). `cargo pgrx test pg17|pg18` passes in Docker.
- The Dockerfile has three stages: `toolchain` (Rust, cargo-pgrx, PG 17 and 18 dev
  packages, non-root `dev` user, `USER=dev`), `build` (`cargo pgrx package`) and
  `postgres` (`postgres:18` with the extension installed and
  `shared_preload_libraries=pg_profiler`). The compose `builder` service uses
  `toolchain`.

Decisions taken: name `pg_profiler`; PG 17 and 18 (develop on 18); v1 = core tracing,
**no external export** (OTLP/ClickHouse come later).

The pasted guide is **not** reliable for pgrx 0.20. Facts checked in the pgrx 0.20
sources:
- There is no `pgrx::hooks`. Hook globals are
  `Option<unsafe extern "C-unwind" fn(..)>`, so the previous hook goes in a plain
  `static mut PREV_X: pg_sys::X_hook_type = None` (backends are single-threaded; this is
  exactly what `pg_shmem_init!` does). No `AtomicPtr` or transmute.
- `ExecutorRun_hook_type` has an extra `execute_once: bool` on **PG 17 only**, so it
  needs `#[cfg(feature = "pg17")]`/`pg18` variants. The other hook types are identical
  on 17 and 18.
- The GUC API is `GucRegistry::define_{bool,int,float,enum}_guc(c"name", c"short",
  c"long", &STATIC_GUCSETTING, [min, max,] GucContext, GucFlags)`, with
  `static X: GucSetting<T> = GucSetting::<T>::new(..)` and `X.get()`.
- Shared memory: `pg_shmem_init!` / `PgLwLock<T>` only support fixed-size `T`. Because
  the ring size comes from a GUC, use raw `pg_sys` instead: `RequestAddinShmemSpace`,
  `RequestNamedLWLockTranche`, `ShmemInitStruct`, `GetNamedLWLockTranche`.
- Error handling: `pgrx::pg_sys::PgTryBuilder::new(|| ..).catch_others(|e| ..)
  .finally(|| ..).execute()`. The guide's `PgTry` does not exist.
- Instrumentation: allocate `totaltime` **after** chaining `ExecutorStart`, inside
  `estate->es_query_cxt`, with
  `InstrAlloc(1, InstrumentOption::INSTRUMENT_ALL as i32, false)` (the
  pg_stat_statements pattern). The guide allocates it before and with the wrong
  arguments.
- Other symbols present on both versions: `ParallelWorkerNumber`, `EnableQueryId`,
  `pg_prng_double` + `pg_global_prng_state`, `planner_hook`. `WalUsage` gains
  `wal_buffers_full` on PG 18 only.
- The guide's per-backend `Vec` for query text cannot work, because other backends
  can't read it. The text must live in shared memory.

## Architecture

```
src/
  lib.rs        pg_module_magic!, _PG_init (preload guard → gucs → EnableQueryId → shmem → hooks)
  gucs.rs       GucSetting statics + register()
  record.rs     TraceRecord (#[repr(C)] fixed header) + RecordBuilder helpers
  ring.rs       shmem layout, request/startup hooks, push(), snapshot(), reset(), stats()
  backend.rs    per-backend state (thread_local! Cell/RefCell): nesting_level, sampled flag,
                pending plan time, per-QueryDesc start info
  hooks/mod.rs  install() + PREV_* statics + chain helpers
  hooks/planner.rs, executor.rs, utility.rs
  sql.rs        #[pg_extern] SRFs + extension_sql! views/grants
```

### Ring buffer (`ring.rs`)
- Layout in one `ShmemInitStruct("pg_profiler", size)`: a `RingHeader { capacity: u64,
  slot_size: u64, head: u64, reset_at: u64, total_written: u64, text_cap: u32 }`,
  followed by `capacity` slots. Each slot is a `TraceRecord` plus `text_cap` bytes of
  inline query text. Size = header + capacity × align8(size_of::<TraceRecord>() +
  text_cap).
- `capacity` = `pg_profiler.buffer_size` and `text_cap` = `pg_profiler.max_query_len`
  (both `PGC_POSTMASTER`). Document the memory cost (defaults 10 000 × ~2.2 KB ≈ 22 MB).
- Locking: one named LWLock tranche `"pg_profiler"` (1 lock).
  - `push()` builds the record and truncates the text (UTF-8-safe cut) **before** taking
    the lock. While holding `LW_EXCLUSIVE` it only does `slot = head % cap`, a memcpy of
    the record and text, and `head += 1`.
  - `snapshot(limit)` copies out under `LW_SHARED` into a local `Vec`, newest first, only
    for slots with sequence ≥ `reset_at`, then releases the lock before building tuples.
  - `reset()` sets `reset_at = head` under exclusive.
  - Lock-free per-slot seqlocks are a later optimisation; record that in a code comment.
- Hooks chain `shmem_request_hook` / `shmem_startup_hook` the same way as the others.
  Also handle `!found` versus already-initialised.

### TraceRecord fields
`seq u64, queryid i64, top_queryid? (skip v1), start_ts i64 (TimestampTz),
exec_time_us f64, plan_time_us f64, cpu_user_us i64, cpu_sys_us i64, rows u64,
shared_blks_hit/read/dirtied/written i64, local_blks_hit/read i64, temp_blks_read/written i64,
wal_records i64, wal_fpi i64, wal_bytes u64, userid Oid, dbid Oid, pid i32, nesting i32,
cmd_type u8 (select/insert/update/delete/merge/utility), toplevel bool, is_error bool,
sqlstate [u8;5], query_len u32`

### Per-backend state & nesting (`backend.rs`)
- Use the pg_stat_statements pattern rather than the guide's. `nesting_level` is
  incremented around `ExecutorRun`, `ExecutorFinish` and `ProcessUtility` (and the
  planner) inside `PgTryBuilder ... .finally(|| level -= 1)`. That way an `ERROR` cannot
  leak the counter, even though `ExecutorEnd` is skipped on abort.
- `toplevel = nesting_level == 0` at `ExecutorStart` / `ProcessUtility` entry.
- Sampling: at each **top-level** entry, `sampled = enabled && pg_prng_double(..) <
  sample_rate`. Nested statements inherit the flag, so a sampled statement's children
  are kept together. Use random sampling, not the guide's queryid hash, which keeps
  every execution of some queries and none of others.
- `pg_profiler.track = top | all | none` (enum) decides whether nested statements are
  recorded.

### Hooks
| Hook | Work |
|---|---|
| `planner_hook` | When active, time the planner call (nesting++ in try/finally). Stash `(result PlannedStmt ptr, plan_us)` in backend state. |
| `ExecutorStart` | Skip if `ParallelWorkerNumber >= 0` or not tracked. Chain first. Then allocate `totaltime` in `es_query_cxt`, and record `start_ts = GetCurrentTimestamp()`, a `getrusage(RUSAGE_SELF)` baseline, and the plan time if the `plannedstmt` pointer matches the stash. Keep the start info keyed by the `QueryDesc` pointer, in a small Vec stack. |
| `ExecutorRun` (cfg pg17/pg18) | nesting++ / try / finally nesting--. On `catch_others`, emit an error record (duration = now − start_ts, sqlstate from `CaughtError`), then rethrow (`e.rethrow()`). |
| `ExecutorFinish` | Same as Run. |
| `ExecutorEnd` | If tracked: `InstrEndLoop(totaltime)`, then build the record from `totaltime.total`, `bufusage`, `walusage`, `estate.es_processed`, `plannedstmt.queryId`, the rusage delta and the text. Push it, pop the start info, then chain. |
| `ProcessUtility` | Skip `EXECUTE`/`PREPARE`/`DEALLOCATE` (the executor already records them), as pgss does. If `pg_profiler.track_utility`, time the call with try/finally and catch errors as above. Rows come from `QueryCompletion.nprocessed`. Always chain. |

Query text: slice `sourceText` with `plannedstmt.stmt_location/stmt_len` (`-1`/`0` means
the whole string), so a multi-statement string records only its own statement. Store raw
text; normalisation is out of scope for v1. Every hook body is `#[pg_guard] unsafe
extern "C-unwind"` and always chains to the previous hook or `standard_*`.

### GUCs (`gucs.rs`)
| GUC | Type | Default | Context |
|---|---|---|---|
| `pg_profiler.enabled` | bool | on | SUSET |
| `pg_profiler.track` | enum top/all/none | top | SUSET |
| `pg_profiler.track_utility` | bool | on | SUSET |
| `pg_profiler.sample_rate` | real 0–1 | 1.0 | SUSET |
| `pg_profiler.min_duration_ms` | real ≥0 | 0 | SUSET (skip faster statements at push time) |
| `pg_profiler.buffer_size` | int 100–1 000 000 | 10 000 | POSTMASTER |
| `pg_profiler.max_query_len` | int 0–65 536 | 2 048 | POSTMASTER (0 = no text) |

### SQL surface (`sql.rs`, `extension_sql!`)
- `pg_profiler(limit int DEFAULT 1000) RETURNS TABLE(...)`. A `TableIterator` with
  `name!()` columns that returns the newest rows first. The `query` column shows
  `<insufficient privilege>` unless the caller is a superuser, a member of
  `pg_read_all_stats`, or the row's `userid` (the same rule as pgss).
- `pg_profiler_reset() RETURNS void`. `REVOKE ALL ... FROM PUBLIC`.
- `pg_profiler_info() RETURNS TABLE(capacity, total_written, retained, overwritten,
  max_query_len)`.
- Views: `pg_profiler_recent` (the last 100), `pg_profiler_slowest` (ordered by
  exec_time desc) and `pg_profiler_errors` (`WHERE is_error`).
- Calling the functions without `shared_preload_libraries` raises a clear `ERROR`, not a
  crash.

## Implementation order (each step ends green in Docker)
1. Commit the current scaffold plus the Docker changes.
2. `gucs.rs` and `_PG_init` with the preload guard. Test: `SHOW pg_profiler.track`.
3. `ring.rs`, `pg_profiler_info()` and `reset()`. Test: info shows the capacity.
4. ExecutorStart/End: record queryid, time, rows, buffers, WAL. Test: a `SELECT` shows
   up.
5. ExecutorRun/Finish with nesting, error capture and the parallel-worker skip. Test:
   a nested function call is recorded with `track=all` only, and `SELECT 1/0` gives an
   error row with sqlstate `22012`.
6. Planner timing, CPU rusage, text slicing and the privilege mask.
7. ProcessUtility. Test: `CREATE TABLE` and `ALTER TABLE` are recorded, and `EXECUTE`
   is not double-counted.
8. Sampling, `min_duration_ms` and the views.
9. README usage section and a GitHub Actions CI file (matrix pg17/pg18 running
   `cargo pgrx test` + `cargo clippy -D warnings` inside the Docker toolchain).

## Verification
- `docker compose run --rm builder cargo pgrx test pg18`, then `pg17`.
  - `pg_test::postgresql_conf_options()` must return
    `shared_preload_libraries='pg_profiler'`.
  - `#[pg_test]` statements run nested (inside the test function), so those tests set
    `pg_profiler.track = 'all'`.
- Top-level behaviour gets `pg_regress` tests under `tests/pg_regress/`
  (`cargo pgrx regress pg18`): sql/expected for a basic select, an error, DDL, nesting
  and reset. If regress doesn't preload the library, add it to
  `~/.pgrx/data-18/postgresql.conf` in the toolchain image.
- `docker compose run --rm builder cargo clippy --features pg18 -- -D warnings`.
- End-to-end: `docker compose up --build db`, then run psql
  `SELECT * FROM pg_profiler_recent;` after a few queries and
  `pgbench -i && pgbench -T 30 -c 8` against the container. Confirm there are no
  crashes, the row counts are sensible, and `pg_profiler_info().overwritten` grows once
  the ring wraps.

## As built: deviations from the design above

v1 is implemented as designed, apart from these changes (all found while
building; the text above is left as the original plan).

- **Preload guard.** `_PG_init` returns early, instead of raising an `ERROR`,
  when the library is loaded without `shared_preload_libraries` (e.g. by
  `CREATE EXTENSION`). The SQL functions then raise
  `pg_profiler must be loaded via shared_preload_libraries`, so the extension
  can still be created.
- **Planner errors.** `SELECT 1/0` is constant-folded and fails in the planner,
  so the executor hooks never see it. The planner hook now also records error
  rows (query text from `Query.stmt_location/stmt_len`). Errors from the
  planner and utility commands ignore `min_duration_ms`; planner errors also
  ignore sampling.
- **Sampling.** The decision is made at each top-level `ExecutorStart` /
  `ProcessUtility` entry. The planner times planning independently of it (the
  result is matched to the plan by pointer in `ExecutorStart`; cached plans
  report plan time 0).
- **Query text.** Clipped with `pg_mbcliplen` (safe for every server encoding,
  not just UTF-8) and trimmed of surrounding whitespace and `;` like
  `pg_stat_statements`, because the parser's statement range differs between
  PG 17 and 18.
- **Abort cleanup.** Transaction and subtransaction abort callbacks clear the
  per-backend start stack and nesting level, since `ExecutorEnd` never runs for
  statements that error out.
- **PostgreSQL 19.** Added after v1 as a third supported version (`pg19` feature).
  PG 19 changes the planner hook signature and the query instrumentation API and
  counts `instr_time` in CPU ticks; see "Phase H" in [ROADMAP.md](ROADMAP.md).
- **Utility.** `ProcessUtility` rows use `qc->nprocessed`; buffer/WAL usage comes
  from diffing `pgBufferUsage`/`pgWalUsage`. Like pgss, an untracked utility
  command (`EXECUTE`, `PREPARE`, `DEALLOCATE`, or `track_utility = off`) does
  not increment the nesting level.
- **SQL surface.** `pg_profiler(limit)` columns are `seq, start_time, userid,
  dbid, pid, queryid, cmd_type, toplevel, nesting, exec_time_ms, plan_time_ms,
  cpu_user_ms, cpu_sys_ms, rows, <buffer columns>, wal_*, is_error, sqlstate,
  query`. `pg_profiler_slowest`/`pg_profiler_errors` look at the whole ring.
- **Tooling.** `libc` is a direct dependency (`getrusage`). The toolchain image
  gained `clippy` and `rustfmt`. `pg_regress` runs through
  `scripts/regress.sh`, which adds `shared_preload_libraries` to the pgrx data
  directory's `postgresql.conf` first (the `~/.pgrx` volume hides changes made
  in the image). The compose `pgdata` volume is mounted at
  `/var/lib/postgresql`, which `postgres:18` requires.

## Phase 2 (after v1): OTLP trace export

Goal: ship each recorded execution as an OpenTelemetry span to an OTLP collector, without ever slowing backends.

### Design
- **Exporter background worker.** Register it in `_PG_init` with `pgrx::bgworkers::BackgroundWorkerBuilder` only when `pg_profiler.otlp_enabled` (POSTMASTER) is on. It connects to no database because it only reads shared memory. It sleeps on `BackgroundWorker::wait_latch(export_interval)`, reloads config on SIGHUP and exits on SIGTERM.
- **Ring cursor.** Add `export_cursor: u64` and the counters `exported`, `export_dropped` and `export_errors` (plus `last_export_error_at`) to `RingHeader`. Each cycle the worker copies up to `batch_size` slots from `export_cursor` to `head` under `LW_SHARED`. If the writer has lapped it (`head - export_cursor > capacity`), it skips forward and adds the gap to `export_dropped`. Backends never wait on the exporter. `pg_profiler_info()` exposes the new counters.
- **Span identity.** Add `trace_id: [u8;16]`, `span_id: [u8;8]` and `parent_span_id: [u8;8]` to `TraceRecord`. Backends assign them in ExecutorStart/ProcessUtility from the per-backend nesting stack:
  - A top-level statement gets a new random trace_id (`pg_prng`).
  - A nested statement inherits the trace_id and gets its parent's span_id as `parent_span_id`.
  - Later: honour an incoming W3C `traceparent` passed as a sqlcommenter comment (`/*traceparent='00-…'*/`) on the top-level statement, as pg_tracing does.
- **Span mapping.**
  - name = `db.operation.name` + target, or the command type.
  - start = `start_ts`, end = `start_ts + exec_time`.
  - status = ERROR when `is_error`.
  - Attributes follow the OTel DB semantic conventions: `db.system.name=postgresql`, `db.namespace` (database name), `db.operation.name`, `db.query.text` (only when `pg_profiler.otlp_include_query_text`), `db.response.status_code` (SQLSTATE). Add `pg_profiler.*` attributes for queryid, rows, plan_time, block and WAL counters, CPU, userid and pid.
  - Resource attributes: `service.name` (GUC) and `host.name`.
  - Planning becomes an optional child span.
- **Transport.** OTLP/HTTP with protobuf (`POST {endpoint}/v1/traces`). Use the prost types from the `opentelemetry-proto` crate and a blocking HTTP client (`ureq` with rustls). gzip is optional. No tonic/gRPC and no tokio runtime in v2, because the worker is single-threaded and batched. Retry with exponential backoff and cap the retries. On persistent failure, drop batches (count them) instead of buffering without limit.
- **GUCs.**
  - POSTMASTER: `pg_profiler.otlp_enabled` (bool, off).
  - SIGHUP: `pg_profiler.otlp_endpoint` (string, default `http://localhost:4318`), `pg_profiler.otlp_headers` (string, `k=v,k=v` for auth), `pg_profiler.otlp_service_name` (default `postgresql`), `pg_profiler.otlp_include_query_text` (bool, on), `pg_profiler.otlp_export_interval_ms` (1000), `pg_profiler.otlp_batch_size` (512), `pg_profiler.otlp_timeout_ms` (5000).

### Implementation order
1. TraceRecord span ids + nesting-stack propagation (tests: nested statement shares trace_id and links parent). Note: the v1 per-backend start stack (`backend::STARTS`) only holds executor statements; ProcessUtility entries are not pushed onto it, so utility commands must be added to the stack for their nested statements to find a parent span.
2. Ring export cursor + counters + `pg_profiler_info()` columns.
3. Background worker skeleton (latch loop, SIGHUP/SIGTERM, reads batches, logs counts).
4. OTLP protobuf mapping (unit-test the record → `ResourceSpans` conversion in plain Rust).
5. HTTP transport, retries, drop accounting.
6. Optional: `traceparent` sqlcommenter propagation.

### Verification
- Add an `otel-collector` service (otel/opentelemetry-collector-contrib with the `debug` exporter, or Jaeger all-in-one) to `docker-compose.yml`, and point `db` at it with `pg_profiler.otlp_enabled=on`.
- Run queries, nested functions and an error. Confirm in the collector output or the Jaeger UI that the spans have the correct parent/child links, timings, status and attributes.
- Stop the collector during `pgbench`. Backend latency must not change, and `export_dropped`/`export_errors` must rise and then recover when the collector returns.
