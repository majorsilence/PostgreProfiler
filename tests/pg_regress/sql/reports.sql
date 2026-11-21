-- Query Store style reports over synthetic history (fixed 2020 timestamps)
SET timezone = 'UTC';
CREATE FUNCTION regress_synth(q bigint, p bigint, ts timestamptz, calls bigint, mean float8, sd float8,
                              waits int[] DEFAULT NULL) RETURNS void
LANGUAGE sql AS $$
    INSERT INTO pg_profiler_hist_stats
    SELECT ts, 1, 10, q, p, calls, 0, calls * mean, calls * (mean * mean + sd * sd), mean - sd, mean + sd, 0,
           calls, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
           (SELECT array_agg(CASE WHEN g = least(60, floor(ln(mean * 1000) / ln(2) * 2)::int + 1)
                                  THEN calls::int ELSE 0 END ORDER BY g) FROM generate_series(1, 60) g),
           coalesce(waits, array_fill(0, ARRAY[12]))
$$;
SELECT pg_profiler_ensure_partition(timestamptz '2020-02-01 00:00:00+00');

-- baseline: ten hourly intervals; recent: the two after
SELECT regress_synth(7001, 1, t, 100, 10, 1) FROM generate_series(timestamptz '2020-02-01 00:00:00+00', '2020-02-01 09:00:00+00', interval '1 hour') t;
SELECT regress_synth(7001, 2, t, 100, 30, 3) FROM generate_series(timestamptz '2020-02-01 10:00:00+00', '2020-02-01 11:00:00+00', interval '1 hour') t;
SELECT regress_synth(7002, 1, t, 200, 10, 1) FROM generate_series(timestamptz '2020-02-01 00:00:00+00', '2020-02-01 11:00:00+00', interval '1 hour') t;
SELECT regress_synth(7003, 1, t, 100, 10, 20) FROM generate_series(timestamptz '2020-02-01 00:00:00+00', '2020-02-01 09:00:00+00', interval '1 hour') t;
SELECT regress_synth(7003, 1, t, 10, 15, 20) FROM generate_series(timestamptz '2020-02-01 10:00:00+00', '2020-02-01 11:00:00+00', interval '1 hour') t;
SELECT regress_synth(7004, 1, t, 100, 10, 1) FROM generate_series(timestamptz '2020-02-01 00:00:00+00', '2020-02-01 09:00:00+00', interval '1 hour') t;
SELECT regress_synth(7004, 1, t, 3, 50, 1) FROM generate_series(timestamptz '2020-02-01 10:00:00+00', '2020-02-01 11:00:00+00', interval '1 hour') t;
SELECT regress_synth(7005, 1, t, 50, 100, 5, ARRAY[30,0,70,0,0,0,0,0,0,0,0,0]) FROM generate_series(timestamptz '2020-02-01 00:00:00+00', '2020-02-01 02:00:00+00', interval '1 hour') t;
INSERT INTO pg_profiler_hist_queries
SELECT 1, q, 'SELECT report_marker_' || q, timestamptz '2020-02-01', timestamptz '2020-02-01 12:00:00+00'
  FROM generate_series(7001, 7005) q;

-- regressed: 7001 (3x slower, and a new plan). 7003 is too noisy, 7004 has too few recent calls.
SELECT queryid, baseline_calls, recent_calls, round(baseline_avg_ms::numeric, 1) AS baseline_avg,
       round(recent_avg_ms::numeric, 1) AS recent_avg, round(ratio::numeric, 1) AS ratio,
       z_score > 10 AS significant, round(added_time_ms::numeric, 0) AS added_ms,
       plan_changed, baseline_plans, recent_plans, query
  FROM pg_profiler_regressed_queries(
         '2020-02-01 10:00+00', '2020-02-01 12:00+00', '2020-02-01 00:00+00', '2020-02-01 10:00+00')
 WHERE queryid BETWEEN 7000 AND 7999;
-- looser thresholds also list the noisy one
SELECT queryid, round(ratio::numeric, 1) AS ratio, round(z_score::numeric, 1) AS z
  FROM pg_profiler_regressed_queries(
         '2020-02-01 10:00+00', '2020-02-01 12:00+00', '2020-02-01 00:00+00', '2020-02-01 10:00+00',
         min_z => 1)
 WHERE queryid BETWEEN 7000 AND 7999 ORDER BY queryid;

-- top consumers by total time, by calls, and an unknown metric
SELECT queryid, calls, round(total_exec_ms::numeric, 0) AS total_ms, round(avg_exec_ms::numeric, 1) AS avg_ms,
       round(stddev_exec_ms::numeric, 1) AS sd_ms, round(p95_exec_ms::numeric, 0) AS p95_ms, plans, round(metric_value::numeric, 0) AS metric
  FROM pg_profiler_top_consumers('exec_time', '2020-02-01 00:00+00', '2020-02-02 00:00+00', 100)
 WHERE queryid BETWEEN 7000 AND 7999;
SELECT queryid, metric_value FROM pg_profiler_top_consumers('calls', '2020-02-01 00:00+00', '2020-02-02 00:00+00', 100)
 WHERE queryid BETWEEN 7000 AND 7999 LIMIT 2;
SELECT * FROM pg_profiler_top_consumers('speed');

-- queries with the most variation
SELECT queryid, calls, round(cv::numeric, 1) AS cv, round(p50_exec_ms::numeric, 0) AS p50, round(p99_exec_ms::numeric, 0) AS p99
  FROM pg_profiler_high_variation('2020-02-01 00:00+00', '2020-02-02 00:00+00', 100, 10)
 WHERE queryid BETWEEN 7000 AND 7999 LIMIT 3;

-- overall consumption per 6 hours
SELECT bucket_start, calls, distinct_queries
  FROM pg_profiler_overall_consumption('2020-02-01 00:00+00', '2020-02-01 12:00+00', interval '6 hours')
 ORDER BY 1;

-- one query over time, per plan
SELECT interval_start, plan_id, calls, round(avg_exec_ms::numeric, 1) AS avg_ms
  FROM pg_profiler_query_history(7001, '2020-02-01 08:00+00', '2020-02-01 12:00+00')
 ORDER BY 1;

-- where statements wait
SELECT queryid, category, samples, round(pct_of_query::numeric, 0) AS pct
  FROM pg_profiler_wait_stats('2020-02-01 00:00+00', '2020-02-02 00:00+00', 10)
 WHERE queryid = 7005;

-- readers only
CREATE ROLE regress_reports_plain;
SET ROLE regress_reports_plain;
SELECT count(*) FROM pg_profiler_top_consumers('exec_time', '2020-02-01 00:00+00', '2020-02-02 00:00+00', 5);
RESET ROLE;
DROP ROLE regress_reports_plain;
