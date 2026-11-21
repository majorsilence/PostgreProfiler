//! The archiver background worker: copies closed statistics intervals, query
//! texts and plans from shared memory into tables of the archive database, and
//! applies the retention policy.
//!
//! It connects to one database (`pg_profiler.archive_database`, which needs
//! `CREATE EXTENSION pg_profiler`), so the history of the whole instance is
//! central rather than one store per database. It is only started once
//! recovery has finished, so standbys keep their statistics in memory only.

use crate::gucs;
use crate::record::{plan_reason, PlanRecord};
use crate::ring;
use crate::stats::{self, Row, Which};
use pgrx::bgworkers::{BackgroundWorker, BackgroundWorkerBuilder, SignalWakeFlags};
use pgrx::pg_sys::panic::CaughtError;
use pgrx::prelude::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
use std::time::Duration;

/// Seconds an interval must be closed before it is archived, so statements
/// that were finishing at the boundary are counted.
const GRACE_US: i64 = 2_000_000;
const POLL: Duration = Duration::from_secs(5);
/// How often the retention policy runs.
const PURGE_EVERY_US: i64 = 3_600_000_000;

pub fn register() {
    BackgroundWorkerBuilder::new("pg_profiler archiver")
        .set_type("pg_profiler archiver")
        .set_library("pg_profiler")
        .set_function("pg_profiler_archiver_main")
        .enable_spi_access()
        .set_restart_time(Some(Duration::from_secs(30)))
        .load();
}

fn log(msg: &str) {
    pgrx::log!("pg_profiler archiver: {msg}");
}

fn describe(e: &CaughtError) -> String {
    match e {
        CaughtError::PostgresError(r) | CaughtError::ErrorReport(r) => r.message().to_string(),
        CaughtError::RustPanic { ereport, .. } => ereport.message().to_string(),
    }
}

/// Run `f` in its own transaction. Errors are caught, the transaction is
/// aborted and the message returned.
fn in_transaction<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    unsafe {
        pg_sys::SetCurrentStatementStartTimestamp();
        pg_sys::StartTransactionCommand();
        pg_sys::PushActiveSnapshot(pg_sys::GetTransactionSnapshot());
    }
    let result = PgTryBuilder::new(AssertUnwindSafe(|| Ok(f())))
        .catch_others(|e| Err(describe(&e)))
        .execute();
    unsafe {
        if result.is_ok() {
            pg_sys::PopActiveSnapshot();
            pg_sys::CommitTransactionCommand();
        } else {
            pg_sys::FlushErrorState();
            pg_sys::AbortCurrentTransaction();
        }
    }
    result
}

#[no_mangle]
#[pg_guard]
pub extern "C-unwind" fn pg_profiler_archiver_main(_arg: pg_sys::Datum) {
    BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGHUP | SignalWakeFlags::SIGTERM);
    let db = gucs::ARCHIVE_DATABASE.get().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    let db = if db.is_empty() { "postgres".to_string() } else { db };
    BackgroundWorker::connect_worker_to_spi(Some(&db), None);
    log(&format!("started, archive database \"{db}\""));

    let mut state = State::default();
    while BackgroundWorker::wait_latch(Some(POLL)) {
        if BackgroundWorker::sighup_received() {
            unsafe { pg_sys::ProcessConfigFile(pg_sys::GucContext::PGC_SIGHUP) };
        }
        if !gucs::ARCHIVE.get() {
            continue;
        }
        cycle(&mut state, false);
    }
    // Shutting down: keep what the interval in progress has collected.
    if gucs::ARCHIVE.get() {
        cycle(&mut state, true);
    }
    log("stopped");
}

#[derive(Default)]
struct State {
    /// Schema the extension is installed in (the archive database has it).
    schema: Option<String>,
    last_plan_seq: i64,
    last_purge_ts: i64,
    last_warning_ts: i64,
}

fn now() -> i64 {
    unsafe { pg_sys::GetCurrentTimestamp() }
}

fn warn_rarely(state: &mut State, msg: &str) {
    if now() - state.last_warning_ts > 600_000_000 {
        state.last_warning_ts = now();
        log(msg);
    }
}

