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
