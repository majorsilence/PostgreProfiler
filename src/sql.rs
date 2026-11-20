//! SQL-facing functions and views.

use crate::record::{c_bytes, cmd, copy_into, event, plan_reason, EventRecord, PlanRecord, TraceRecord};
use crate::{gucs, ring, rules, running};
use pgrx::prelude::*;

/// Version of the SQL surface that clients (the GUI) can rely on. Bumped on
/// incompatible changes to the columns of the functions and views below.
const API_VERSION: i32 = 2;

#[pg_extern]
fn pg_profiler_api_version() -> i32 {
    API_VERSION
}

#[pg_extern]
fn pg_profiler_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[pg_extern]
#[allow(clippy::type_complexity)]
fn pg_profiler_info() -> TableIterator<
    'static,
    (
        name!(capacity, i64),
        name!(total_written, i64),
        name!(retained, i64),
        name!(overwritten, i64),
        name!(max_query_len, i32),
        name!(oldest_seq, i64),
        name!(next_seq, i64),
        name!(events_capacity, i64),
        name!(events_total_written, i64),
        name!(events_retained, i64),
        name!(events_overwritten, i64),
        name!(plans_capacity, i64),
        name!(plans_total_written, i64),
        name!(plans_retained, i64),
        name!(plans_overwritten, i64),
    ),
> {
    ring::ensure_ready();
    let s = ring::stmt_ring().stats();
    let e = ring::event_ring().stats();
    let p = ring::plan_ring().stats();
    TableIterator::once((
        s.capacity,
        s.total_written,
        s.retained,
        s.overwritten,
        gucs::MAX_QUERY_LEN.get(),
        s.oldest_seq,
        s.next_seq,
        e.capacity,
        e.total_written,
        e.retained,
        e.overwritten,
        if gucs::MAX_PLAN_LEN.get() > 0 { p.capacity } else { 0 },
        p.total_written,
        p.retained,
        p.overwritten,
    ))
}

/// Forget every statement and event recorded so far.
#[pg_extern]
fn pg_profiler_reset() {
    ring::ensure_ready();
    ring::stmt_ring().reset();
    ring::event_ring().reset();
    ring::plan_ring().reset();
    ring::plan_seen_clear();
}

/// Who may see query text and parameters of other users' statements:
/// superusers, members of `pg_read_all_stats` (the `pg_stat_statements` rule)
/// and members of `profiler_reader`.
struct Visibility {
    me: pg_sys::Oid,
    sees_all: bool,
}

fn has_role(me: pg_sys::Oid, name: &std::ffi::CStr) -> bool {
    unsafe {
        let role = pg_sys::get_role_oid(name.as_ptr(), true);
        role != pg_sys::Oid::INVALID && pg_sys::has_privs_of_role(me, role)
    }
}

fn visibility() -> Visibility {
    unsafe {
        let me = pg_sys::GetUserId();
        let sees_all = pg_sys::superuser()
            || pg_sys::has_privs_of_role(me, pg_sys::Oid::from(pg_sys::ROLE_PG_READ_ALL_STATS))
            || has_role(me, c"profiler_reader");
        Visibility { me, sees_all }
    }
}

const MASKED: &str = "<insufficient privilege>";

fn text_of(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
}

fn cstr_opt(buf: &[u8]) -> Option<String> {
    text_of(c_bytes(buf))
}

fn ts(v: i64) -> Option<TimestampWithTimeZone> {
    if v == 0 {
        None
    } else {
        TimestampWithTimeZone::try_from(v).ok()
    }
}