/// Make the extension's tables resolvable. False if the archive database does
/// not have the extension.
fn prepare(state: &mut State) -> bool {
    if state.schema.is_some() {
        return true;
    }
    let schema = in_transaction(|| {
        Spi::get_one::<String>(
            "SELECT n.nspname::text FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace \
             WHERE e.extname = 'pg_profiler'",
        )
        .ok()
        .flatten()
    });
    match schema {
        Ok(Some(s)) => {
            state.schema = Some(s);
            true
        }
        Ok(None) => {
            warn_rarely(state, "the extension is not installed in the archive database; nothing is archived");
            false
        }
        Err(e) => {
            warn_rarely(state, &format!("cannot read the archive database: {e}"));
            false
        }
    }
}

fn cycle(state: &mut State, final_flush: bool) {
    if unsafe { pg_sys::RecoveryInProgress() } {
        return;
    }
    if !prepare(state) {
        return;
    }
    let interval_us = stats::interval_us();
    if interval_us <= 0 {
        return;
    }

    let which = if final_flush { Which::Unarchived } else { Which::ClosedUnarchived };
    let mut rows: Vec<Row> = stats::snapshot(which);
    if !final_flush {
        let t = now();
        rows.retain(|r| t - (r.interval_no as i64 + 1) * interval_us >= GRACE_US);
    }
    let mut by_interval: BTreeMap<u64, Vec<Row>> = BTreeMap::new();
    for r in rows {
        by_interval.entry(r.interval_no).or_default().push(r);
    }
    for (n, rows) in by_interval {
        match archive_interval(state, n, interval_us, &rows) {
            Ok(()) => stats::mark_archived(n),
            Err(e) => {
                log(&format!("storing interval {n} failed: {e}"));
                break;
            }
        }
    }

    archive_plans(state);

    let t = now();
    if !final_flush && t - state.last_purge_ts > PURGE_EVERY_US {
        state.last_purge_ts = t;
        purge(state);
    }
}

/// Resolve the extension's tables for the current transaction.
fn set_search_path(schema: &str) -> Result<(), String> {
    Spi::run_with_args(
        "SELECT set_config('search_path', $1, true)",
        &[format!("{}, pg_catalog", quote_ident(schema)).into()],
    )
    .map_err(|e| e.to_string())
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn f(v: f64) -> Value {
    if v.is_finite() {
        json!(v)
    } else {
        json!(0.0)
    }
}

fn stats_json(r: &Row) -> Value {
    let d = &r.data;
    let ms = |us: f64| f(us / 1000.0);
    json!({
        "dbid": r.dbid, "userid": r.userid, "queryid": r.queryid, "plan_id": r.plan_id,
        "calls": d.calls, "errors": d.errors,
        "exec_total_ms": ms(d.exec_sum_us), "exec_sumsq_ms2": f(d.exec_sumsq_us / 1.0e6),
        "exec_min_ms": if d.calls == 0 { f(0.0) } else { ms(d.exec_min_us) },
        "exec_max_ms": ms(d.exec_max_us), "plan_total_ms": ms(d.plan_sum_us),
        "rows": d.rows,
        "shared_blks_hit": d.shared_blks_hit, "shared_blks_read": d.shared_blks_read,
        "shared_blks_dirtied": d.shared_blks_dirtied, "shared_blks_written": d.shared_blks_written,
        "temp_blks_read": d.temp_blks_read, "temp_blks_written": d.temp_blks_written,
        "wal_records": d.wal_records, "wal_fpi": d.wal_fpi, "wal_bytes": d.wal_bytes,
        "cpu_user_ms": ms(d.cpu_user_us as f64), "cpu_sys_ms": ms(d.cpu_sys_us as f64),
        "blk_read_ms": ms(d.blk_read_us as f64), "blk_write_ms": ms(d.blk_write_us as f64),
        "hist": d.hist.to_vec(), "wait_samples": d.waits.to_vec(),
    })
}

const INSERT_STATS: &str = "
INSERT INTO pg_profiler_hist_stats
       (interval_start, dbid, userid, queryid, plan_id, calls, errors,
        exec_total_ms, exec_sumsq_ms2, exec_min_ms, exec_max_ms, plan_total_ms, rows,
        shared_blks_hit, shared_blks_read, shared_blks_dirtied, shared_blks_written,
        temp_blks_read, temp_blks_written, wal_records, wal_fpi, wal_bytes,
        cpu_user_ms, cpu_sys_ms, blk_read_ms, blk_write_ms, hist, wait_samples)
SELECT $1::timestamptz, x.dbid, x.userid, x.queryid, x.plan_id, x.calls, x.errors,
       x.exec_total_ms, x.exec_sumsq_ms2, x.exec_min_ms, x.exec_max_ms, x.plan_total_ms, x.rows,
       x.shared_blks_hit, x.shared_blks_read, x.shared_blks_dirtied, x.shared_blks_written,
       x.temp_blks_read, x.temp_blks_written, x.wal_records, x.wal_fpi, x.wal_bytes,
       x.cpu_user_ms, x.cpu_sys_ms, x.blk_read_ms, x.blk_write_ms, x.hist, x.wait_samples
  FROM jsonb_to_recordset($2::jsonb) AS x(
       dbid oid, userid oid, queryid bigint, plan_id bigint, calls bigint, errors bigint,
       exec_total_ms float8, exec_sumsq_ms2 float8, exec_min_ms float8, exec_max_ms float8,
       plan_total_ms float8, rows bigint,
       shared_blks_hit bigint, shared_blks_read bigint, shared_blks_dirtied bigint,
       shared_blks_written bigint, temp_blks_read bigint, temp_blks_written bigint,
       wal_records bigint, wal_fpi bigint, wal_bytes bigint,
       cpu_user_ms float8, cpu_sys_ms float8, blk_read_ms float8, blk_write_ms float8,
       hist int4[], wait_samples int4[])
ON CONFLICT DO NOTHING";

const UPSERT_QUERIES: &str = "
INSERT INTO pg_profiler_hist_queries AS q (dbid, queryid, query, first_seen, last_seen)
SELECT x.dbid, x.queryid, x.query, $1::timestamptz, $1::timestamptz
  FROM jsonb_to_recordset($2::jsonb) AS x(dbid oid, queryid bigint, query text)
ON CONFLICT (dbid, queryid) DO UPDATE
   SET last_seen = GREATEST(q.last_seen, EXCLUDED.last_seen),
       first_seen = LEAST(q.first_seen, EXCLUDED.first_seen),
       query = EXCLUDED.query";

const TOUCH_PLANS: &str = "
UPDATE pg_profiler_hist_plans p
   SET last_seen = GREATEST(p.last_seen, $1::timestamptz),
       first_seen = LEAST(p.first_seen, $1::timestamptz)
  FROM jsonb_to_recordset($2::jsonb) AS x(dbid oid, queryid bigint, plan_id bigint)
 WHERE p.dbid = x.dbid AND p.queryid = x.queryid AND p.plan_id = x.plan_id";

fn ts_arg(us: i64) -> pgrx::datum::DatumWithOid<'static> {
    TimestampWithTimeZone::try_from(us).unwrap_or_else(|_| TimestampWithTimeZone::try_from(0).unwrap()).into()
}

