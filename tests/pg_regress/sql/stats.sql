-- aggregated per-interval statistics (Query Store style)
SET pg_profiler.track = 'top';
SET pg_profiler.stats_capture = 'all';
SELECT pg_profiler_stats_reset();
SELECT entries_capacity > 0 AS has_capacity, interval_seconds > 0 AS has_interval, current_entries >= 0 AS has_entries
  FROM pg_profiler_stats_info();

CREATE TEMP TABLE regress_s (a int);
INSERT INTO regress_s SELECT generate_series(1, 100);
-- one query, three constants: one entry
SELECT count(*) FROM regress_s WHERE a > 50;
SELECT count(*) FROM regress_s WHERE a > 10;
SELECT count(*) FROM regress_s WHERE a > 20;

CREATE TEMP VIEW regress_q AS
SELECT m.*, t.query
  FROM pg_profiler_stats_memory() m
  JOIN pg_profiler_stats_texts() t USING (dbid, queryid);

SELECT sum(calls) AS calls, sum(errors) AS errors, sum(rows) AS rows,
       sum(exec_total_ms) >= 0 AS has_time,
       min(exec_min_ms) <= max(exec_max_ms) AS min_le_max,
       sum(exec_sumsq_ms2) >= 0 AS has_sumsq,
       count(DISTINCT plan_id) AS plans
  FROM regress_q
 WHERE query LIKE 'SELECT count(*) FROM regress_s WHERE a > %';

-- every successful run lands in exactly one histogram bucket
SELECT (SELECT sum(x) FROM unnest(pg_profiler_hist_agg(hist)) x) AS histogram_runs,
       pg_profiler_hist_percentile(pg_profiler_hist_agg(hist), 0.5) IS NOT NULL AS has_p50,
       pg_profiler_hist_percentile(pg_profiler_hist_agg(hist), 0.99) >= pg_profiler_hist_percentile(pg_profiler_hist_agg(hist), 0.5) AS p99_ge_p50
  FROM regress_q
 WHERE query LIKE 'SELECT count(*) FROM regress_s WHERE a > %';

-- failed statements are counted as errors, not as calls
SELECT 1 / (a - a) FROM regress_s;
SELECT sum(calls) AS calls, sum(errors) AS errors
  FROM regress_q
 WHERE query LIKE 'SELECT 1 / (a - a) FROM regress_s%';

-- the whole set, as the Query Store views will use it
SELECT array_length(pg_profiler_wait_categories(), 1) AS wait_categories;

-- auto mode only starts tracking queries that have had a slow run
SET pg_profiler.stats_capture = 'auto';
SET pg_profiler.stats_auto_min_ms = 100000;
SELECT 11 AS regress_auto_fast;
SELECT count(*) AS tracked FROM regress_q WHERE query LIKE 'SELECT 11 AS regress_auto_fast%';
SET pg_profiler.stats_auto_min_ms = 0;
SELECT 12 AS regress_auto_slow;
SELECT count(*) AS tracked FROM regress_q WHERE query LIKE 'SELECT 12 AS regress_auto_slow%';

-- none switches it off
SET pg_profiler.stats_capture = 'none';
SELECT 13 AS regress_none;
SELECT count(*) AS tracked FROM regress_q WHERE query LIKE 'SELECT 13 AS regress_none%';
SET pg_profiler.stats_capture = 'all';

-- reset forgets everything in memory
SELECT pg_profiler_stats_reset();
SELECT count(*) AS entries_after_reset FROM regress_q WHERE query LIKE 'SELECT count(*) FROM regress_s%';