fn name_of(p: *const std::ffi::c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

#[allow(clippy::type_complexity)]
type StmtRow = (
    i64,
    Option<TimestampWithTimeZone>,
    Option<TimestampWithTimeZone>,
    pg_sys::Oid,
    pg_sys::Oid,
    i32,
    Option<String>,
    Option<String>,
    Option<i64>,
    i64,
    i64,
    String,
    i32,
    f64,
    f64,
    f64,
    f64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    f64,
    f64,
    i64,
    i64,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn stmt_row(vis: &Visibility, (r, payload): (TraceRecord, Vec<u8>)) -> StmtRow {
    let q_len = (r.query_len as usize).min(payload.len());
    let (query, params) = payload.split_at(q_len);
    let visible = vis.sees_all || r.userid == vis.me.to_u32();
    let (query, params) = if visible {
        (text_of(query), text_of(params))
    } else {
        (Some(MASKED.to_string()), text_of(params).map(|_| MASKED.to_string()))
    };
    (
        r.seq as i64,
        ts(r.start_ts),
        ts(r.xact_start),
        pg_sys::Oid::from(r.userid),
        pg_sys::Oid::from(r.dbid),
        r.pid,
        cstr_opt(&r.application_name),
        cstr_opt(&r.client_addr),
        if r.xid == 0 { None } else { Some(i64::from(r.xid)) },
        r.queryid,
        r.plan_id,
        cmd::name(r.cmd_type).to_string(),
        r.nesting,
        r.exec_time_us / 1000.0,
        r.plan_time_us / 1000.0,
        r.cpu_user_us as f64 / 1000.0,
        r.cpu_sys_us as f64 / 1000.0,
        r.rows as i64,
        r.shared_blks_hit,
        r.shared_blks_read,
        r.shared_blks_dirtied,
        r.shared_blks_written,
        r.temp_blks_read,
        r.temp_blks_written,
        r.blk_read_time_us as f64 / 1000.0,
        r.blk_write_time_us as f64 / 1000.0,
        r.wal_records,
        r.wal_fpi,
        r.wal_bytes as i64,
        if r.is_error { cstr_opt(&r.sqlstate) } else { None },
        query,
        params,
    )
}

/// The newest `lim` recorded executions, newest first.
#[pg_extern(name = "pg_profiler")]
#[allow(clippy::type_complexity)]
fn pg_profiler_rows(
    lim: default!(i32, 1000),
) -> TableIterator<
    'static,
    (
        name!(seq, i64),
        name!(start_time, Option<TimestampWithTimeZone>),
        name!(xact_start, Option<TimestampWithTimeZone>),
        name!(userid, pg_sys::Oid),
        name!(dbid, pg_sys::Oid),
        name!(pid, i32),
        name!(application_name, Option<String>),
        name!(client_addr, Option<String>),
        name!(xid, Option<i64>),
        name!(queryid, i64),
        name!(plan_id, i64),
        name!(cmd_type, String),
        name!(nesting, i32),
        name!(exec_time_ms, f64),
        name!(plan_time_ms, f64),
        name!(cpu_user_ms, f64),
        name!(cpu_sys_ms, f64),
        name!(rows, i64),
        name!(shared_blks_hit, i64),
        name!(shared_blks_read, i64),
        name!(shared_blks_dirtied, i64),
        name!(shared_blks_written, i64),
        name!(temp_blks_read, i64),
        name!(temp_blks_written, i64),
        name!(blk_read_time_ms, f64),
        name!(blk_write_time_ms, f64),
        name!(wal_records, i64),
        name!(wal_fpi, i64),
        name!(wal_bytes, i64),
        name!(sqlstate, Option<String>),
        name!(query, Option<String>),
        name!(params, Option<String>),
    ),
> {
    ring::ensure_ready();
    let vis = visibility();
    let rows = ring::stmt_ring().newest(lim.max(0) as usize);
    TableIterator::new(rows.into_iter().map(move |r| stmt_row(&vis, r)))
}

/// Incremental read for live viewers: up to `max_rows` executions with
/// `seq > after_seq`, oldest first. Pass `-1` to start from the oldest retained
/// row. If the first returned `seq` is greater than `after_seq + 1`, rows were
/// overwritten before they could be read.
#[pg_extern(name = "pg_profiler_since")]
#[allow(clippy::type_complexity)]
fn pg_profiler_since_rows(
    after_seq: i64,
    max_rows: default!(i32, 1000),
) -> TableIterator<
    'static,
    (
        name!(seq, i64),
        name!(start_time, Option<TimestampWithTimeZone>),
        name!(xact_start, Option<TimestampWithTimeZone>),
        name!(userid, pg_sys::Oid),
        name!(dbid, pg_sys::Oid),
        name!(pid, i32),
        name!(application_name, Option<String>),
        name!(client_addr, Option<String>),
        name!(xid, Option<i64>),
        name!(queryid, i64),
        name!(plan_id, i64),
        name!(cmd_type, String),
        name!(nesting, i32),
        name!(exec_time_ms, f64),
        name!(plan_time_ms, f64),
        name!(cpu_user_ms, f64),
        name!(cpu_sys_ms, f64),
        name!(rows, i64),
        name!(shared_blks_hit, i64),
        name!(shared_blks_read, i64),
        name!(shared_blks_dirtied, i64),
        name!(shared_blks_written, i64),
        name!(temp_blks_read, i64),
        name!(temp_blks_written, i64),
        name!(blk_read_time_ms, f64),
        name!(blk_write_time_ms, f64),
        name!(wal_records, i64),
        name!(wal_fpi, i64),
        name!(wal_bytes, i64),
        name!(sqlstate, Option<String>),
        name!(query, Option<String>),
        name!(params, Option<String>),
    ),
> {
    ring::ensure_ready();
    let vis = visibility();
    let rows = ring::stmt_ring().since(after_seq, max_rows.max(0) as usize);
    TableIterator::new(rows.into_iter().map(move |r| stmt_row(&vis, r)))
}

#[allow(clippy::type_complexity)]
type EventRow = (
    i64,
    Option<TimestampWithTimeZone>,
    String,
    i32,
    pg_sys::Oid,
    pg_sys::Oid,
    Option<String>,
    Option<String>,
    Option<String>,
    f64,
    Option<String>,
);

fn event_row(vis: &Visibility, (r, detail): (EventRecord, Vec<u8>)) -> EventRow {
    let visible = vis.sees_all || r.userid == vis.me.to_u32();
    let detail = if visible {
        text_of(&detail)
    } else {
        text_of(&detail).map(|_| MASKED.to_string())
    };
    let sqlstate = if r.sqlstate[0] == 0 { None } else { cstr_opt(&r.sqlstate) };
    (
        r.seq as i64,
        ts(r.ts),
        event::name(r.kind).to_string(),
        r.pid,
        pg_sys::Oid::from(r.userid),
        pg_sys::Oid::from(r.dbid),
        cstr_opt(&r.application_name),
        cstr_opt(&r.client_addr),
        sqlstate,
        r.duration_us as f64 / 1000.0,
        detail,
    )
}

/// The newest `lim` events (logins, deadlocks, lock waits, cancels, ...),
/// newest first.
#[pg_extern(name = "pg_profiler_events")]
#[allow(clippy::type_complexity)]
fn pg_profiler_events_rows(
    lim: default!(i32, 1000),
) -> TableIterator<
    'static,
    (
        name!(seq, i64),
        name!(ts, Option<TimestampWithTimeZone>),
        name!(kind, String),
        name!(pid, i32),
        name!(userid, pg_sys::Oid),
        name!(dbid, pg_sys::Oid),
        name!(application_name, Option<String>),
        name!(client_addr, Option<String>),
        name!(sqlstate, Option<String>),
        name!(duration_ms, f64),
        name!(detail, Option<String>),
    ),
> {
    ring::ensure_ready();
    let vis = visibility();
    let rows = ring::event_ring().newest(lim.max(0) as usize);
    TableIterator::new(rows.into_iter().map(move |r| event_row(&vis, r)))
}

/// Incremental read of events, like `pg_profiler_since`.
#[pg_extern(name = "pg_profiler_events_since")]
#[allow(clippy::type_complexity)]
fn pg_profiler_events_since_rows(
    after_seq: i64,
    max_rows: default!(i32, 1000),
) -> TableIterator<
    'static,
    (
        name!(seq, i64),
        name!(ts, Option<TimestampWithTimeZone>),
        name!(kind, String),
        name!(pid, i32),
        name!(userid, pg_sys::Oid),
        name!(dbid, pg_sys::Oid),
        name!(application_name, Option<String>),
        name!(client_addr, Option<String>),
        name!(sqlstate, Option<String>),
        name!(duration_ms, f64),
        name!(detail, Option<String>),
    ),
> {
    ring::ensure_ready();
    let vis = visibility();
    let rows = ring::event_ring().since(after_seq, max_rows.max(0) as usize);
    TableIterator::new(rows.into_iter().map(move |r| event_row(&vis, r)))
}

#[allow(clippy::type_complexity)]
type PlanRow = (
    i64,
    Option<TimestampWithTimeZone>,
    i64,
    i64,
    pg_sys::Oid,
    pg_sys::Oid,
    i32,
    String,
    String,
    bool,
    f64,
    f64,
    f64,
    i64,
    Option<String>,
    Option<String>,
);

fn plan_row(vis: &Visibility, (r, payload): (PlanRecord, Vec<u8>)) -> PlanRow {
    let p_len = (r.plan_len as usize).min(payload.len());
    let (plan, query) = payload.split_at(p_len);
    let visible = vis.sees_all || r.userid == vis.me.to_u32();
    let (plan, query) = if visible {
        (text_of(plan), text_of(query))
    } else {
        (text_of(plan).map(|_| MASKED.to_string()), text_of(query).map(|_| MASKED.to_string()))
    };
    (
        r.seq as i64,
        ts(r.ts),
        r.queryid,
        r.plan_id,
        pg_sys::Oid::from(r.userid),
        pg_sys::Oid::from(r.dbid),
        r.pid,
        plan_reason::names(r.reasons),
        if r.format == 1 { "json" } else { "text" }.to_string(),
        r.analyzed,
        r.exec_time_us / 1000.0,
        r.plan_rows,
        r.max_misestimate,
        r.temp_blks_written,
        plan,
        query,
    )
}

/// The newest `lim` stored plans, newest first. A plan is stored the first
/// time its shape is seen for a query (`reason` = `new`) and, for sampled
/// statements, when a run was slow, spilled or had a bad estimate (the plan then
/// carries actual row counts).
#[pg_extern(name = "pg_profiler_plans")]
#[allow(clippy::type_complexity)]
fn pg_profiler_plans_rows(
    lim: default!(i32, 100),
) -> TableIterator<
    'static,
    (
        name!(seq, i64),
        name!(ts, Option<TimestampWithTimeZone>),
        name!(queryid, i64),
        name!(plan_id, i64),
        name!(userid, pg_sys::Oid),
        name!(dbid, pg_sys::Oid),
        name!(pid, i32),
        name!(reason, String),
        name!(format, String),
        name!(has_actuals, bool),
        name!(exec_time_ms, f64),
        name!(plan_rows, f64),
        name!(max_misestimate, f64),
        name!(temp_blks_written, i64),
        name!(plan, Option<String>),
        name!(query, Option<String>),
    ),
> {
    ring::ensure_ready();
    let vis = visibility();
    let rows = ring::plan_ring().newest(lim.max(0) as usize);
    TableIterator::new(rows.into_iter().map(move |r| plan_row(&vis, r)))
}

/// Incremental read of stored plans, like `pg_profiler_since`.
#[pg_extern(name = "pg_profiler_plans_since")]
#[allow(clippy::type_complexity)]
fn pg_profiler_plans_since_rows(
    after_seq: i64,
    max_rows: default!(i32, 100),
) -> TableIterator<
    'static,
    (
        name!(seq, i64),
        name!(ts, Option<TimestampWithTimeZone>),
        name!(queryid, i64),
        name!(plan_id, i64),
        name!(userid, pg_sys::Oid),
        name!(dbid, pg_sys::Oid),
        name!(pid, i32),
        name!(reason, String),
        name!(format, String),
        name!(has_actuals, bool),
        name!(exec_time_ms, f64),
        name!(plan_rows, f64),
        name!(max_misestimate, f64),
        name!(temp_blks_written, i64),
        name!(plan, Option<String>),
        name!(query, Option<String>),
    ),
> {
    ring::ensure_ready();
    let vis = visibility();
    let rows = ring::plan_ring().since(after_seq, max_rows.max(0) as usize);
    TableIterator::new(rows.into_iter().map(move |r| plan_row(&vis, r)))
}

/// The statement each backend is running right now (top-level only).
#[pg_extern(name = "pg_profiler_running")]
#[allow(clippy::type_complexity)]
fn pg_profiler_running_rows() -> TableIterator<
    'static,
    (
        name!(pid, i32),
        name!(userid, pg_sys::Oid),
        name!(dbid, pg_sys::Oid),
        name!(queryid, i64),
        name!(cmd_type, String),
        name!(start_time, Option<TimestampWithTimeZone>),
        name!(xact_start, Option<TimestampWithTimeZone>),
        name!(query, Option<String>),
    ),
> {
    ring::ensure_ready();
    let vis = visibility();
    let rows = running::snapshot();
    TableIterator::new(rows.into_iter().map(move |(s, text)| {
        let visible = vis.sees_all || s.userid == vis.me.to_u32();
        let query = if visible { text_of(&text) } else { Some(MASKED.to_string()) };
        (
            s.pid,
            pg_sys::Oid::from(s.userid),
            pg_sys::Oid::from(s.dbid),
            s.queryid,
            cmd::name(s.cmd_type).to_string(),
            ts(s.start_ts),
            ts(s.xact_start),
            query,
        )
    }))
}

/// Add a capture rule. A statement is kept if it matches any rule; a rule
/// matches when all of its set conditions hold. Returns the rule id.
#[pg_extern]
fn pg_profiler_rule_add(
    database: default!(Option<&str>, "NULL"),
    role: default!(Option<&str>, "NULL"),
    application_name: default!(Option<&str>, "NULL"),
    min_duration_ms: default!(f64, 0),
    query_contains: default!(Option<&str>, "NULL"),
    errors_only: default!(bool, false),
) -> i32 {
    ring::ensure_ready();
    let mut rule = rules::Rule::empty();
    unsafe {
        if let Some(db) = database {
            let c = std::ffi::CString::new(db).unwrap_or_default();
            rule.dbid = pg_sys::get_database_oid(c.as_ptr(), false).to_u32();
        }
        if let Some(r) = role {
            let c = std::ffi::CString::new(r).unwrap_or_default();
            rule.userid = pg_sys::get_role_oid(c.as_ptr(), false).to_u32();
        }
    }
    for (label, v) in [("application_name", application_name), ("query_contains", query_contains)] {
        if v.is_some_and(|s| s.len() >= rules::NAME_LEN) {
            error!("{label} must be shorter than {} bytes", rules::NAME_LEN);
        }
    }
    if let Some(a) = application_name {
        copy_into(&mut rule.app, a.as_bytes());
        rule.app_len = a.len() as u8;
    }
    if let Some(t) = query_contains {
        copy_into(&mut rule.text, t.as_bytes());
        rule.text_len = t.len() as u8;
    }
    rule.min_duration_us = min_duration_ms.max(0.0) * 1000.0;
    rule.errors_only = u8::from(errors_only);
    match rules::add(rule) {
        Ok(id) => id as i32,
        Err(msg) => error!("{msg}"),
    }
}

#[pg_extern]
fn pg_profiler_rule_remove(id: i32) -> bool {
    ring::ensure_ready();
    rules::remove(id.max(0) as u32)
}

#[pg_extern(name = "pg_profiler_rules")]
#[allow(clippy::type_complexity)]
fn pg_profiler_rules_rows() -> TableIterator<
    'static,
    (
        name!(id, i32),
        name!(database, Option<String>),
        name!(role, Option<String>),
        name!(application_name, Option<String>),
        name!(min_duration_ms, f64),
        name!(query_contains, Option<String>),
        name!(errors_only, bool),
    ),
> {
    ring::ensure_ready();
    let list = rules::list();
    TableIterator::new(list.into_iter().map(|r| {
        let database = if r.dbid == 0 {
            None
        } else {
            name_of(unsafe { pg_sys::get_database_name(pg_sys::Oid::from(r.dbid)) })
        };
        let role = if r.userid == 0 {
            None
        } else {
            name_of(unsafe { pg_sys::GetUserNameFromId(pg_sys::Oid::from(r.userid), true) })
        };
        (
            r.id as i32,
            database,
            role,
            text_of(&r.app[..r.app_len as usize]),
            r.min_duration_us / 1000.0,
            text_of(&r.text[..r.text_len as usize]),
            r.errors_only != 0,
        )
    }))
}

extension_sql!(
    r#"
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'profiler_reader') THEN
        CREATE ROLE profiler_reader NOLOGIN;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'profiler_admin') THEN
        CREATE ROLE profiler_admin NOLOGIN;
    END IF;
END $$;
GRANT profiler_reader TO profiler_admin;

REVOKE ALL ON FUNCTION pg_profiler_reset() FROM PUBLIC;
REVOKE ALL ON FUNCTION pg_profiler_rule_add(text, text, text, float8, text, bool) FROM PUBLIC;
REVOKE ALL ON FUNCTION pg_profiler_rule_remove(int) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pg_profiler_reset() TO profiler_admin;
GRANT EXECUTE ON FUNCTION pg_profiler_rule_add(text, text, text, float8, text, bool) TO profiler_admin;
GRANT EXECUTE ON FUNCTION pg_profiler_rule_remove(int) TO profiler_admin;
"#,
    name = "roles_and_grants",
    requires = [pg_profiler_reset, pg_profiler_rule_add, pg_profiler_rule_remove]
);