fn archive_interval(state: &mut State, n: u64, interval_us: i64, rows: &[Row]) -> Result<(), String> {
    let schema = state.schema.clone().unwrap_or_default();
    let start_ts = n as i64 * interval_us;
    // distinct queries and plans, with their cached text
    let keys: BTreeSet<(u32, i64)> = rows.iter().map(|r| (r.dbid, r.queryid)).collect();
    let plan_keys: BTreeSet<(u32, i64, i64)> =
        rows.iter().filter(|r| r.plan_id != 0).map(|r| (r.dbid, r.queryid, r.plan_id)).collect();
    let queries: Vec<Value> = keys
        .iter()
        .filter_map(|&(dbid, queryid)| {
            stats::text_get(dbid, queryid).map(|t| {
                json!({"dbid": dbid, "queryid": queryid, "query": String::from_utf8_lossy(&t)})
            })
        })
        .collect();
    let plans: Vec<Value> =
        plan_keys.iter().map(|&(d, q, p)| json!({"dbid": d, "queryid": q, "plan_id": p})).collect();
    let stats_rows = Value::Array(rows.iter().map(stats_json).collect()).to_string();
    let queries = Value::Array(queries).to_string();
    let plans = Value::Array(plans).to_string();

    in_transaction(|| -> Result<(), String> {
        set_search_path(&schema)?;
        let run = |sql: &str, args: &[pgrx::datum::DatumWithOid]| Spi::run_with_args(sql, args).map_err(|e| e.to_string());
        run("SELECT pg_profiler_ensure_partition($1::timestamptz)", &[ts_arg(start_ts)])?;
        run(INSERT_STATS, &[ts_arg(start_ts), stats_rows.as_str().into()])?;
        run(UPSERT_QUERIES, &[ts_arg(start_ts), queries.as_str().into()])?;
        run(TOUCH_PLANS, &[ts_arg(start_ts), plans.as_str().into()])?;
        Ok(())
    })
    .and_then(|r| r)
}

