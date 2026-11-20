-- utility statements are recorded; EXECUTE/PREPARE are not double-counted
SET pg_profiler.track = 'top';
SELECT pg_profiler_reset();
CREATE TABLE regress_util (a int);
ALTER TABLE regress_util ADD COLUMN b text;
PREPARE regress_prep AS SELECT 1 AS regress_marker_prep;
EXECUTE regress_prep;
EXECUTE regress_prep;
DEALLOCATE regress_prep;
DROP TABLE regress_util;

SELECT cmd_type, rows, sqlstate IS NOT NULL AS is_error, query
  FROM pg_profiler()
 WHERE query LIKE '%regress_util%' OR query LIKE '%regress_prep%' OR query LIKE '%regress_marker_prep%'
 ORDER BY seq;

-- with track_utility off, DDL is not recorded
SET pg_profiler.track_utility = off;
SELECT pg_profiler_reset();
CREATE TABLE regress_util2 (a int);
DROP TABLE regress_util2;
SELECT count(*) FROM pg_profiler() WHERE query LIKE '%regress_util2%';

-- a failing utility statement is recorded with its SQLSTATE
SET pg_profiler.track_utility = on;
SELECT pg_profiler_reset();
DROP TABLE regress_does_not_exist;
SELECT cmd_type, sqlstate IS NOT NULL AS is_error, sqlstate FROM pg_profiler()
 WHERE query LIKE 'DROP TABLE regress_does_not_exist%';
