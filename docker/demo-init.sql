-- Demo setup, run once when the database volume is first created.
-- (Demo passwords only: do not use these outside a local demo.)
CREATE EXTENSION pg_profiler;

-- The UI reads with a member of profiler_reader and, for the admin actions,
-- uses a member of profiler_admin that may also signal other backends.
CREATE ROLE profiler_ui LOGIN PASSWORD 'profiler_ui' IN ROLE profiler_reader;
CREATE ROLE profiler_ui_admin LOGIN PASSWORD 'profiler_ui_admin' IN ROLE profiler_admin, pg_signal_backend;

-- State, wait events and the blocking tree of other users' sessions come from
-- pg_stat_activity, which only members of pg_read_all_stats see in full.
GRANT pg_read_all_stats TO profiler_ui, profiler_ui_admin;

-- Lock waits and deadlocks show up as events only with these.
ALTER SYSTEM SET log_lock_waits = on;
ALTER SYSTEM SET deadlock_timeout = '1s';
