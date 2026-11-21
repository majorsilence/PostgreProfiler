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

// The history: tables the archiver fills, helper functions for partitions and
// retention, and the views that merge history with what is still in memory.
extension_sql!(
    r#"
CREATE TABLE pg_profiler_hist_stats (
    interval_start      timestamptz NOT NULL,
    dbid                oid         NOT NULL,
    userid              oid         NOT NULL,
    queryid             bigint      NOT NULL,
    plan_id             bigint      NOT NULL,
    calls               bigint      NOT NULL,
    errors              bigint      NOT NULL,
    exec_total_ms       float8      NOT NULL,
    exec_sumsq_ms2      float8      NOT NULL,
    exec_min_ms         float8      NOT NULL,
    exec_max_ms         float8      NOT NULL,
    plan_total_ms       float8      NOT NULL,
    rows                bigint      NOT NULL,
    shared_blks_hit     bigint      NOT NULL,
    shared_blks_read    bigint      NOT NULL,
    shared_blks_dirtied bigint      NOT NULL,
    shared_blks_written bigint      NOT NULL,
    temp_blks_read      bigint      NOT NULL,
    temp_blks_written   bigint      NOT NULL,
    wal_records         bigint      NOT NULL,
    wal_fpi             bigint      NOT NULL,
    wal_bytes           bigint      NOT NULL,
    cpu_user_ms         float8      NOT NULL,
    cpu_sys_ms          float8      NOT NULL,
    blk_read_ms         float8      NOT NULL,
    blk_write_ms        float8      NOT NULL,
    hist                int4[]      NOT NULL,
    wait_samples        int4[]      NOT NULL,
    PRIMARY KEY (interval_start, dbid, userid, queryid, plan_id)
) PARTITION BY RANGE (interval_start);

CREATE TABLE pg_profiler_hist_queries (
    dbid       oid         NOT NULL,
    queryid    bigint      NOT NULL,
    query      text        NOT NULL,
    first_seen timestamptz NOT NULL,
    last_seen  timestamptz NOT NULL,
    PRIMARY KEY (dbid, queryid)
);

-- Plan shapes seen for each query (estimates only; see plan_samples for actuals).
CREATE TABLE pg_profiler_hist_plans (
    dbid       oid         NOT NULL,
    queryid    bigint      NOT NULL,
    plan_id    bigint      NOT NULL,
    first_seen timestamptz NOT NULL,
    last_seen  timestamptz NOT NULL,
    plan       text        NOT NULL,
    format     text        NOT NULL,
    PRIMARY KEY (dbid, queryid, plan_id)
);

-- Outlier runs stored with actual row counts.
CREATE TABLE pg_profiler_hist_plan_samples (
    ts                timestamptz NOT NULL,
    dbid              oid         NOT NULL,
    queryid           bigint      NOT NULL,
    plan_id           bigint      NOT NULL,
    reason            text        NOT NULL,
    exec_time_ms      float8      NOT NULL,
    max_misestimate   float8      NOT NULL,
    temp_blks_written bigint      NOT NULL,
    plan              text        NOT NULL,
    format            text        NOT NULL,
    PRIMARY KEY (ts, dbid, queryid, plan_id, reason)
);

-- Create the daily (UTC) partition that holds a timestamp.
CREATE FUNCTION pg_profiler_ensure_partition(ts timestamptz) RETURNS text
LANGUAGE plpgsql AS $$
DECLARE
    schema_name text;
    day date := (ts AT TIME ZONE 'UTC')::date;
    part text := 'pg_profiler_hist_stats_' || to_char(day, 'YYYYMMDD');
BEGIN
    SELECT n.nspname INTO schema_name
      FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
     WHERE e.extname = 'pg_profiler';
    IF to_regclass(format('%I.%I', schema_name, part)) IS NULL THEN
        EXECUTE format(
            'CREATE TABLE %I.%I PARTITION OF %I.pg_profiler_hist_stats FOR VALUES FROM (%L) TO (%L)',
            schema_name, part, schema_name,
            (day::timestamp AT TIME ZONE 'UTC'), ((day + 1)::timestamp AT TIME ZONE 'UTC'));
    END IF;
    RETURN part;
END $$;

-- Apply the retention policy; returns what was removed.
CREATE FUNCTION pg_profiler_purge(
    retention_days int DEFAULT 30,
    max_size_mb bigint DEFAULT 0,
    max_plans_per_query int DEFAULT 20)
RETURNS TABLE (dropped_partitions int, deleted_queries bigint, deleted_plans bigint, deleted_samples bigint)
LANGUAGE plpgsql AS $$
DECLARE
    schema_name text;
    parent regclass;
    r record;
    n_parts int;
    total bigint;
    cutoff timestamptz;
BEGIN
    SELECT n.nspname INTO schema_name
      FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
     WHERE e.extname = 'pg_profiler';
    parent := format('%I.pg_profiler_hist_stats', schema_name)::regclass;
    dropped_partitions := 0;
    deleted_queries := 0; deleted_plans := 0; deleted_samples := 0;

    -- daily partitions older than the retention
    IF retention_days > 0 THEN
        FOR r IN
            SELECT c.oid::regclass::text AS rel, substring(c.relname from '[0-9]{8}$') AS day
              FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
             WHERE i.inhparent = parent
             ORDER BY c.relname
        LOOP
            IF to_date(r.day, 'YYYYMMDD') < (now() AT TIME ZONE 'UTC')::date - retention_days THEN
                EXECUTE 'DROP TABLE ' || r.rel;
                dropped_partitions := dropped_partitions + 1;
            END IF;
        END LOOP;
    END IF;

    -- size cap: drop the oldest day, but never the last one
    IF max_size_mb > 0 THEN
        LOOP
            SELECT count(*), coalesce(sum(pg_total_relation_size(c.oid)), 0) INTO n_parts, total
              FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
             WHERE i.inhparent = parent;
            EXIT WHEN n_parts <= 1 OR total <= max_size_mb * 1024 * 1024;
            SELECT c.oid::regclass::text AS rel INTO r
              FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
             WHERE i.inhparent = parent
             ORDER BY c.relname LIMIT 1;
            EXECUTE 'DROP TABLE ' || r.rel;
            dropped_partitions := dropped_partitions + 1;
        END LOOP;
    END IF;

    -- stale queries, plans and samples
    IF retention_days > 0 THEN
        cutoff := now() - make_interval(days => retention_days);
        DELETE FROM pg_profiler_hist_queries WHERE last_seen < cutoff;
        GET DIAGNOSTICS deleted_queries = ROW_COUNT;
        DELETE FROM pg_profiler_hist_plans WHERE last_seen < cutoff;
        GET DIAGNOSTICS deleted_plans = ROW_COUNT;
        DELETE FROM pg_profiler_hist_plan_samples WHERE ts < cutoff;
        GET DIAGNOSTICS deleted_samples = ROW_COUNT;
    END IF;

    -- per-query plan cap: keep the most recently seen plans
    IF max_plans_per_query > 0 THEN
        DELETE FROM pg_profiler_hist_plans p
         USING (SELECT dbid, queryid, plan_id,
                       row_number() OVER (PARTITION BY dbid, queryid ORDER BY last_seen DESC, plan_id) AS rn
                  FROM pg_profiler_hist_plans) x
         WHERE p.dbid = x.dbid AND p.queryid = x.queryid AND p.plan_id = x.plan_id
           AND x.rn > max_plans_per_query;
        GET DIAGNOSTICS n_parts = ROW_COUNT;
        deleted_plans := deleted_plans + n_parts;
    END IF;
    RETURN NEXT;
END $$;

REVOKE ALL ON FUNCTION pg_profiler_ensure_partition(timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION pg_profiler_purge(int, bigint, int) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pg_profiler_ensure_partition(timestamptz) TO profiler_admin;
GRANT EXECUTE ON FUNCTION pg_profiler_purge(int, bigint, int) TO profiler_admin;

-- Statistics per interval: the history plus the intervals still only in memory
-- (those newer than the newest archived one, so no row is counted twice).
CREATE VIEW pg_profiler_runtime_stats AS
    SELECT * FROM pg_profiler_hist_stats
    UNION ALL
    SELECT * FROM pg_profiler_stats_memory(true) m
     WHERE m.interval_start > coalesce((SELECT max(interval_start) FROM pg_profiler_hist_stats), '-infinity');

-- Query texts: the history plus what is cached in memory.
CREATE VIEW pg_profiler_queries AS
    SELECT dbid, queryid, query, first_seen, last_seen FROM pg_profiler_hist_queries
    UNION ALL
    SELECT t.dbid, t.queryid, t.query, NULL::timestamptz, NULL::timestamptz
      FROM pg_profiler_stats_texts() t
     WHERE NOT EXISTS (SELECT 1 FROM pg_profiler_hist_queries h WHERE h.dbid = t.dbid AND h.queryid = t.queryid);

GRANT SELECT ON pg_profiler_hist_stats, pg_profiler_hist_queries, pg_profiler_hist_plans,
                pg_profiler_hist_plan_samples, pg_profiler_runtime_stats, pg_profiler_queries
      TO profiler_reader;
"#,
    name = "history",
    requires = ["stats_helpers", "roles_and_grants", pg_profiler_stats_memory_rows, pg_profiler_stats_texts_rows]
);

// Reports over the statistics, modelled on the Query Store reports: top
// consumers, overall consumption, queries with high variation, the history of
// one query, regressed queries and where statements wait.
extension_sql!(
    r#"
-- Top resource consumers. metric: exec_time, avg_time, calls, rows, reads (shared
-- blocks read), temp (temp blocks written), wal (bytes), cpu, io_time, plan_time.
CREATE FUNCTION pg_profiler_top_consumers(
    metric text DEFAULT 'exec_time',
    from_ts timestamptz DEFAULT now() - interval '1 hour',
    to_ts timestamptz DEFAULT now(),
    lim int DEFAULT 20)
RETURNS TABLE (
    dbid oid, queryid bigint, query text, calls bigint, errors bigint,
    total_exec_ms float8, avg_exec_ms float8, stddev_exec_ms float8, p95_exec_ms float8,
    rows bigint, shared_blks_read bigint, temp_blks_written bigint, wal_bytes bigint,
    cpu_ms float8, io_ms float8, plans bigint, metric_value float8)
LANGUAGE plpgsql STABLE AS $$
BEGIN
    IF metric NOT IN ('exec_time', 'avg_time', 'calls', 'rows', 'reads', 'temp', 'wal', 'cpu', 'io_time', 'plan_time') THEN
        RAISE EXCEPTION 'unknown metric "%"', metric
            USING HINT = 'Use exec_time, avg_time, calls, rows, reads, temp, wal, cpu, io_time or plan_time.';
    END IF;
    RETURN QUERY
    WITH s AS (
        SELECT r.dbid, r.queryid,
               sum(r.calls)::bigint AS calls, sum(r.errors)::bigint AS errors,
               sum(r.exec_total_ms) AS total, sum(r.exec_sumsq_ms2) AS sumsq,
               sum(r.rows)::bigint AS rows, sum(r.shared_blks_read)::bigint AS reads,
               sum(r.temp_blks_written)::bigint AS temp, sum(r.wal_bytes)::bigint AS wal,
               sum(r.cpu_user_ms + r.cpu_sys_ms) AS cpu, sum(r.blk_read_ms + r.blk_write_ms) AS io,
               sum(r.plan_total_ms) AS plan_ms,
               count(DISTINCT r.plan_id) FILTER (WHERE r.plan_id <> 0) AS plans,
               pg_profiler_hist_agg(r.hist) AS hist
          FROM pg_profiler_runtime_stats r
         WHERE r.interval_start >= from_ts AND r.interval_start < to_ts
         GROUP BY r.dbid, r.queryid
    )
    SELECT s.dbid, s.queryid, q.query, s.calls, s.errors,
           s.total, s.total / NULLIF(s.calls, 0),
           sqrt(greatest(s.sumsq / NULLIF(s.calls, 0) - (s.total / NULLIF(s.calls, 0)) ^ 2, 0)),
           pg_profiler_hist_percentile(s.hist, 0.95),
           s.rows, s.reads, s.temp, s.wal, s.cpu, s.io, s.plans,
           CASE metric
               WHEN 'exec_time' THEN s.total
               WHEN 'avg_time' THEN s.total / NULLIF(s.calls, 0)
               WHEN 'calls' THEN s.calls::float8
               WHEN 'rows' THEN s.rows::float8
               WHEN 'reads' THEN s.reads::float8
               WHEN 'temp' THEN s.temp::float8
               WHEN 'wal' THEN s.wal::float8
               WHEN 'cpu' THEN s.cpu
               WHEN 'io_time' THEN s.io
               WHEN 'plan_time' THEN s.plan_ms
           END AS metric_value
      FROM s LEFT JOIN pg_profiler_queries q ON q.dbid = s.dbid AND q.queryid = s.queryid
     ORDER BY 17 DESC NULLS LAST, s.queryid
     LIMIT lim;
END $$;

-- Overall consumption: the totals of everything, per time bucket.
CREATE FUNCTION pg_profiler_overall_consumption(
    from_ts timestamptz DEFAULT now() - interval '24 hours',
    to_ts timestamptz DEFAULT now(),
    bucket interval DEFAULT interval '1 hour')
RETURNS TABLE (
    bucket_start timestamptz, calls bigint, errors bigint, exec_total_ms float8, avg_exec_ms float8,
    rows bigint, shared_blks_read bigint, shared_blks_written bigint, temp_blks_written bigint,
    wal_bytes bigint, cpu_ms float8, distinct_queries bigint)
LANGUAGE sql STABLE AS $$
    SELECT date_bin(bucket, r.interval_start, from_ts),
           sum(r.calls)::bigint, sum(r.errors)::bigint, sum(r.exec_total_ms),
           sum(r.exec_total_ms) / NULLIF(sum(r.calls), 0),
           sum(r.rows)::bigint, sum(r.shared_blks_read)::bigint, sum(r.shared_blks_written)::bigint,
           sum(r.temp_blks_written)::bigint, sum(r.wal_bytes)::bigint,
           sum(r.cpu_user_ms + r.cpu_sys_ms), count(DISTINCT (r.dbid, r.queryid))
      FROM pg_profiler_runtime_stats r
     WHERE r.interval_start >= from_ts AND r.interval_start < to_ts
     GROUP BY 1
     ORDER BY 1
$$;

-- Queries whose run time varies a lot: highest coefficient of variation first.
CREATE FUNCTION pg_profiler_high_variation(
    from_ts timestamptz DEFAULT now() - interval '1 hour',
    to_ts timestamptz DEFAULT now(),
    lim int DEFAULT 20,
    min_calls bigint DEFAULT 10)
RETURNS TABLE (
    dbid oid, queryid bigint, query text, calls bigint, avg_exec_ms float8, stddev_exec_ms float8,
    cv float8, min_exec_ms float8, max_exec_ms float8, p50_exec_ms float8, p95_exec_ms float8,
    p99_exec_ms float8)
LANGUAGE sql STABLE AS $$
    WITH s AS (
        SELECT r.dbid, r.queryid, sum(r.calls)::bigint AS calls,
               sum(r.exec_total_ms) AS total, sum(r.exec_sumsq_ms2) AS sumsq,
               min(r.exec_min_ms) FILTER (WHERE r.calls > 0) AS mn, max(r.exec_max_ms) AS mx,
               pg_profiler_hist_agg(r.hist) AS hist
          FROM pg_profiler_runtime_stats r
         WHERE r.interval_start >= from_ts AND r.interval_start < to_ts
         GROUP BY r.dbid, r.queryid
        HAVING sum(r.calls) >= min_calls
    ), v AS (
        SELECT s.*, s.total / s.calls AS mean,
               sqrt(greatest(s.sumsq / s.calls - (s.total / s.calls) ^ 2, 0)) AS sd
          FROM s
    )
    SELECT v.dbid, v.queryid, q.query, v.calls, v.mean, v.sd, v.sd / NULLIF(v.mean, 0),
           v.mn, v.mx,
           pg_profiler_hist_percentile(v.hist, 0.50), pg_profiler_hist_percentile(v.hist, 0.95),
           pg_profiler_hist_percentile(v.hist, 0.99)
      FROM v LEFT JOIN pg_profiler_queries q ON q.dbid = v.dbid AND q.queryid = v.queryid
     ORDER BY 7 DESC NULLS LAST, v.queryid
     LIMIT lim
$$;

-- One query over time, per interval and plan (the "tracked query" report).
CREATE FUNCTION pg_profiler_query_history(
    queryid bigint,
    from_ts timestamptz DEFAULT now() - interval '24 hours',
    to_ts timestamptz DEFAULT now(),
    dbid oid DEFAULT NULL)
RETURNS TABLE (
    interval_start timestamptz, plan_id bigint, calls bigint, errors bigint,
    avg_exec_ms float8, stddev_exec_ms float8, min_exec_ms float8, max_exec_ms float8,
    p95_exec_ms float8, rows bigint, shared_blks_read bigint, temp_blks_written bigint)
LANGUAGE sql STABLE AS $$
    SELECT r.interval_start, r.plan_id, sum(r.calls)::bigint, sum(r.errors)::bigint,
           sum(r.exec_total_ms) / NULLIF(sum(r.calls), 0),
           sqrt(greatest(sum(r.exec_sumsq_ms2) / NULLIF(sum(r.calls), 0)
                         - (sum(r.exec_total_ms) / NULLIF(sum(r.calls), 0)) ^ 2, 0)),
           min(r.exec_min_ms) FILTER (WHERE r.calls > 0), max(r.exec_max_ms),
           pg_profiler_hist_percentile(pg_profiler_hist_agg(r.hist), 0.95),
           sum(r.rows)::bigint, sum(r.shared_blks_read)::bigint, sum(r.temp_blks_written)::bigint
      FROM pg_profiler_runtime_stats r
     WHERE r.queryid = pg_profiler_query_history.queryid
       AND (pg_profiler_query_history.dbid IS NULL OR r.dbid = pg_profiler_query_history.dbid)
       AND r.interval_start >= from_ts AND r.interval_start < to_ts
     GROUP BY r.interval_start, r.plan_id
     ORDER BY r.interval_start, r.plan_id
$$;

-- Regressed queries: a recent window against a baseline window. A query is
-- listed when its mean run time grew by at least min_ratio and Welch's t
-- statistic (computed from the sums and sums of squares) is at least min_z,
-- both windows having at least min_calls calls. plan_changed says that the
-- recent window used a plan the baseline did not.
CREATE FUNCTION pg_profiler_regressed_queries(
    recent_from timestamptz DEFAULT now() - interval '1 hour',
    recent_to timestamptz DEFAULT now(),
    baseline_from timestamptz DEFAULT now() - interval '25 hours',
    baseline_to timestamptz DEFAULT now() - interval '1 hour',
    min_calls bigint DEFAULT 10,
    min_ratio float8 DEFAULT 1.5,
    min_z float8 DEFAULT 3.0,
    lim int DEFAULT 50)
RETURNS TABLE (
    dbid oid, queryid bigint, query text,
    baseline_calls bigint, recent_calls bigint,
    baseline_avg_ms float8, recent_avg_ms float8, ratio float8, z_score float8,
    added_time_ms float8, plan_changed boolean, baseline_plans bigint[], recent_plans bigint[])
LANGUAGE sql STABLE AS $$
    WITH w AS (
        SELECT r.dbid, r.queryid, r.plan_id,
               (r.interval_start >= recent_from AND r.interval_start < recent_to) AS is_recent,
               r.calls, r.exec_total_ms, r.exec_sumsq_ms2
          FROM pg_profiler_runtime_stats r
         WHERE (r.interval_start >= recent_from AND r.interval_start < recent_to)
            OR (r.interval_start >= baseline_from AND r.interval_start < baseline_to)
    ), q AS (
        SELECT w.dbid, w.queryid,
               sum(w.calls) FILTER (WHERE NOT w.is_recent) AS n1,
               sum(w.exec_total_ms) FILTER (WHERE NOT w.is_recent) AS t1,
               sum(w.exec_sumsq_ms2) FILTER (WHERE NOT w.is_recent) AS s1,
               sum(w.calls) FILTER (WHERE w.is_recent) AS n2,
               sum(w.exec_total_ms) FILTER (WHERE w.is_recent) AS t2,
               sum(w.exec_sumsq_ms2) FILTER (WHERE w.is_recent) AS s2,
               array_agg(DISTINCT w.plan_id) FILTER (WHERE NOT w.is_recent AND w.calls > 0 AND w.plan_id <> 0) AS plans1,
               array_agg(DISTINCT w.plan_id) FILTER (WHERE w.is_recent AND w.calls > 0 AND w.plan_id <> 0) AS plans2
          FROM w GROUP BY w.dbid, w.queryid
    ), m AS (
        SELECT q.*, t1 / n1 AS mean1, t2 / n2 AS mean2,
               greatest((s1 - n1 * (t1 / n1) ^ 2) / NULLIF(n1 - 1, 0), 0) AS var1,
               greatest((s2 - n2 * (t2 / n2) ^ 2) / NULLIF(n2 - 1, 0), 0) AS var2
          FROM q
         WHERE n1 >= min_calls AND n2 >= min_calls
    ), z AS (
        SELECT m.*, mean2 / NULLIF(mean1, 0) AS ratio,
               CASE WHEN var1 / n1 + var2 / n2 > 0
                    THEN (mean2 - mean1) / sqrt(var1 / n1 + var2 / n2)
                    WHEN mean2 > mean1 THEN 'Infinity'::float8
                    ELSE 0 END AS zs
          FROM m
    )
    SELECT z.dbid, z.queryid, qq.query, z.n1::bigint, z.n2::bigint, z.mean1, z.mean2, z.ratio, z.zs,
           (z.mean2 - z.mean1) * z.n2,
           coalesce(NOT (z.plans2 <@ z.plans1), false),
           coalesce(z.plans1, '{}'), coalesce(z.plans2, '{}')
      FROM z LEFT JOIN pg_profiler_queries qq ON qq.dbid = z.dbid AND qq.queryid = z.queryid
     WHERE z.ratio >= min_ratio AND z.zs >= min_z
     ORDER BY (z.mean2 - z.mean1) * z.n2 DESC, z.queryid
     LIMIT lim
$$;

-- Where statements wait: samples per wait category (needs the wait sampler).
CREATE FUNCTION pg_profiler_wait_stats(
    from_ts timestamptz DEFAULT now() - interval '1 hour',
    to_ts timestamptz DEFAULT now(),
    lim int DEFAULT 50)
RETURNS TABLE (dbid oid, queryid bigint, query text, category text, samples bigint, pct_of_query float8)
LANGUAGE sql STABLE AS $$
    WITH per_cat AS (
        SELECT r.dbid, r.queryid, c.name AS category, sum(w.cnt)::bigint AS samples
          FROM pg_profiler_runtime_stats r
         CROSS JOIN LATERAL unnest(r.wait_samples) WITH ORDINALITY AS w(cnt, ord)
          JOIN unnest(pg_profiler_wait_categories()) WITH ORDINALITY AS c(name, ord) ON c.ord = w.ord
         WHERE r.interval_start >= from_ts AND r.interval_start < to_ts
         GROUP BY r.dbid, r.queryid, c.name
    ), tot AS (
        SELECT p.*, sum(p.samples) OVER (PARTITION BY p.dbid, p.queryid) AS total
          FROM per_cat p
    )
    SELECT t.dbid, t.queryid, q.query, t.category, t.samples, 100.0 * t.samples / NULLIF(t.total, 0)
      FROM tot t LEFT JOIN pg_profiler_queries q ON q.dbid = t.dbid AND q.queryid = t.queryid
     WHERE t.samples > 0
     ORDER BY t.samples DESC, t.queryid, t.category
     LIMIT lim
$$;

GRANT EXECUTE ON FUNCTION
    pg_profiler_top_consumers(text, timestamptz, timestamptz, int),
    pg_profiler_overall_consumption(timestamptz, timestamptz, interval),
    pg_profiler_high_variation(timestamptz, timestamptz, int, bigint),
    pg_profiler_query_history(bigint, timestamptz, timestamptz, oid),
    pg_profiler_regressed_queries(timestamptz, timestamptz, timestamptz, timestamptz, bigint, float8, float8, int),
    pg_profiler_wait_stats(timestamptz, timestamptz, int)
  TO profiler_reader;
REVOKE ALL ON FUNCTION
    pg_profiler_top_consumers(text, timestamptz, timestamptz, int),
    pg_profiler_overall_consumption(timestamptz, timestamptz, interval),
    pg_profiler_high_variation(timestamptz, timestamptz, int, bigint),
    pg_profiler_query_history(bigint, timestamptz, timestamptz, oid),
    pg_profiler_regressed_queries(timestamptz, timestamptz, timestamptz, timestamptz, bigint, float8, float8, int),
    pg_profiler_wait_stats(timestamptz, timestamptz, int)
  FROM PUBLIC;
"#,
    name = "reports",
    requires = ["history"]
);
