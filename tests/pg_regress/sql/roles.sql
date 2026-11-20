-- roles: admin functions need profiler_admin, query text needs profiler_reader
CREATE ROLE regress_plain;
SET pg_profiler.track = 'top';
SELECT pg_profiler_api_version() AS api_version;
SELECT 1 AS regress_marker_roles;

SET ROLE regress_plain;
SELECT pg_profiler_reset();
SELECT pg_profiler_rule_add(query_contains => 'x');
SELECT count(*) AS visible FROM pg_profiler() WHERE query LIKE 'SELECT 1 AS regress_marker_roles%';
SELECT count(*) > 0 AS masked FROM pg_profiler() WHERE query = '<insufficient privilege>';
RESET ROLE;

GRANT profiler_reader TO regress_plain;
SET ROLE regress_plain;
SELECT count(*) AS visible FROM pg_profiler() WHERE query LIKE 'SELECT 1 AS regress_marker_roles%';
SELECT pg_profiler_reset();
RESET ROLE;

GRANT profiler_admin TO regress_plain;
SET ROLE regress_plain;
SELECT pg_profiler_reset();
RESET ROLE;
REVOKE profiler_admin FROM regress_plain;
REVOKE profiler_reader FROM regress_plain;
DROP ROLE regress_plain;
