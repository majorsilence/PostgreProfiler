//! Wait sampling: attributes what running statements wait for to their
//! statistics entries.

/// Wait categories, in the order of the counters kept per statement. The first
/// one means "not waiting" (running on the CPU or in a short uninstrumented
/// wait), the others are the classes of `pg_stat_activity.wait_event_type`.
pub const CATEGORY_NAMES: [&str; crate::stats::WAIT_CATS] = [
    "cpu",
    "lwlock",
    "lock",
    "bufferpin",
    "activity",
    "client",
    "extension",
    "ipc",
    "timeout",
    "io",
    "injectionpoint",
    "other",
];