extension_sql!(
    r#"
CREATE VIEW pg_profiler_recent AS
    SELECT *, (nesting = 0) AS toplevel, (sqlstate IS NOT NULL) AS is_error FROM pg_profiler(100);

CREATE VIEW pg_profiler_slowest AS
    SELECT *, (nesting = 0) AS toplevel, (sqlstate IS NOT NULL) AS is_error
      FROM pg_profiler(2147483647) ORDER BY exec_time_ms DESC LIMIT 100;

CREATE VIEW pg_profiler_errors AS
    SELECT *, (nesting = 0) AS toplevel, true AS is_error
      FROM pg_profiler(2147483647) WHERE sqlstate IS NOT NULL;

-- One row per (database, query, plan shape) that is still in the statement buffer.
CREATE VIEW pg_profiler_plan_stats AS
    SELECT dbid, queryid, plan_id,
           count(*) AS calls,
           round(avg(exec_time_ms)::numeric, 3) AS avg_exec_ms,
           max(exec_time_ms) AS max_exec_ms,
           round(avg(plan_time_ms)::numeric, 3) AS avg_plan_ms,
           sum(rows) AS rows,
           sum(shared_blks_read) AS shared_blks_read,
           sum(temp_blks_written) AS temp_blks_written,
           min(start_time) AS first_seen,
           max(start_time) AS last_seen,
           (array_agg(query ORDER BY seq DESC))[1] AS query
      FROM pg_profiler(2147483647)
     WHERE plan_id <> 0 AND sqlstate IS NULL
     GROUP BY dbid, queryid, plan_id;

-- Queries that ran with more than one plan shape: candidates for a plan regression.
CREATE VIEW pg_profiler_plan_changes AS
    SELECT dbid, queryid,
           count(*) AS plans,
           sum(calls) AS calls,
           min(avg_exec_ms) AS fastest_avg_ms,
           max(avg_exec_ms) AS slowest_avg_ms,
           round(max(avg_exec_ms) / NULLIF(min(avg_exec_ms), 0), 1) AS slowdown,
           max(last_seen) AS last_seen,
           (array_agg(query ORDER BY last_seen DESC))[1] AS query
      FROM pg_profiler_plan_stats
     GROUP BY dbid, queryid
    HAVING count(*) > 1;

CREATE VIEW pg_profiler_activity AS
    SELECT r.pid, a.usename, a.datname, a.application_name, a.client_addr,
           a.state, a.wait_event_type, a.wait_event,
           r.queryid, r.cmd_type, r.start_time, r.xact_start,
           now() - r.start_time AS elapsed,
           pg_blocking_pids(r.pid) AS blocked_by,
           r.query
      FROM pg_profiler_running() r
      LEFT JOIN pg_stat_activity a USING (pid);

GRANT SELECT ON pg_profiler_recent, pg_profiler_slowest, pg_profiler_errors,
                pg_profiler_activity, pg_profiler_plan_stats, pg_profiler_plan_changes TO PUBLIC;
"#,
    name = "views",
    requires = [pg_profiler_rows, pg_profiler_running_rows, "roles_and_grants"]
);
