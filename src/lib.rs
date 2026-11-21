use pgrx::prelude::*;

mod archiver;
mod backend;
mod events;
mod gucs;
mod plan;
mod hooks;
mod record;
mod rules;
mod running;
mod ring;
mod sampler;
mod sql;
mod sql_stats;
mod stats;

::pgrx::pg_module_magic!(name, version);

#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    // The ring buffer and hooks need postmaster-time setup, so the library
    // must be in shared_preload_libraries. If it is merely loaded (e.g. by
    // CREATE EXTENSION) we do nothing; SQL functions then raise a clear error.
    if unsafe { !pg_sys::process_shared_preload_libraries_in_progress } {
        return;
    }
    gucs::register();
    ring::install();
    hooks::install();
    archiver::register();
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_guc_defaults() {
        assert_eq!(
            Spi::get_one::<String>("SHOW pg_profiler.track").unwrap().as_deref(),
            Some("top")
        );
        assert_eq!(
            Spi::get_one::<String>("SHOW pg_profiler.buffer_size").unwrap().as_deref(),
            Some("10000")
        );
    }

    #[pg_test]
    fn test_info_shows_capacity() {
        let cap = Spi::get_one::<i64>("SELECT capacity FROM pg_profiler_info()").unwrap();
        assert_eq!(cap, Some(10000));
        let len = Spi::get_one::<i32>("SELECT max_query_len FROM pg_profiler_info()").unwrap();
        assert_eq!(len, Some(2048));
    }

    #[pg_test]
    fn test_select_is_recorded() {
        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        Spi::run("SELECT pg_profiler_reset()").unwrap();
        Spi::run("SELECT 42 AS pg_profiler_marker_4").unwrap();
        let rows = Spi::get_one::<i64>(
            "SELECT count(*) FROM pg_profiler() WHERE query LIKE '%pg_profiler_marker_4%' AND query NOT LIKE '%pg_profiler()%'",
        )
        .unwrap();
        assert_eq!(rows, Some(1));
        let (rows, cmd, err) = Spi::get_three::<i64, String, bool>(
            "SELECT rows, cmd_type, sqlstate IS NOT NULL FROM pg_profiler() WHERE query LIKE 'SELECT 42 AS pg_profiler_marker_4%'",
        )
        .unwrap();
        assert_eq!(rows, Some(1));
        assert_eq!(cmd.as_deref(), Some("select"));
        assert_eq!(err, Some(false));
    }

    /// Guards the time units: PG 19 counts `instr_time` in CPU ticks, which
    /// once made a 200 ms sleep show up as 700 ms.
    #[pg_test]
    fn test_exec_time_is_wall_clock_milliseconds() {
        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        Spi::run("SELECT pg_profiler_reset()").unwrap();
        Spi::run("SELECT pg_sleep(0.2) /* pg_profiler_marker_time */").unwrap();
        let ms = Spi::get_one::<f64>(
            "SELECT exec_time_ms FROM pg_profiler() WHERE query LIKE '%pg_profiler_marker_time%' AND query NOT LIKE '%pg_profiler()%'",
        )
        .unwrap()
        .unwrap();
        assert!((195.0..400.0).contains(&ms), "exec_time_ms = {ms}");
    }

    #[pg_test]
    fn test_reset_hides_rows() {
        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        Spi::run("SELECT 1 AS pg_profiler_marker_reset").unwrap();
        Spi::run("SELECT pg_profiler_reset()").unwrap();
        let n = Spi::get_one::<i64>(
            "SELECT count(*) FROM pg_profiler() WHERE query LIKE 'SELECT 1 AS pg_profiler_marker_reset%'",
        )
        .unwrap();
        assert_eq!(n, Some(0));
    }

    fn count_marker(marker: &str) -> i64 {
        Spi::get_one::<i64>(&format!(
            "SELECT count(*) FROM pg_profiler() WHERE query LIKE '%{marker}%'              AND query NOT LIKE '%pg_profiler()%'"
        ))
        .unwrap()
        .unwrap()
    }

    #[pg_test]
    fn test_nested_recorded_only_with_track_all() {
        Spi::run(
            "CREATE FUNCTION pg_profiler_nested_fn() RETURNS int LANGUAGE plpgsql AS              $$ BEGIN RETURN (SELECT 7 AS pg_profiler_marker_nested); END $$",
        )
        .unwrap();
        // The test body itself runs nested inside the top-level test call, so
        // with track = top nothing it executes is recorded.
        Spi::run("SET pg_profiler.track = 'top'").unwrap();
        Spi::run("SELECT pg_profiler_nested_fn()").unwrap();
        assert_eq!(count_marker("pg_profiler_marker_nested"), 0);

        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        Spi::run("SELECT pg_profiler_nested_fn()").unwrap();
        // the inner SELECT inside the function body
        assert_eq!(count_marker("pg_profiler_marker_nested"), 1);
        let nesting = Spi::get_one::<i32>(
            "SELECT nesting FROM pg_profiler() WHERE query LIKE '%pg_profiler_marker_nested%'              AND query NOT LIKE '%pg_profiler()%' AND query NOT LIKE 'CREATE%'",
        )
        .unwrap()
        .unwrap();
        assert!(nesting >= 1);
    }

    #[pg_test]
    fn test_runtime_error_is_recorded() {
        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        Spi::run(
            "DO $$ BEGIN PERFORM 1 / g AS pg_profiler_marker_err FROM generate_series(0, 0) g;              EXCEPTION WHEN division_by_zero THEN NULL; END $$",
        )
        .unwrap();
        let state = Spi::get_one::<String>(
            "SELECT sqlstate FROM pg_profiler() WHERE sqlstate IS NOT NULL              AND query LIKE '%pg_profiler_marker_err%' AND query NOT LIKE 'DO%'              AND query NOT LIKE '%pg_profiler()%'",
        )
        .unwrap();
        assert_eq!(state.as_deref(), Some("22012"));
    }

    #[pg_test]
    fn test_plan_time_cpu_and_text_slice() {
        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        // two statements in one string: only the second one's text is stored
        Spi::run("SELECT 1 AS pg_profiler_marker_first; SELECT 2 AS pg_profiler_marker_slice")
            .unwrap();
        let q = Spi::get_one::<String>(
            "SELECT query FROM pg_profiler() WHERE query LIKE '%pg_profiler_marker_slice%'              AND query NOT LIKE '%pg_profiler()%' ORDER BY seq DESC LIMIT 1",
        )
        .unwrap();
        assert_eq!(q.as_deref(), Some("SELECT 2 AS pg_profiler_marker_slice"));

        // a CPU-bound statement reports non-zero cpu and plan time
        Spi::run("SELECT count(*) AS pg_profiler_marker_cpu FROM generate_series(1, 2000000)")
            .unwrap();
        let (cpu, plan) = Spi::get_two::<f64, f64>(
            "SELECT cpu_user_ms + cpu_sys_ms, plan_time_ms FROM pg_profiler()              WHERE query LIKE '%pg_profiler_marker_cpu%' AND query NOT LIKE '%pg_profiler()%'              ORDER BY seq DESC LIMIT 1",
        )
        .unwrap();
        assert!(cpu.unwrap() > 0.0, "cpu {cpu:?}");
        assert!(plan.unwrap() > 0.0, "plan {plan:?}");
    }

    #[pg_test]
    fn test_planner_error_is_recorded() {
        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        // 1/0 is constant-folded, so it fails in the planner
        Spi::run(
            "DO $$ BEGIN PERFORM 1 / 0 AS pg_profiler_marker_planerr;              EXCEPTION WHEN division_by_zero THEN NULL; END $$",
        )
        .unwrap();
        let state = Spi::get_one::<String>(
            "SELECT sqlstate FROM pg_profiler() WHERE sqlstate IS NOT NULL              AND query LIKE '%pg_profiler_marker_planerr%' AND query NOT LIKE 'DO%'              AND query NOT LIKE '%pg_profiler()%'",
        )
        .unwrap();
        assert_eq!(state.as_deref(), Some("22012"));
    }

    #[pg_test]
    fn test_query_text_hidden_from_other_users() {
        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        Spi::run("CREATE ROLE profiler_peon").unwrap();
        Spi::run("SELECT 1 AS pg_profiler_marker_secret").unwrap();
        let visible = Spi::get_one::<i64>(
            "SELECT count(*) FROM pg_profiler() WHERE query LIKE 'SELECT 1 AS pg_profiler_marker_secret%'",
        )
        .unwrap();
        assert_eq!(visible, Some(1));

        Spi::run("SET LOCAL ROLE profiler_peon").unwrap();
        let visible = Spi::get_one::<i64>(
            "SELECT count(*) FROM pg_profiler() WHERE query LIKE '%pg_profiler_marker_secret%'",
        )
        .unwrap();
        assert_eq!(visible, Some(0));
        let masked = Spi::get_one::<i64>(
            "SELECT count(*) FROM pg_profiler() WHERE query = '<insufficient privilege>'",
        )
        .unwrap();
        assert!(masked.unwrap() >= 1);
        Spi::run("RESET ROLE").unwrap();
    }

    #[pg_test]
    fn test_utility_statements_recorded_execute_not_doubled() {
        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        Spi::run("CREATE TABLE pg_profiler_t7 (a int)").unwrap();
        Spi::run("ALTER TABLE pg_profiler_t7 ADD COLUMN b int").unwrap();
        for stmt in ["CREATE TABLE pg_profiler_t7%", "ALTER TABLE pg_profiler_t7%"] {
            let kind = Spi::get_one::<String>(&format!(
                "SELECT cmd_type FROM pg_profiler() WHERE query LIKE '{stmt}'"
            ))
            .unwrap();
            assert_eq!(kind.as_deref(), Some("utility"), "{stmt}");
        }

        Spi::run("PREPARE pg_profiler_p AS SELECT 1 AS pg_profiler_marker_prep").unwrap();
        Spi::run("EXECUTE pg_profiler_p").unwrap();
        assert_eq!(count_marker("pg_profiler_marker_prep"), 1);
        let utility_rows = Spi::get_one::<i64>(
            "SELECT count(*) FROM pg_profiler() WHERE query LIKE 'EXECUTE pg_profiler_p%'",
        )
        .unwrap();
        assert_eq!(utility_rows, Some(0));
    }

    #[pg_test]
    fn test_since_cursor_reads_new_rows_oldest_first() {
        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        let next = Spi::get_one::<i64>("SELECT next_seq FROM pg_profiler_info()").unwrap().unwrap();
        Spi::run("SELECT 1 AS pg_profiler_marker_since_a").unwrap();
        Spi::run("SELECT 2 AS pg_profiler_marker_since_b").unwrap();
        let ordered = Spi::get_one::<bool>(&format!(
            "SELECT array_agg(query ORDER BY seq) FILTER (WHERE query LIKE '%pg_profiler_marker_since%'                     AND query NOT LIKE '%pg_profiler_since%')                     = ARRAY['SELECT 1 AS pg_profiler_marker_since_a', 'SELECT 2 AS pg_profiler_marker_since_b']                FROM pg_profiler_since({}, 1000)",
            next - 1
        ))
        .unwrap();
        assert_eq!(ordered, Some(true));
    }

    #[pg_test]
    fn test_session_and_transaction_context() {
        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        Spi::run("SET application_name = 'pgp_test_app'").unwrap();
        Spi::run("SELECT 1 AS pg_profiler_marker_ctx").unwrap();
        let (app, client, xact_start_set) = Spi::get_three::<String, String, bool>(
            "SELECT application_name, client_addr, xact_start IS NOT NULL FROM pg_profiler()              WHERE query LIKE 'SELECT 1 AS pg_profiler_marker_ctx%' ORDER BY seq DESC LIMIT 1",
        )
        .unwrap();
        assert_eq!(app.as_deref(), Some("pgp_test_app"));
        assert!(client.is_some(), "client_addr should be set");
        assert_eq!(xact_start_set, Some(true));
    }

    #[pg_test]
    fn test_running_shows_own_statement() {
        let n = Spi::get_one::<i64>(
            "SELECT count(*) FROM pg_profiler_running()              WHERE pid = pg_backend_pid() AND query LIKE '%test_running_shows_own_statement%'",
        )
        .unwrap();
        assert_eq!(n, Some(1));
    }

    #[pg_test]
    fn test_plan_text_hidden_from_other_users() {
        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        Spi::run("CREATE ROLE profiler_plan_peon").unwrap();
        Spi::run("SELECT pg_profiler_reset()").unwrap();
        Spi::run("SELECT 1 + 2 AS pg_profiler_marker_plan").unwrap();
        let mine = Spi::get_one::<i64>(
            "SELECT count(*) FROM pg_profiler_plans() WHERE query LIKE 'SELECT 1 + 2 AS pg_profiler_marker_plan%' AND plan LIKE '%Result%'",
        )
        .unwrap();
        assert_eq!(mine, Some(1));

        Spi::run("SET LOCAL ROLE profiler_plan_peon").unwrap();
        let leaked = Spi::get_one::<i64>(
            "SELECT count(*) FROM pg_profiler_plans() WHERE (plan LIKE '%Result%' OR query LIKE '%pg_profiler_marker_plan%') AND query NOT LIKE '%pg_profiler_plans%'",
        )
        .unwrap();
        assert_eq!(leaked, Some(0));
        let masked = Spi::get_one::<i64>(
            "SELECT count(*) FROM pg_profiler_plans() WHERE plan = '<insufficient privilege>' AND query = '<insufficient privilege>'",
        )
        .unwrap();
        assert!(masked.unwrap() >= 1);
        Spi::run("RESET ROLE").unwrap();
    }

    #[pg_test]
    fn test_bind_parameters_are_captured_when_enabled() {
        Spi::run("SET pg_profiler.track = 'all'").unwrap();
        Spi::run("SET pg_profiler.capture_params = on").unwrap();
        Spi::run_with_args(
            "SELECT $1::int AS pg_profiler_marker_params",
            &[pgrx::datum::DatumWithOid::from(42i32)],
        )
        .unwrap();
        let params = Spi::get_one::<String>(
            "SELECT params FROM pg_profiler() WHERE query LIKE 'SELECT $1::int AS pg_profiler_marker_params%'              ORDER BY seq DESC LIMIT 1",
        )
        .unwrap();
        assert_eq!(params.as_deref(), Some("$1 = '42'"));

        Spi::run("SET pg_profiler.capture_params = off").unwrap();
        Spi::run_with_args(
            "SELECT $1::int AS pg_profiler_marker_noparams",
            &[pgrx::datum::DatumWithOid::from(7i32)],
        )
        .unwrap();
        let none = Spi::get_one::<String>(
            "SELECT params FROM pg_profiler() WHERE query LIKE 'SELECT $1::int AS pg_profiler_marker_noparams%'              ORDER BY seq DESC LIMIT 1",
        )
        .unwrap();
        assert_eq!(none, None);
    }
}

/// This module is required by `cargo pgrx test` invocations.
/// It must be visible at the root of your extension crate.
#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {}

    #[must_use]
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        vec!["shared_preload_libraries='pg_profiler'", "pg_profiler.max_param_len=256"]
    }
}
