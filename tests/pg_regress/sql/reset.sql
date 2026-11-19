SET pg_profiler.track = 'top';
SELECT 1 AS regress_marker_reset;
SELECT pg_profiler_reset();
SELECT count(*) FROM pg_profiler() WHERE query LIKE '%regress_marker_reset%';
SELECT capacity, max_query_len FROM pg_profiler_info();
