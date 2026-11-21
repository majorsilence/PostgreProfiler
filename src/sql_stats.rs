//! SQL surface of the statistics (Query Store) part: in-memory intervals,
//! cached query texts, histogram helpers.

use crate::sql::{text_of, ts, visibility, MASKED};
use crate::stats::{self, Row, Which, HIST_BUCKETS};
use pgrx::prelude::*;

/// Start of an interval number as a timestamptz.
fn interval_start(n: u64) -> Option<TimestampWithTimeZone> {
    let us = n as i64 * stats::interval_us();
    ts(us)
}

#[allow(clippy::type_complexity)]
type StatsRow = (
    Option<TimestampWithTimeZone>,
    pg_sys::Oid,
    pg_sys::Oid,
    i64,
    i64,
    i64,
    i64,
    f64,
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
    i64,
    i64,
    i64,
    f64,
    f64,
    f64,
    f64,
    Vec<i32>,
    Vec<i32>,
);

fn stats_row(r: Row) -> StatsRow {
    let d = r.data;
    let ms = |us: f64| us / 1000.0;
    (
        interval_start(r.interval_no),
        pg_sys::Oid::from(r.dbid),
        pg_sys::Oid::from(r.userid),
        r.queryid,
        r.plan_id,
        d.calls as i64,
        d.errors as i64,
        ms(d.exec_sum_us),
        d.exec_sumsq_us / 1.0e6,
        if d.calls == 0 { 0.0 } else { ms(d.exec_min_us) },
        ms(d.exec_max_us),
        ms(d.plan_sum_us),
        d.rows as i64,
        d.shared_blks_hit as i64,
        d.shared_blks_read as i64,
        d.shared_blks_dirtied as i64,
        d.shared_blks_written as i64,
        d.temp_blks_read as i64,
        d.temp_blks_written as i64,
        d.wal_records as i64,
        d.wal_fpi as i64,
        d.wal_bytes as i64,
        ms(d.cpu_user_us as f64),
        ms(d.cpu_sys_us as f64),
        ms(d.blk_read_us as f64),
        ms(d.blk_write_us as f64),
        d.hist.iter().map(|&c| c as i32).collect(),
        d.waits.iter().map(|&c| c as i32).collect(),
    )
}

/// Statistics of the intervals still in memory: one row per interval, database,
/// role, query and plan. With `only_unarchived` the intervals the archiver has
/// already stored are left out.
///
/// Times are in milliseconds; `exec_sumsq_ms2` is the sum of squared execution
/// times (for the standard deviation); `hist` is the latency histogram (two
/// buckets per octave starting at about 1 microsecond, see
/// `pg_profiler_hist_percentile`).
#[pg_extern(name = "pg_profiler_stats_memory")]
#[allow(clippy::type_complexity)]
fn pg_profiler_stats_memory_rows(
    only_unarchived: default!(bool, false),
) -> TableIterator<
    'static,
    (
        name!(interval_start, Option<TimestampWithTimeZone>),
        name!(dbid, pg_sys::Oid),
        name!(userid, pg_sys::Oid),
        name!(queryid, i64),
        name!(plan_id, i64),
        name!(calls, i64),
        name!(errors, i64),
        name!(exec_total_ms, f64),
        name!(exec_sumsq_ms2, f64),
        name!(exec_min_ms, f64),
        name!(exec_max_ms, f64),
        name!(plan_total_ms, f64),
        name!(rows, i64),
        name!(shared_blks_hit, i64),
        name!(shared_blks_read, i64),
        name!(shared_blks_dirtied, i64),
        name!(shared_blks_written, i64),
        name!(temp_blks_read, i64),
        name!(temp_blks_written, i64),
        name!(wal_records, i64),
        name!(wal_fpi, i64),
        name!(wal_bytes, i64),
        name!(cpu_user_ms, f64),
        name!(cpu_sys_ms, f64),
        name!(blk_read_ms, f64),
        name!(blk_write_ms, f64),
        name!(hist, Vec<i32>),
        name!(wait_samples, Vec<i32>),
    ),
> {
    crate::ring::ensure_ready();
    let which = if only_unarchived { Which::Unarchived } else { Which::All };
    TableIterator::new(stats::snapshot(which).into_iter().map(stats_row))
}

