-- sampling: sample_rate = 0 records nothing, 1 records everything
SET pg_profiler.track = 'top';
SET pg_profiler.sample_rate = 0;
SELECT 1 AS regress_marker_sample0;
SET pg_profiler.sample_rate = 1;
SELECT 1 AS regress_marker_sample1;
SELECT count(*) FILTER (WHERE query LIKE 'SELECT 1 AS regress_marker_sample0%') AS sampled_out,
       count(*) FILTER (WHERE query LIKE 'SELECT 1 AS regress_marker_sample1%') AS sampled_in
  FROM pg_profiler();

-- min_duration_ms drops fast statements but never errors
SELECT pg_profiler_reset();
SET pg_profiler.min_duration_ms = 100;
SELECT 1 AS regress_marker_fast;
SELECT pg_sleep(0.15), 1 AS regress_marker_slow;
SELECT 1 / g AS regress_marker_fast_err FROM generate_series(0, 0) g;
SET pg_profiler.min_duration_ms = 0;
SELECT count(*) FILTER (WHERE query LIKE 'SELECT 1 AS regress_marker_fast%') AS fast,
       count(*) FILTER (WHERE query LIKE 'SELECT pg_sleep(0.15), 1 AS regress_marker_slow%') AS slow,
       count(*) FILTER (WHERE query LIKE 'SELECT 1 / g AS regress_marker_fast_err%' AND is_error) AS err
  FROM pg_profiler();
