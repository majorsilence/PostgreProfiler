-- Upgrade pg_profiler 0.1.0 -> 0.2.0: interval statistics and history (Query Store),
-- reports and the wait sampler. Only additions: nothing from 0.1.0 changes.
-- Written from the difference of the generated install scripts; scripts/test-upgrade.sh
-- checks that an upgraded database matches a fresh 0.2.0 install.

/* <begin connected objects> */
-- src/sql_stats.rs:226
-- pg_profiler::sql_stats::pg_profiler_wait_categories
CREATE  FUNCTION "pg_profiler_wait_categories"() RETURNS TEXT[] /* Vec < & '_ str > */
IMMUTABLE STRICT PARALLEL SAFE
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_profiler_wait_categories_wrapper';
/* </end connected objects> */

/* <begin connected objects> */
-- src/sql_stats.rs:133
-- pg_profiler::sql_stats::pg_profiler_stats_texts
CREATE  FUNCTION "pg_profiler_stats_texts"() RETURNS TABLE (
	"dbid" oid,  /* pg_sys :: Oid */
	"queryid" bigint,  /* i64 */
	"query" TEXT  /* Option < String > */
)
STRICT 
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_profiler_stats_texts_rows_wrapper';
/* </end connected objects> */

/* <begin connected objects> */
-- src/sql_stats.rs:184
-- pg_profiler::sql_stats::pg_profiler_stats_reset
CREATE  FUNCTION "pg_profiler_stats_reset"() RETURNS void
STRICT
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_profiler_stats_reset_wrapper';
/* </end connected objects> */

/* <begin connected objects> */
-- src/sql_stats.rs:89
-- pg_profiler::sql_stats::pg_profiler_stats_memory
CREATE  FUNCTION "pg_profiler_stats_memory"(
	"only_unarchived" bool DEFAULT false /* bool */
) RETURNS TABLE (
	"interval_start" timestamp with time zone,  /* Option < TimestampWithTimeZone > */
	"dbid" oid,  /* pg_sys :: Oid */
	"userid" oid,  /* pg_sys :: Oid */
	"queryid" bigint,  /* i64 */
	"plan_id" bigint,  /* i64 */
	"calls" bigint,  /* i64 */
	"errors" bigint,  /* i64 */
	"exec_total_ms" double precision,  /* f64 */
	"exec_sumsq_ms2" double precision,  /* f64 */
	"exec_min_ms" double precision,  /* f64 */
	"exec_max_ms" double precision,  /* f64 */
	"plan_total_ms" double precision,  /* f64 */
	"rows" bigint,  /* i64 */
	"shared_blks_hit" bigint,  /* i64 */
	"shared_blks_read" bigint,  /* i64 */
	"shared_blks_dirtied" bigint,  /* i64 */
	"shared_blks_written" bigint,  /* i64 */
	"temp_blks_read" bigint,  /* i64 */
	"temp_blks_written" bigint,  /* i64 */
	"wal_records" bigint,  /* i64 */
	"wal_fpi" bigint,  /* i64 */
	"wal_bytes" bigint,  /* i64 */
	"cpu_user_ms" double precision,  /* f64 */
	"cpu_sys_ms" double precision,  /* f64 */
	"blk_read_ms" double precision,  /* f64 */
	"blk_write_ms" double precision,  /* f64 */
	"hist" INT[],  /* Vec < i32 > */
	"wait_samples" INT[]  /* Vec < i32 > */
)
STRICT 
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_profiler_stats_memory_rows_wrapper';
/* </end connected objects> */

/* <begin connected objects> */
-- src/sql_stats.rs:148
-- pg_profiler::sql_stats::pg_profiler_stats_info
CREATE  FUNCTION "pg_profiler_stats_info"() RETURNS TABLE (
	"interval_seconds" INT,  /* i32 */
	"entries_capacity" INT,  /* i32 */
	"current_interval" timestamp with time zone,  /* Option < TimestampWithTimeZone > */
	"intervals_in_memory" INT,  /* i32 */
	"current_entries" INT,  /* i32 */
	"unarchived_intervals" INT,  /* i32 */
	"rotations" bigint,  /* i64 */
	"lost_intervals" bigint,  /* i64 */
	"dropped_entries" bigint,  /* i64 */
	"texts_evicted" bigint  /* i64 */
)
STRICT 
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_profiler_stats_info_rows_wrapper';
/* </end connected objects> */

/* <begin connected objects> */
-- src/sql_stats.rs:207
-- pg_profiler::sql_stats::pg_profiler_hist_percentile
CREATE  FUNCTION "pg_profiler_hist_percentile"(
	"hist" bigint[], /* Vec < i64 > */
	"pct" double precision /* f64 */
) RETURNS double precision /* Option < f64 > */
IMMUTABLE STRICT PARALLEL SAFE
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_profiler_hist_percentile_wrapper';
/* </end connected objects> */

/* <begin connected objects> */
-- src/sql_stats.rs:192
-- pg_profiler::sql_stats::pg_profiler_hist_add
CREATE  FUNCTION "pg_profiler_hist_add"(
	"state" bigint[], /* :: std :: option :: Option < Vec < i64 > > */
	"hist" INT[] /* :: std :: option :: Option < Vec < i32 > > */
) RETURNS bigint[] /* :: std :: option :: Option < Vec < i64 > > */
IMMUTABLE PARALLEL SAFE
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_profiler_hist_add_wrapper';
/* </end connected objects> */

/* <begin connected objects> */
-- src/sql_stats.rs:231
-- requires:
--   pg_profiler_hist_add
--   pg_profiler_stats_reset
--   roles_and_grants


CREATE AGGREGATE pg_profiler_hist_agg(int4[]) (
    SFUNC = pg_profiler_hist_add,
    STYPE = int8[]
);

REVOKE ALL ON FUNCTION pg_profiler_stats_reset() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pg_profiler_stats_reset() TO profiler_admin;
/* </end connected objects> */

/* <begin connected objects> */
-- src/sql_stats.rs:247
-- requires:
--   stats_helpers
--   roles_and_grants
--   pg_profiler_stats_memory_rows
--   pg_profiler_stats_texts_rows


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
/* </end connected objects> */

/* <begin connected objects> */
-- src/sql_stats.rs:447
-- requires:
--   history


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
/* </end connected objects> */