/// Cached query texts of the statistics. Only superusers, `pg_read_all_stats`
/// and `profiler_reader` see them; everybody else gets a masked text.
#[pg_extern(name = "pg_profiler_stats_texts")]
fn pg_profiler_stats_texts_rows() -> TableIterator<
    'static,
    (name!(dbid, pg_sys::Oid), name!(queryid, i64), name!(query, Option<String>)),
> {
    crate::ring::ensure_ready();
    let vis = visibility();
    TableIterator::new(stats::all_texts().into_iter().map(move |(dbid, queryid, text)| {
        let q = if vis.sees_all { text_of(&text) } else { Some(MASKED.to_string()) };
        (pg_sys::Oid::from(dbid), queryid, q)
    }))
}

/// State of the in-memory statistics: the current interval, how full it is and
/// what was lost.
#[pg_extern(name = "pg_profiler_stats_info")]
#[allow(clippy::type_complexity)]
fn pg_profiler_stats_info_rows() -> TableIterator<
    'static,
    (
        name!(interval_seconds, i32),
        name!(entries_capacity, i32),
        name!(current_interval, Option<TimestampWithTimeZone>),
        name!(intervals_in_memory, i32),
        name!(current_entries, i32),
        name!(unarchived_intervals, i32),
        name!(rotations, i64),
        name!(lost_intervals, i64),
        name!(dropped_entries, i64),
        name!(texts_evicted, i64),
    ),
> {
    crate::ring::ensure_ready();
    let Some(i) = stats::info() else { return TableIterator::new(std::iter::empty()) };
    let current_entries = i.gens.iter().find(|g| g.interval_no == i.current).map_or(0, |g| g.entries as i32);
    let unarchived = i.gens.iter().filter(|g| !g.archived && g.interval_no < i.current).count() as i32;
    TableIterator::once((
        (i.interval_us / 1_000_000) as i32,
        i.capacity as i32,
        if i.current == 0 { None } else { interval_start(i.current) },
        i.gens.len() as i32,
        current_entries,
        unarchived,
        i.rotations as i64,
        i.lost_intervals as i64,
        i.dropped_entries as i64,
        i.texts_dropped as i64,
    ))
}

/// Forget the in-memory statistics (the archive is not touched).
#[pg_extern]
fn pg_profiler_stats_reset() {
    crate::ring::ensure_ready();
    stats::reset();
}

/// Elementwise sum of two latency histograms; the state function of
/// `pg_profiler_hist_agg`.
#[pg_extern(immutable, parallel_safe)]
fn pg_profiler_hist_add(state: Option<Vec<i64>>, hist: Option<Vec<i32>>) -> Option<Vec<i64>> {
    let Some(hist) = hist else { return state };
    let mut acc = state.unwrap_or_else(|| vec![0; HIST_BUCKETS]);
    if acc.len() < hist.len() {
        acc.resize(hist.len(), 0);
    }
    for (a, h) in acc.iter_mut().zip(hist) {
        *a += i64::from(h);
    }
    Some(acc)
}

/// Estimated percentile (0..1) of a latency histogram in milliseconds: the
/// middle of the bucket that contains it, so accurate to about 20 %.
#[pg_extern(immutable, parallel_safe)]
fn pg_profiler_hist_percentile(hist: Vec<i64>, pct: f64) -> Option<f64> {
    let total: i64 = hist.iter().sum();
    if total <= 0 {
        return None;
    }
    let target = (pct.clamp(0.0, 1.0) * total as f64).ceil().max(1.0) as i64;
    let mut seen = 0;
    for (i, c) in hist.iter().enumerate() {
        seen += c;
        if seen >= target {
            let (lo, hi) = (stats::bucket_lower_us(i), stats::bucket_lower_us(i + 1));
            return Some((lo * hi).sqrt() / 1000.0);
        }
    }
    None
}

/// Names of the wait categories, in the order of `wait_samples`.
#[pg_extern(immutable, parallel_safe)]
fn pg_profiler_wait_categories() -> Vec<&'static str> {
    crate::sampler::CATEGORY_NAMES.to_vec()
}

extension_sql!(
    r#"
CREATE AGGREGATE pg_profiler_hist_agg(int4[]) (
    SFUNC = pg_profiler_hist_add,
    STYPE = int8[]
);

REVOKE ALL ON FUNCTION pg_profiler_stats_reset() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pg_profiler_stats_reset() TO profiler_admin;
"#,
    name = "stats_helpers",
    requires = [pg_profiler_hist_add, pg_profiler_stats_reset, "roles_and_grants"]
);
