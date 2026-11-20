SET pg_profiler.track = 'top';
SELECT pg_profiler_reset();
SELECT pg_sleep(0.05), 1 AS regress_marker_view_slow;
SELECT 1 / g AS regress_marker_view_err FROM generate_series(0, 0) g;

SELECT count(*) AS recent FROM pg_profiler_recent WHERE query LIKE 'SELECT pg_sleep(0.05)%';
SELECT query FROM pg_profiler_errors WHERE query LIKE '%regress_marker_view_err%';
SELECT query LIKE 'SELECT pg_sleep(0.05)%' AS slowest_is_sleep
  FROM pg_profiler_slowest WHERE query LIKE '%regress_marker_view%'
 ORDER BY exec_time_ms DESC LIMIT 1;
SELECT count(*) <= 100 AS slowest_limited FROM pg_profiler_slowest;
