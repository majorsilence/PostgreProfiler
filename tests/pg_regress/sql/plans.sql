-- plan_id fingerprints the plan shape; new plans and outliers are stored
SET pg_profiler.track = 'top';
SET pg_profiler.plan_format = 'text';
SELECT pg_profiler_reset();

CREATE TEMP TABLE regress_p (id int PRIMARY KEY, grp int, note text);
INSERT INTO regress_p SELECT g, g % 10, 'x' FROM generate_series(1, 5000) g;
ANALYZE regress_p;

-- constants do not change the plan id
SELECT * FROM regress_p WHERE id = 1;
SELECT * FROM regress_p WHERE id = 2;
SELECT count(*) AS runs, count(DISTINCT plan_id) AS plan_ids, bool_and(plan_id <> 0) AS has_plan_id
  FROM pg_profiler()
 WHERE query LIKE 'SELECT * FROM regress_p WHERE id = %';

-- a different access path is a different plan for the same query
SET enable_indexscan = off;
SET enable_bitmapscan = off;
SELECT * FROM regress_p WHERE id = 3;
RESET enable_indexscan;
RESET enable_bitmapscan;
SELECT count(*) AS runs, count(DISTINCT plan_id) AS plan_ids, count(DISTINCT queryid) AS query_ids
  FROM pg_profiler()
 WHERE query LIKE 'SELECT * FROM regress_p WHERE id = %';
SELECT plans, calls
  FROM pg_profiler_plan_changes
 WHERE query LIKE 'SELECT * FROM regress_p WHERE id = %';
SELECT calls, avg_exec_ms >= 0 AS has_time
  FROM pg_profiler_plan_stats
 WHERE query LIKE 'SELECT * FROM regress_p WHERE id = %'
 ORDER BY calls DESC;

-- each new plan is stored once
SELECT reason, has_actuals,
       plan LIKE '%Index Scan using regress_p_pkey on regress_p%' AS index_scan,
       plan LIKE '%Seq Scan on regress_p%' AS seq_scan,
       plan_id <> 0 AS has_plan_id
  FROM pg_profiler_plans()
 WHERE query LIKE 'SELECT * FROM regress_p WHERE id = %'
 ORDER BY seq;

-- sampled statements with a badly wrong estimate are stored again, with actual rows
CREATE TEMP TABLE regress_m AS SELECT g AS a, g AS b FROM generate_series(1, 100000) g;
ANALYZE regress_m;
SET pg_profiler.plan_sample_rate = 1;
SET pg_profiler.plan_misestimate_ratio = 10;
SELECT count(*) FROM regress_m WHERE a < 3000 AND b < 3000;
SELECT reason, has_actuals, max_misestimate > 10 AS bad_estimate,
       plan LIKE '%actual rows=%' AS shows_actual
  FROM pg_profiler_plans()
 WHERE query LIKE 'SELECT count(*) FROM regress_m WHERE a < 3000%'
 ORDER BY seq;
-- the same outlier is not stored twice
SELECT count(*) FROM regress_m WHERE a < 3000 AND b < 3000;
SELECT count(*) AS stored
  FROM pg_profiler_plans()
 WHERE query LIKE 'SELECT count(*) FROM regress_m WHERE a < 3000%';
RESET pg_profiler.plan_misestimate_ratio;

-- a statement that writes temporary files is stored too
SET work_mem = '64kB';
SELECT sum(a) FROM (SELECT a FROM regress_m ORDER BY b OFFSET 0) s;
RESET work_mem;
SELECT reason LIKE '%spill%' AS spill, has_actuals, temp_blks_written > 0 AS wrote_temp
  FROM pg_profiler_plans()
 WHERE query LIKE 'SELECT sum(a) FROM (SELECT a FROM regress_m ORDER BY b%'
   AND has_actuals;
RESET pg_profiler.plan_sample_rate;

-- JSON plans
SET pg_profiler.plan_format = 'json';
SELECT count(*) FROM regress_p;
SELECT format, plan::jsonb -> 0 -> 'Plan' ->> 'Node Type' AS root_node
  FROM pg_profiler_plans()
 WHERE query LIKE 'SELECT count(*) FROM regress_p%';
SET pg_profiler.plan_format = 'text';

-- reset clears the plans, and a plan seen again is stored again
SELECT pg_profiler_reset();
SELECT count(*) AS plans_after_reset FROM pg_profiler_plans()
 WHERE query NOT LIKE '%pg_profiler_plans%';
SELECT * FROM regress_p WHERE id = 4;
SELECT count(*) AS plans_after_rerun
  FROM pg_profiler_plans()
 WHERE query LIKE 'SELECT * FROM regress_p WHERE id = %';
SELECT plans_total_written > 0 AS has_stats, plans_capacity FROM pg_profiler_info();
