-- a failing statement is recorded with its SQLSTATE
SET pg_profiler.track = 'top';
SELECT pg_profiler_reset();
SELECT 1 / g AS regress_marker_err FROM generate_series(0, 0) g;
SELECT sqlstate IS NOT NULL AS is_error, sqlstate, cmd_type, rows
  FROM pg_profiler()
 WHERE query LIKE 'SELECT 1 / g AS regress_marker_err%';

-- constant folding fails in the planner, before the executor starts
SELECT pg_profiler_reset();
SELECT 1 / 0 AS regress_marker_planerr;
SELECT sqlstate IS NOT NULL AS is_error, sqlstate, cmd_type
  FROM pg_profiler()
 WHERE query LIKE 'SELECT 1 / 0 AS regress_marker_planerr%';