const INSERT_PLANS: &str = "
INSERT INTO pg_profiler_hist_plans AS p (dbid, queryid, plan_id, first_seen, last_seen, plan, format)
SELECT x.dbid, x.queryid, x.plan_id, t.ts, t.ts, x.plan, x.format
  FROM jsonb_to_recordset($1::jsonb) AS x(dbid oid, queryid bigint, plan_id bigint, ts_us bigint, plan text, format text),
       LATERAL (SELECT timestamptz '2000-01-01 00:00:00+00' + x.ts_us * interval '1 microsecond' AS ts) t
ON CONFLICT (dbid, queryid, plan_id) DO UPDATE
   SET last_seen = GREATEST(p.last_seen, EXCLUDED.last_seen)";

const INSERT_SAMPLES: &str = "
INSERT INTO pg_profiler_hist_plan_samples
       (ts, dbid, queryid, plan_id, reason, exec_time_ms, max_misestimate, temp_blks_written, plan, format)
SELECT t.ts, x.dbid, x.queryid, x.plan_id, x.reason, x.exec_time_ms, x.max_misestimate, x.temp_blks_written,
       x.plan, x.format
  FROM jsonb_to_recordset($1::jsonb) AS x(ts_us bigint, dbid oid, queryid bigint, plan_id bigint, reason text,
       exec_time_ms float8, max_misestimate float8, temp_blks_written bigint, plan text, format text),
       LATERAL (SELECT timestamptz '2000-01-01 00:00:00+00' + x.ts_us * interval '1 microsecond' AS ts) t
ON CONFLICT DO NOTHING";

fn plan_json(r: &PlanRecord, payload: &[u8]) -> Value {
    let (plan, _) = payload.split_at((r.plan_len as usize).min(payload.len()));
    json!({
        "ts_us": r.ts, "dbid": r.dbid, "queryid": r.queryid, "plan_id": r.plan_id,
        "reason": plan_reason::names(r.reasons),
        "exec_time_ms": f(r.exec_time_us / 1000.0), "max_misestimate": f(r.max_misestimate),
        "temp_blks_written": r.temp_blks_written,
        "plan": String::from_utf8_lossy(plan),
        "format": if r.format == 1 { "json" } else { "text" },
    })
}

/// Copy new entries of the plan ring into the plan tables: plans seen for the
/// first time go to the plan table, plans captured with actual rows to the
/// samples table.
fn archive_plans(state: &mut State) {
    let ring = ring::plan_ring();
    if !ring.is_ready() {
        return;
    }
    let new = ring.since(state.last_plan_seq - 1, 200);
    let Some(last_seq) = new.last().map(|(r, _)| r.seq as i64 + 1) else { return };
    let mut first_rows = Vec::new();
    let mut sample_rows = Vec::new();
    for (r, payload) in &new {
        if r.analyzed {
            sample_rows.push(plan_json(r, payload));
        } else {
            first_rows.push(plan_json(r, payload));
        }
    }
    let schema = state.schema.clone().unwrap_or_default();
    let first_rows = Value::Array(first_rows).to_string();
    let sample_rows = Value::Array(sample_rows).to_string();

    let result = in_transaction(|| -> Result<(), String> {
        set_search_path(&schema)?;
        Spi::run_with_args(INSERT_PLANS, &[first_rows.as_str().into()]).map_err(|e| e.to_string())?;
        Spi::run_with_args(INSERT_SAMPLES, &[sample_rows.as_str().into()]).map_err(|e| e.to_string())?;
        Ok(())
    });
    match result.and_then(|r| r) {
        Ok(()) => state.last_plan_seq = last_seq,
        Err(e) => log(&format!("storing plans failed: {e}")),
    }
}

fn purge(state: &State) {
    let schema = state.schema.clone().unwrap_or_default();
    let days = gucs::RETENTION_DAYS.get();
    let mb = i64::from(gucs::MAX_ARCHIVE_SIZE_MB.get());
    let per_query = gucs::MAX_PLANS_PER_QUERY.get();
    let r = in_transaction(|| -> Result<(), String> {
        set_search_path(&schema)?;
        Spi::run_with_args(
            "SELECT * FROM pg_profiler_purge($1, $2, $3)",
            &[days.into(), mb.into(), per_query.into()],
        )
        .map_err(|e| e.to_string())
    });
    if let Err(e) = r.and_then(|r| r) {
        log(&format!("retention failed: {e}"));
    }
}
