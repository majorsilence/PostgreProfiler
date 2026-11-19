//! SQL-facing functions and views.

use crate::ring;
use pgrx::prelude::*;

#[pg_extern]
fn pg_profiler_info() -> TableIterator<
    'static,
    (
        name!(capacity, i64),
        name!(total_written, i64),
        name!(retained, i64),
        name!(overwritten, i64),
        name!(max_query_len, i32),
    ),
> {
    let s = ring::stats();
    TableIterator::once((s.capacity, s.total_written, s.retained, s.overwritten, s.max_query_len))
}

#[pg_extern]
fn pg_profiler_reset() {
    ring::reset();
}

extension_sql!(
    r#"REVOKE ALL ON FUNCTION pg_profiler_reset() FROM PUBLIC;"#,
    name = "revoke_reset",
    requires = [pg_profiler_reset]
);
