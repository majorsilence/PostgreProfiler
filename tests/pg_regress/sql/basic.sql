-- top-level statements are recorded; the statement text is stored verbatim
SET pg_profiler.track = 'top';
SELECT pg_profiler_reset();
SELECT 1 AS regress_marker_basic;
SELECT cmd_type, nesting = 0 AS toplevel, nesting, rows, is_error, sqlstate
  FROM pg_profiler()
 WHERE query LIKE 'SELECT 1 AS regress_marker_basic%';
SELECT queryid <> 0 AS has_queryid, exec_time_ms >= 0 AS has_time, pid = pg_backend_pid() AS own_pid
  FROM pg_profiler()
 WHERE query LIKE 'SELECT 1 AS regress_marker_basic%';

-- DML reports row counts
CREATE TEMP TABLE regress_t (a int);
INSERT INTO regress_t SELECT generate_series(1, 5);
SELECT cmd_type, rows FROM pg_profiler()
 WHERE query LIKE 'INSERT INTO regress_t%';
