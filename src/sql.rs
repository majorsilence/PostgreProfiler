//! SQL-facing functions and views.

use crate::record::cmd;
use crate::ring;
use pgrx::prelude::*;

#[pg_extern]
fn pg_profiler_info() -> TableIterator<
    'static,
    (
        name!(capacity, i64),
        name!(total_written, i64),
        name!(retained, i64),
        name!(overwritten, i64),
        name!(max_query_len, i32),
    ),
> {
    let s = ring::stats();
    TableIterator::once((s.capacity, s.total_written, s.retained, s.overwritten, s.max_query_len))
}

#[pg_extern]
fn pg_profiler_reset() {
    ring::reset();
}

extension_sql!(
    r#"REVOKE ALL ON FUNCTION pg_profiler_reset() FROM PUBLIC;"#,
    name = "revoke_reset",
    requires = [pg_profiler_reset]
);

/// The newest `lim` recorded executions, newest first.
#[pg_extern(name = "pg_profiler")]
#[allow(clippy::type_complexity)]
fn pg_profiler_rows(lim: default!(i32, 1000)) -> TableIterator<
    'static,
    (
        name!(seq, i64),
        name!(start_time, Option<TimestampWithTimeZone>),
        name!(userid, pg_sys::Oid),
        name!(dbid, pg_sys::Oid),
        name!(pid, i32),
        name!(queryid, i64),
        name!(cmd_type, String),
        name!(toplevel, bool),
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
        name!(local_blks_hit, i64),
        name!(local_blks_read, i64),
        name!(temp_blks_read, i64),
        name!(temp_blks_written, i64),
        name!(wal_records, i64),
        name!(wal_fpi, i64),
        name!(wal_bytes, i64),
        name!(is_error, bool),
        name!(sqlstate, Option<String>),
        name!(query, Option<String>),
    ),
> {
    ring::ensure_ready();
    let rows = ring::snapshot(lim.max(0) as usize);
    // Same visibility rule as pg_stat_statements: query text of other users'
    // statements is hidden unless the caller is a superuser or has the
    // privileges of pg_read_all_stats.
    let me = unsafe { pg_sys::GetUserId() };
    let sees_all = unsafe {
        pg_sys::superuser()
            || pg_sys::has_privs_of_role(me, pg_sys::Oid::from(pg_sys::ROLE_PG_READ_ALL_STATS))
    };
    TableIterator::new(rows.into_iter().map(move |(r, text)| {
        let sqlstate = if r.is_error {
            Some(String::from_utf8_lossy(&r.sqlstate).into_owned())
        } else {
            None
        };
        let query = if !sees_all && r.userid != me.to_u32() {
            Some("<insufficient privilege>".to_string())
        } else if r.query_len > 0 {
            Some(String::from_utf8_lossy(&text).into_owned())
        } else {
            None
        };
        (
            r.seq as i64,
            TimestampWithTimeZone::try_from(r.start_ts).ok(),
            pg_sys::Oid::from(r.userid),
            pg_sys::Oid::from(r.dbid),
            r.pid,
            r.queryid,
            cmd::name(r.cmd_type).to_string(),
            r.toplevel,
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
            r.local_blks_hit,
            r.local_blks_read,
            r.temp_blks_read,
            r.temp_blks_written,
            r.wal_records,
            r.wal_fpi,
            r.wal_bytes as i64,
            r.is_error,
            sqlstate,
            query,
        )
    }))
}

extension_sql!(
    r#"
CREATE VIEW pg_profiler_recent AS
    SELECT * FROM pg_profiler(100);

CREATE VIEW pg_profiler_slowest AS
    SELECT * FROM pg_profiler(2147483647) ORDER BY exec_time_ms DESC LIMIT 100;

CREATE VIEW pg_profiler_errors AS
    SELECT * FROM pg_profiler(2147483647) WHERE is_error;

GRANT SELECT ON pg_profiler_recent, pg_profiler_slowest, pg_profiler_errors TO PUBLIC;
"#,
    name = "views",
    requires = [pg_profiler_rows]
);
