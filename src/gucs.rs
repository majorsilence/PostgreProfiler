//! GUC definitions (`pg_profiler.*`).

use pgrx::guc::{GucContext, GucFlags, GucRegistry, GucSetting};
use pgrx::prelude::*;

/// Which statements are recorded.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PostgresGucEnum)]
pub enum TrackLevel {
    /// Do not record anything.
    #[name = c"none"]
    None,
    /// Only top-level statements.
    #[name = c"top"]
    Top,
    /// Top-level and nested statements (inside functions).
    #[name = c"all"]
    All,
}

pub static ENABLED: GucSetting<bool> = GucSetting::<bool>::new(true);
pub static TRACK: GucSetting<TrackLevel> = GucSetting::<TrackLevel>::new(TrackLevel::Top);
pub static TRACK_UTILITY: GucSetting<bool> = GucSetting::<bool>::new(true);
pub static SAMPLE_RATE: GucSetting<f64> = GucSetting::<f64>::new(1.0);
pub static MIN_DURATION_MS: GucSetting<f64> = GucSetting::<f64>::new(0.0);
pub static BUFFER_SIZE: GucSetting<i32> = GucSetting::<i32>::new(10_000);
pub static MAX_QUERY_LEN: GucSetting<i32> = GucSetting::<i32>::new(2048);

pub fn register() {
    GucRegistry::define_bool_guc(
        c"pg_profiler.enabled",
        c"Enables recording of statement executions.",
        c"",
        &ENABLED,
        GucContext::Suset,
        GucFlags::default(),
    );
    GucRegistry::define_enum_guc(
        c"pg_profiler.track",
        c"Selects which statements are recorded.",
        c"none: nothing, top: top-level statements only, all: also nested statements.",
        &TRACK,
        GucContext::Suset,
        GucFlags::default(),
    );
    GucRegistry::define_bool_guc(
        c"pg_profiler.track_utility",
        c"Selects whether utility commands (DDL etc.) are recorded.",
        c"",
        &TRACK_UTILITY,
        GucContext::Suset,
        GucFlags::default(),
    );
    GucRegistry::define_float_guc(
        c"pg_profiler.sample_rate",
        c"Fraction of top-level statements that are recorded.",
        c"Nested statements follow the decision made for their top-level statement.",
        &SAMPLE_RATE,
        0.0,
        1.0,
        GucContext::Suset,
        GucFlags::default(),
    );
    GucRegistry::define_float_guc(
        c"pg_profiler.min_duration_ms",
        c"Statements faster than this are not recorded.",
        c"Errors are always recorded.",
        &MIN_DURATION_MS,
        0.0,
        f64::from(i32::MAX),
        GucContext::Suset,
        GucFlags::UNIT_MS,
    );
    GucRegistry::define_int_guc(
        c"pg_profiler.buffer_size",
        c"Number of statement executions kept in the ring buffer.",
        c"Each slot costs roughly sizeof(record) + max_query_len bytes of shared memory.",
        &BUFFER_SIZE,
        100,
        1_000_000,
        GucContext::Postmaster,
        GucFlags::default(),
    );
    GucRegistry::define_int_guc(
        c"pg_profiler.max_query_len",
        c"Maximum number of bytes of query text stored per execution.",
        c"0 disables query text capture.",
        &MAX_QUERY_LEN,
        0,
        65_536,
        GucContext::Postmaster,
        GucFlags::UNIT_BYTE,
    );
}
