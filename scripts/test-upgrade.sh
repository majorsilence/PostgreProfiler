#!/usr/bin/env bash
# Check that upgrading from 0.1.0 gives the same objects as a fresh install.
# Needs the `db` service running (docker compose up --build -d db). The 0.1.0
# install script is kept in tests/upgrade/ (it is not part of the package).
set -euo pipefail
export MSYS_NO_PATHCONV=1   # Git Bash on Windows would rewrite container paths
dc() { docker compose exec -T db "$@"; }
sql() { dc psql -U postgres -v ON_ERROR_STOP=1 -qAt "$@"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

ext_dir=$(dc pg_config --sharedir | tr -d '\r')/extension
docker compose cp tests/upgrade/pg_profiler--0.1.0.sql "db:$ext_dir/"
dc test -f "$ext_dir/pg_profiler--0.1.0--0.2.0.sql" || fail "upgrade script is not part of the package"

sql -d postgres -c "DROP DATABASE IF EXISTS upg_old" -c "DROP DATABASE IF EXISTS upg_new" \
    -c "CREATE DATABASE upg_old" -c "CREATE DATABASE upg_new"
sql -d upg_old -c "CREATE EXTENSION pg_profiler VERSION '0.1.0'"
v=$(sql -d upg_old -c "SELECT extversion FROM pg_extension WHERE extname = 'pg_profiler'")
[ "$v" = "0.1.0" ] || fail "expected to start at 0.1.0, got $v"
sql -d upg_old -c "ALTER EXTENSION pg_profiler UPDATE"
sql -d upg_new -c "CREATE EXTENSION pg_profiler"

# everything an extension member can differ in: definitions, columns, views, privileges
shape() {
  sql -d "$1" -c "
    SELECT 'extversion ' || extversion FROM pg_extension WHERE extname = 'pg_profiler'
    UNION ALL
    SELECT 'function ' || p.oid::regprocedure::text || ' acl=' || coalesce(p.proacl::text, '-') || E'\n' || pg_get_functiondef(p.oid)
      FROM pg_proc p JOIN pg_depend d ON d.objid = p.oid AND d.classid = 'pg_proc'::regclass AND d.deptype = 'e'
      JOIN pg_extension e ON e.oid = d.refobjid AND e.extname = 'pg_profiler' WHERE p.prokind <> 'a'
    UNION ALL
    SELECT 'aggregate ' || p.oid::regprocedure::text || ' ' || a.aggtransfn::text || ' ' || a.aggtranstype::regtype::text
      FROM pg_aggregate a JOIN pg_proc p ON p.oid = a.aggfnoid JOIN pg_depend d ON d.objid = p.oid AND d.deptype = 'e'
      JOIN pg_extension e ON e.oid = d.refobjid AND e.extname = 'pg_profiler'
    UNION ALL
    SELECT 'relation ' || c.relname || ' kind=' || c.relkind::text || ' acl=' || coalesce(c.relacl::text, '-') || E'\n' ||
           coalesce((SELECT string_agg(a.attname || ' ' || format_type(a.atttypid, a.atttypmod) || CASE WHEN a.attnotnull THEN ' not null' ELSE '' END, ', ' ORDER BY a.attnum)
                       FROM pg_attribute a WHERE a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped), '') ||
           CASE WHEN c.relkind = 'v' THEN E'\n' || pg_get_viewdef(c.oid) ELSE '' END
      FROM pg_class c JOIN pg_depend d ON d.objid = c.oid AND d.classid = 'pg_class'::regclass AND d.deptype = 'e'
      JOIN pg_extension e ON e.oid = d.refobjid AND e.extname = 'pg_profiler' WHERE c.relkind IN ('r', 'p', 'v')
    UNION ALL
    SELECT 'constraint ' || c.conrelid::regclass::text || ' ' || pg_get_constraintdef(c.oid)
      FROM pg_constraint c WHERE c.conrelid IN (SELECT objid FROM pg_depend d JOIN pg_extension e ON e.oid = d.refobjid
        AND e.extname = 'pg_profiler' WHERE d.classid = 'pg_class'::regclass)
    ORDER BY 1" | tr -d '\r'
}
shape upg_old > /tmp/shape_old.txt
shape upg_new > /tmp/shape_new.txt
[ -s /tmp/shape_new.txt ] || fail "no objects found"
diff /tmp/shape_old.txt /tmp/shape_new.txt || fail "the upgraded database differs from a fresh install"

api=$(sql -d upg_old -c "SELECT pg_profiler_api_version()")
sql -d upg_old -c "SELECT count(*) FROM pg_profiler_stats_memory()" >/dev/null
sql -d postgres -c "DROP DATABASE upg_old" -c "DROP DATABASE upg_new"
echo "upgrade 0.1.0 -> 0.2.0 OK (API version $api, $(wc -l < /tmp/shape_new.txt) lines compared)"
