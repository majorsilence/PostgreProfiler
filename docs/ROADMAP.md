# pg_profiler roadmap: Profiler parity, a Query Store, and a GUI

Status: **plan only, nothing here is implemented.** v1 (per-execution ring buffer,
see [DESIGN.md](DESIGN.md)) is done. DESIGN.md also holds the OTLP export plan,
which this roadmap schedules as phase F.

## 1. Goal

Give Postgres the two things SQL Server users reach for:

1. **SQL Server Profiler**: a live, filterable stream of individual events
   (statements starting and completing, errors, deadlocks, lock waits, logins,
   plans), with saved traces and replay.
2. **Query Store**: durable per-query, per-plan, per-time-interval statistics
   that survive restarts, show plan changes and regressions, and (eventually)
   let you pin a good plan.

…and a **GUI** over both.

## 2. Research summary

Sources read for this plan: Microsoft's Query Store and automatic tuning docs,
the PostgreSQL 18 `pg_stat_statements` docs and the pg_stat_plans / PoWA material,
and the PostgreSQL lock-logging docs. Items marked *(from memory)* are not
re-verified and need a check before they are relied on.

**What Query Store actually is** (Microsoft docs): three stores (queries/plans,
runtime stats, wait stats) aggregated per *runtime-stats interval*, flushed
asynchronously to the user database, with capture modes (`ALL`/`AUTO`/`NONE`/`CUSTOM`),
`MAX_PLANS_PER_QUERY`, size-based and stale-query cleanup, wait-stats capture
grouped into wait *categories*, manual **plan forcing**, and **automatic plan
correction** (detect a regression after a plan change, force the last known good
plan, verify, un-force if it didn't help). Its built-in reports are: Regressed
Queries, Overall Resource Consumption, Top Resource Consuming Queries, Queries
With Forced Plans, Queries With High Variation, Query Wait Statistics, Tracked
Queries.

**What Postgres already gives us**
- `pg_stat_statements` (PG18 docs): per-query aggregates incl. plan time;
  a newer change adds generic/custom plan call counters (check which release).
  No history, no per-plan stats, no per-execution detail.
- `pg_stat_plans` (new, pganalyze) tracks plan shapes over time using plan IDs;
  it deliberately does not track per-plan execution stats. `pg_store_plans`
  and the older `pg_stat_plans` exist but are less maintained.
- `auto_explain` logs plans for slow statements, to the server log only.
- Lock waits: `log_lock_waits` logs after `deadlock_timeout`; deadlocks are
  reported as `40P01` errors whose detail text lists the cycle; live blocking
  is `pg_stat_activity` + `pg_locks`.
- `pg_wait_sampling` *(from memory)* samples wait events in a background worker.
- **PoWA** is the closest prior art for history + UI: `powa-archivist` (extension
  + collector), `powa-web` (UI), optional `powa-collector` for remote hosts,
  PostgreSQL licence. It builds on `pg_stat_statements`, so it has no
  per-execution events and no plan store.
- PG18 `PlannedStmt` already has a `planId` field, but PG17 does not, and core
  does not fill it, so we compute our own plan fingerprint on both.

**Hooks and APIs checked in the pgrx 0.20 bindings (PG17 and PG18):**
`emit_log_hook`, `ExplainOneQuery_hook`, `set_rel_pathlist_hook`,
`set_join_pathlist_hook`, `join_search_hook`, `NewExplainState` +
`ExplainPrintPlan`, `BuildParamLogString`, `ShmemInitHash`,
`before_shmem_exit`, `RecoveryInProgress`, `pgstat_get_wait_event`, `ProcGlobal`
(PGPROC wait info), `PgBackendStatus` (`st_query_id`, `st_appname`,
`st_clientaddr`), and pgrx `BackgroundWorker::{connect_worker_to_spi,
wait_latch, transaction}`. `ClientAuthentication_hook` was **not** found in the
generated bindings; it likely needs a hand-written `extern` declaration.

**Overall verdict:** everything in Profiler is achievable with hooks plus shared
memory. Query Store's storage and reports are achievable with a background
worker that writes ordinary tables. The only genuinely hard item is **plan
forcing**, because Postgres has no native mechanism. It is isolated in phase E
behind a go/no-go spike.

## 3. Feature map

### 3.1 SQL Server Profiler → pg_profiler

| Profiler feature | Plan | Phase |
|---|---|---|
| Duration, CPU, reads/writes, RowCounts | done in v1 | – |
| ApplicationName, HostName, LoginName, DatabaseName | store `application_name`, client address and resolved role/db names in the record | A |
| TransactionID / grouping by transaction | store `xid` (if assigned), `vxid`, transaction start time | A |
| Column filters (db, app, login, duration, text) | capture rules in shared memory evaluated in the hooks, plus client-side filters in the UI | A, D |
| `…Starting` events, "what is running now" | per-backend slot in shared memory, written lock-free at statement start/end | A |
| I/O time, lock/other waits per statement | `blk_read/write_time` fields; wait-category sample counters per statement | A, C |
| Exception / Attention (cancel) | errors already recorded; tag `57014` as cancel | A |
| Login / Logout / Audit | `ClientAuthentication_hook` (manual extern, to verify) + `before_shmem_exit` into an **events ring** | A |
| Deadlock graph | `emit_log_hook` on `40P01`: keep the raw detail text, parse the cycle best-effort, store in the events ring; UI draws the graph | A |
| Lock waits / timeouts | `emit_log_hook` on the `log_lock_waits` message and `55P03` | A |
| Showplan (XML / statistics) | plan capture with plan fingerprint, EXPLAIN text/JSON via `ExplainPrintPlan` | B |
| Sort/spill warnings, missing-stats warnings | per-node instrumentation on a sampled subset: spill flags, estimate-vs-actual ratio | B |
| Bind parameters (RPC events) | optional, off by default: `BuildParamLogString` | A (opt-in) |
| Live trace grid, column chooser, templates | GUI | D |
| Save / load trace | file export in the GUI; "save to table" via the archive | D, G |
| Replay | replay tool using captured text + parameters, preserving per-session ordering and timing | G |
| Tuning Advisor feed | out of scope (possible later: HypoPG-based index advice) | – |
| Perfmon correlation | out of scope for the extension; the GUI may overlay `pg_stat_*` series | – |

### 3.2 SQL Server Query Store → pg_profiler

| Query Store feature | Plan | Phase |
|---|---|---|
| Interval-aggregated runtime stats per query + plan | in-memory aggregate table keyed `(dbid, userid, queryid, plan_id, interval)` | C |
| Persisted across restart, flushed asynchronously | archiver background worker writes partitioned tables | C |
| Avg / min / max / stddev | counters incl. sum of squares; plus a log-scale latency histogram (gives p50/p95/p99, which Query Store lacks) | C |
| Plan store, `MAX_PLANS_PER_QUERY` | `plan` table, plan text/JSON, per-query cap | B, C |
| Wait stats by category | sampler background worker attributes wait samples to `(queryid, category)` | C |
| Capture modes `ALL/AUTO/NONE/CUSTOM` | `pg_profiler.capture_mode` with thresholds (min calls, min total time) | C |
| Size and stale-query cleanup | time-partitioned tables, `retention_days`, size cap, stale-query purge | C |
| Reports: Top consumers, Overall consumption, High variation, Tracked query | SQL functions/views + GUI screens | C, D |
| Regressed Queries | SQL function comparing a recent window with a baseline using sums/sumsq (Welch-style test) and plan-change detection | C |
| Plan comparison / plan change timeline | plan diff in the GUI | D |
| Query hints (Query Store hints) | per-queryid GUC overrides applied in `planner_hook` | E |
| Plan forcing + failure reasons | hint translation, then path-hook based forcing | E |
| Automatic plan correction | regression → force last good plan → verify → revert | E |
| Read-only mode when full | we purge old partitions instead; read-only mode as a safety fallback | C |
| Works on readable secondaries | memory-only on standbys; the UI collector can archive remotely | later |

## 4. Architecture

```
                 ┌────────────────────────── PostgreSQL ──────────────────────────┐
 backends ─hooks─▶ ring buffer   events ring   aggregate hash   plan ring  running│
                 │     ▲              ▲              ▲                ▲      slots │
                 │     │   wait sampler bgworker ─────┘                │           │
                 │     └── archiver bgworker ──SPI──▶ pg_profiler.* tables (history)│
                 └───────────────────────────┬────────────────────────────────────┘
                                  SQL API (versioned views/functions, read roles)
                                             │
                              ┌──────────────▼──────────────┐
                              │ pg_profiler UI server (Rust) │  axum + tokio-postgres
                              │  REST + SSE, auth, saved     │
                              │  traces, multi-instance      │
                              └──────────────┬──────────────┘
                                             │ static SPA embedded in the binary
                                        browser (and later a Tauri desktop shell)
```

### 4.1 Extension side

- **Generic ring.** Refactor `ring.rs` into a reusable ring so we can have several:
  the statement ring (v1), an **events ring** (login, logout, deadlock, lock wait,
  cancel, spill; fixed header + `detail` text) and a **plan ring** (new plans and
  slow-outlier plans, large payload, small capacity).
- **Running-now table.** One slot per backend (`MaxBackends`) holding the current
  top-level statement (start time, queryid, nesting depth, state, wait-category
  sample counters). Written only by its owner with a sequence counter, read
  without locks. This gives "Starting" events and a live activity view cheaply.
- **Plan fingerprint (`plan_id`).** Hash of the plan tree *shape* (node tags,
  relations, indexes, join/sort/agg strategies; no costs, row estimates or
  constants), computed from `PlannedStmt` and cached by plan pointer for
  prepared statements. Works the same on PG17 and PG18.
- **Plan text capture.** On first sight of a `(queryid, plan_id)` per interval,
  build the EXPLAIN text/JSON via `NewExplainState`/`ExplainPrintPlan` and push
  it to the plan ring. For slow or mis-estimated outliers, a sampled
  `plan_sample_rate` enables cheap per-node instrumentation (rows, no timing)
  so the stored plan includes actuals, as `auto_explain` does.
- **Wait sampler bgworker.** Every ~10 ms read each backend's wait event from
  `PGPROC` and `st_query_id`; increment the wait-category counter in the
  backend's running slot (copied into the record at statement end) and in the
  aggregate table. Statements shorter than the sample period show no waits;
  that resolution limit is documented.
- **Aggregate table.** A shared-memory hash (`ShmemInitHash`) of per-interval
  counters, updated at `ExecutorEnd` with a per-entry spinlock (the
  `pg_stat_statements` pattern). Fixed size (`stats_entries`); the oldest interval
  is evicted first and drops are counted.
- **Archiver bgworker.** Connects to an *archive database* (GUC, default the first
  database where the extension was created; central archive like PoWA rather than
  one store per database), closes finished intervals and writes them with SPI
  into time-partitioned tables. Retention drops old partitions. It is skipped
  on hot standbys.
- **Capture rules.** A small array in shared memory (database, role,
  `application_name` prefix, min duration, `LIKE`/prefix on text, error-only)
  checked in the hooks before a record is built; cheap fields first.
- **SQL API contract.** A versioned schema the GUI depends on, so other clients
  can use it too: `pg_profiler_since(after_seq, max_rows)` (oldest-first, reports
  when the cursor was lapped), `pg_profiler_running()`, `pg_profiler_events(...)`,
  `pg_profiler.runtime_stats` (persisted **and** the in-memory current interval),
  `pg_profiler.plans`, `pg_profiler.regressed_queries(...)`, `pg_profiler_version()`.
  Roles `profiler_reader` and `profiler_admin` are created by the extension.

### 4.2 GUI

**Recommendation: a standalone web application** (Rust server + single-page app),
shipped as a Docker image and added to `docker-compose.yml`.

- **Why not in the database:** an HTTP server inside a Postgres background worker
  is a stability and security risk and would fight the backend's single-threaded
  model. **Why not desktop-only:** Profiler is a desktop tool, but a web UI is
  reachable from containers and remote servers; the same SPA can be wrapped in
  Tauri later at small cost.
- **Server:** Rust, `axum` + `tokio-postgres`, REST for queries and Server-Sent
  Events for the live trace (fed by `pg_profiler_since`). Embeds the built SPA
  with `rust-embed`, so the deliverable is one binary. It lives in `ui/` as its
  own Cargo workspace so the pgrx crate's build flags do not leak into it.
- **Frontend (to confirm):** Vue 3 + TypeScript + Vite, charts with uPlot or
  ECharts. Vue is chosen mainly so the execution plan viewer can reuse
  [pev2](https://github.com/dalibo/pev2) *(from memory: Dalibo's Vue plan
  visualiser, PostgreSQL licence; verify licence and fit)* instead of writing a
  plan renderer from scratch.
- **Auth and safety:** the UI connects with a read-only role; admin actions
  (reset, change capture rules, cancel backend, force plan) need the admin role
  and are confirmed in the UI. The UI has its own login (local users first, OIDC
  later), TLS, and never logs query text or parameters. Parameter capture is off
  by default and flagged as sensitive.
- **Multi-instance:** a saved-connections page (credentials encrypted at rest).
  Single instance is enough for the first release.

**Screens**

| Area | Screen | Source |
|---|---|---|
| Profiler | **Live Trace**: streaming grid, column chooser, per-column filters, pause/resume, find, group-by (query / app / user / session), detail pane with full text, plan and waits, saved templates, export | `pg_profiler_since`, events |
| Profiler | **Running Now**: running statements with elapsed time and waits, blocking tree, cancel/terminate (admin) | `pg_profiler_running`, `pg_locks` |
| Profiler | **Events**: deadlock graph, lock-wait timeline, login/logout/auth failures, errors by SQLSTATE | events ring + archive |
| Query Store | **Overall Resource Consumption**, **Top Resource Consumers** (metric picker, drill-down to plans) | `runtime_stats` |
| Query Store | **Regressed Queries**, **High Variation**, **Queries With Forced Plans** | `regressed_queries`, plans |
| Query Store | **Tracked Query**: time series per plan, plan picker, plan compare/diff | `runtime_stats`, `plans` |
| Query Store | **Wait Statistics** by category over time and per query | wait stats |
| Shared | **Plan viewer** (graphical plan, estimates vs actual, spill and misestimate flags) | plan text/JSON |
| Shared | **Settings & health**: capture rules, retention, buffer fill, dropped counters, extension version | `pg_profiler_info`, GUCs |

## 5. Phases

Sizes are relative (S ≈ days, M ≈ 1–2 weeks, L ≈ several weeks, XL = spike plus
build). Every phase ends with tests green on PG17 and PG18 and an overhead check.

### Phase A: richer trace and live visibility  (M–L) — built

**Status: implemented** on PG17 and PG18 (see "Phase A as built" below for how it
differs from this plan).

1. Refactor to a generic ring; add the **events ring**.
2. Session context fields (`application_name`, client address, role and database
   names), `xid`/`vxid`/transaction start, I/O timing. Check the record size and
   update the memory estimate in the README.
3. **Running-now table** and `pg_profiler_running()`.
4. `emit_log_hook` events: deadlock (raw detail + best-effort parsed cycle),
   lock wait, cancel; a recursion guard, and never `ereport` from the hook.
5. Login/logout/auth-failure events (verify or hand-declare
   `ClientAuthentication_hook`).
6. **Capture rules** (shared-memory array, SQL functions to set them).
7. `pg_profiler_since()` cursor API, roles, versioned API views.
8. Opt-in bind-parameter capture.
- *Tests:* pg_test + regress for every new event; a two-session deadlock test
  (needs a second connection, so use a TAP-style script or `pgrx` multi-client
  test); overhead benchmark with `pgbench` (budget: under 3 % TPS loss at defaults).

#### Phase A as built

Everything in items 1–8 above exists, with these differences:

- **No `ClientAuthentication_hook`.** `Port` is opaque in the pgrx bindings, so
  the hook could not read the user or database. Auth failures come from
  `emit_log_hook` (SQLSTATE class 28). A `login` event is written when a backend
  runs its first statement (the earliest point where the user and database are
  known), and `logout` from a `before_shmem_exit` callback.
- **Capture rules** are evaluated when a record is pushed, not at statement
  start, so they reduce ring noise but not instrumentation overhead.
- **`pg_profiler()` lost `toplevel`, `local_blks_hit` and `local_blks_read`.**
  pgrx table functions support at most 32 columns and the new fields needed the
  room. `toplevel` is `nesting = 0` (added back in the views); the local-block
  counters are still stored in the record and are exposed again when the API
  moves to a wider return type. This is API version 1.
- **Role names** are `profiler_reader` and `profiler_admin`: Postgres reserves
  the `pg_` prefix. Members of `profiler_reader` see all query text.
- **Running table** keeps only the top-level statement per backend (with a seqlock
  so readers never block); `pg_profiler_activity` joins it with `pg_stat_activity`
  for waits and `pg_blocking_pids`.
- **Lock waits** need `log_lock_waits = on`; the events are matched on the English
  message text. Verified by hand against real sessions: deadlock (full cycle with
  both statements), lock wait/acquired with durations, cancel, terminate and auth
  failure. Deadlocks and lock waits need two connections, so they are not in the
  automated suite; cancel, login, rules, roles, running, `since`, parameters and
  session context are.
- **Overhead:** on a Docker Desktop box `pgbench -S` showed no difference beyond
  run-to-run noise (about ±15 %), so the "under 3 %" budget is not yet verified;
  that needs a quiet Linux host.

### Phase B: plans and diagnostics  (L)
1. `plan_id` fingerprint on PG17 and PG18 (tests: same shape → same id,
   index change → different id, constants ignored).
2. Plan ring and plan text/JSON capture for new plans.
3. Sampled instrumentation for outliers; record spill, estimate-vs-actual ratio.
4. `plan_id` added to each statement record.
- *Risk:* walking `Plan` trees across versions. Mitigation: one small walker with
  per-version `cfg` shims, covered by golden-plan tests.

### Phase C: Query Store core  (XL)
1. Aggregate hash with counters, sum of squares, log histogram; capture modes.
2. Archiver bgworker, archive-database selection, schema and partitioning,
   retention and size cap, stale-query purge, `pg_profiler.runtime_stats` view
   that merges persisted and in-memory data.
3. Wait sampler bgworker and wait categories.
4. Report functions: top consumers, overall consumption, high variation,
   `regressed_queries` (statistical comparison + plan-change flag).
5. Extension upgrade scripts (`ALTER EXTENSION UPDATE`) from now on: this is the
   first phase with persistent user data, so schema migrations are mandatory.
- *Tests:* restart-survival test, retention test, a synthetic regression
  (change a plan by dropping an index) detected by `regressed_queries`.
- *Risk:* write amplification of the archiver. Mitigation: batch per interval,
  unlogged option, measured with `pgbench`.

### Phase D: GUI  (XL, can start after phase A's API contract exists)
1. **D1 (after A):** server skeleton, auth, Live Trace, Running Now, Events,
   Settings, Docker image and compose service, a demo stack with a workload
   generator.
2. **D2 (after C):** Query Store screens, plan viewer and plan diff.
3. Saved templates and trace export (file); Playwright end-to-end tests against
   the compose stack; accessibility and keyboard basics.

### Phase E: hints and plan forcing  (XL, starts with a spike)
- **E0 spike, go/no-go:** forcing a specific plan in Postgres has no native
  support, so decide between: (a) per-queryid **GUC overrides** (`enable_*`,
  `join_collapse_limit`, `work_mem`…) applied in `planner_hook` (robust,
  coarse, equals Query Store *hints*); (b) **path hooks**
  (`set_rel_pathlist_hook`, `set_join_pathlist_hook`, `join_search_hook`) that
  prune paths to match a stored plan's scan/join methods and join order, a
  mini pg_hint_plan; (c) integrating the existing `pg_hint_plan` extension when
  installed. Go criteria: reproduces a stored plan for the test corpus
  (including after statistics change), with clear failure reasons.
- E1 hints (a), E2 forcing (b/c) with failure tracking and un-force, E3 automatic
  plan correction (detect → force last good → verify → revert), E4 GUI controls.
- *Risk:* highest in the roadmap. If the spike fails, ship (a) only and say so.

### Phase F: OTLP export  (M)
Already specified in [DESIGN.md](DESIGN.md) "Phase 2". Independent of A–E apart
from using the span ids it adds to the record; schedule it whenever useful.

### Phase G: persistence extras and replay  (L)
Save traces to a table, load them back into the GUI, and a **replay** tool
(captured text plus parameters, per-session ordering, speed factor, target
server). Standby support (memory-only on standbys, UI-side archive).

### Phase H: PostgreSQL 19 support  (M)
PostgreSQL 19 is released. pgrx 0.20 already ships PG19 bindings, so the work is:
add a `pg19` cargo feature, install PG19 in the toolchain image and the CI matrix,
and adapt anything that changed in core. Things to check first: the
`ExecutorStart` hook signature (I expect it to return a value in 19, which would
need another `cfg` branch like `ExecutorRun` has for 17/18), `ExecutorRun`,
`PlannedStmt.planId` (possibly filled by core in 19, which could replace our own
plan fingerprint in phase B), the `Instrumentation`/`BufferUsage`/`WalUsage`
layouts, and the `instr_time` representation. Do it before phase B so the plan
code is written against all three versions.

### Suggested order
`A → H → D1 (parallel with B) → C → D2 → E0 → E1/E2/E3 → G`, with F slotted in
wherever OTLP is wanted. A is the best next step: it is small, it closes most of
the Profiler gap, and it unblocks the GUI.

## 6. Cross-cutting concerns

- **Overhead budget:** every phase measures `pgbench` TPS and p99 latency with
  the feature on versus off; budget under 3 % at defaults. Features that cost
  more (parameters, plan instrumentation, wait sampling period) are opt-in or
  sampled.
- **Memory:** all shared memory is sized at start from GUCs and documented in
  the README (`buffer_size`, `events_size`, `stats_entries`, `plan_ring_size`).
- **Privacy:** query text and parameters can contain personal data. Keep the
  `pg_stat_statements` visibility rule, mask parameters unless explicitly
  enabled, and document that text is not normalised. A normalisation option
  (literal stripping) is worth adding before the archive stores text long term.
- **Compatibility:** PG17 and PG18 stay in the CI matrix; PG19 is phase H.
- **Co-existence:** keep using the core `queryid` so rows join to
  `pg_stat_statements`; document running both.
- **Packaging:** Docker images for the extension and the UI; compose demo
  with a workload generator; later `.deb`/`.rpm` for the extension.
- **Testing:** pgrx tests and `pg_regress` per feature; two-session tests for
  locks and deadlocks; UI unit tests plus Playwright end-to-end; a benchmark
  harness in CI that fails on large regressions.

## 7. Decisions to confirm

Defaults used above are in brackets; say if any should change.

1. **GUI form** [standalone web app, Tauri wrapper later] vs desktop-first.
2. **Frontend stack** [Vue 3 + TypeScript + Vite, to reuse pev2].
3. **Archive location** [one central archive database] vs a store per database
   like Query Store.
4. **Plan forcing depth** [spike first; ship hints, then forcing only if the
   spike passes].
5. **Bind-parameter capture** [opt-in, off by default] and whether to add literal
   normalisation before archiving query text.
6. **Multi-instance in the first GUI release** [no, single instance first].
7. **Licensing of UI dependencies** [must be compatible with the four project
   licences; check each dependency, starting with pev2].
