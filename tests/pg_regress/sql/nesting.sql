-- nested statements (inside functions) are only recorded with track = all
CREATE FUNCTION regress_nested() RETURNS int LANGUAGE plpgsql AS
$$ BEGIN RETURN (SELECT 7 AS regress_marker_nested); END $$;

SET pg_profiler.track = 'top';
SELECT pg_profiler_reset();
SELECT regress_nested();
SELECT nesting = 0 AS toplevel, nesting FROM pg_profiler()
 WHERE query LIKE '%regress_marker_nested%' OR query LIKE 'SELECT regress_nested()%'
 ORDER BY seq;

SET pg_profiler.track = 'all';
SELECT pg_profiler_reset();
SELECT regress_nested();
SELECT nesting = 0 AS toplevel, nesting > 0 AS nested FROM pg_profiler()
 WHERE query LIKE '%regress_marker_nested%' OR query LIKE 'SELECT regress_nested()%'
 ORDER BY seq;

SET pg_profiler.track = 'none';
SELECT pg_profiler_reset();
SELECT regress_nested();
SELECT count(*) FROM pg_profiler();
