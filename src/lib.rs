use pgrx::prelude::*;

mod backend;
mod gucs;
mod hooks;
mod record;
mod ring;
mod sql;

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
            "SELECT rows, cmd_type, is_error FROM pg_profiler() WHERE query LIKE 'SELECT 42 AS pg_profiler_marker_4%'",
        )
        .unwrap();
        assert_eq!(rows, Some(1));
        assert_eq!(cmd.as_deref(), Some("select"));
        assert_eq!(err, Some(false));
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
            "SELECT sqlstate FROM pg_profiler() WHERE is_error              AND query LIKE '%pg_profiler_marker_err%' AND query NOT LIKE 'DO%'              AND query NOT LIKE '%pg_profiler()%'",
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
            "SELECT sqlstate FROM pg_profiler() WHERE is_error              AND query LIKE '%pg_profiler_marker_planerr%' AND query NOT LIKE 'DO%'              AND query NOT LIKE '%pg_profiler()%'",
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
}

/// This module is required by `cargo pgrx test` invocations.
/// It must be visible at the root of your extension crate.
#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {}

    #[must_use]
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        vec!["shared_preload_libraries='pg_profiler'"]
    }
}
