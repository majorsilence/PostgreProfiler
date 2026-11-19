use pgrx::prelude::*;

mod gucs;

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
