-- a session is announced by a login event on its first statement
SET pg_profiler.track = 'top';
SELECT kind, client_addr IS NOT NULL AS has_client
  FROM pg_profiler_events()
 WHERE pid = pg_backend_pid() AND kind = 'login';

-- statement timeouts and user cancels are cancel events
SET statement_timeout = 20;
SELECT pg_sleep(2);
RESET statement_timeout;
SELECT kind, sqlstate, detail
  FROM pg_profiler_events()
 WHERE pid = pg_backend_pid() AND kind = 'cancel';

-- the statement itself is an error row too
SELECT is_error, sqlstate FROM pg_profiler() WHERE query LIKE 'SELECT pg_sleep(2)%';

-- running statements: this backend shows up with its own statement
SELECT pid = pg_backend_pid() AS own, cmd_type, query
  FROM pg_profiler_activity WHERE pid = pg_backend_pid();
