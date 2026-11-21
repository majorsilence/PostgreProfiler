//! Wait sampling: a background worker looks every few milliseconds at what the
//! backends that are running a statement are waiting for and counts it in the
//! backend's slot; the backend moves the counters into the statistics entry of
//! the statement when it finishes.
//!
//! Like `pg_stat_activity` sampling this is statistical: a statement shorter
//! than the sample period shows nothing, a long one shows how its time was
//! spent. Samples are attributed to the top-level statement.

use crate::gucs;
use crate::running;
use pgrx::bgworkers::{BackgroundWorker, BackgroundWorkerBuilder, SignalWakeFlags};
use pgrx::prelude::*;
use std::time::Duration;

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

/// Category of a `PGPROC::wait_event_info` value (the class is the top byte).
pub fn category(wait_event_info: u32) -> usize {
    match wait_event_info >> 24 {
        0x00 => 0,
        0x01 => 1,  // LWLock
        0x03 => 2,  // Lock
        0x04 => 3,  // BufferPin
        0x05 => 4,  // Activity
        0x06 => 5,  // Client
        0x07 => 6,  // Extension
        0x08 => 7,  // IPC
        0x09 => 8,  // Timeout
        0x0A => 9,  // IO
        0x0B => 10, // InjectionPoint
        _ => 11,
    }
}

pub fn register() {
    if gucs::WAIT_SAMPLE_MS.get() <= 0 {
        return;
    }
    BackgroundWorkerBuilder::new("pg_profiler wait sampler")
        .set_type("pg_profiler wait sampler")
        .set_library("pg_profiler")
        .set_function("pg_profiler_sampler_main")
        .enable_shmem_access(None)
        .set_restart_time(Some(Duration::from_secs(10)))
        .load();
}

#[no_mangle]
#[pg_guard]
pub extern "C-unwind" fn pg_profiler_sampler_main(_arg: pg_sys::Datum) {
    BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGHUP | SignalWakeFlags::SIGTERM);
    let period = Duration::from_millis(gucs::WAIT_SAMPLE_MS.get().max(1) as u64);
    while BackgroundWorker::wait_latch(Some(period)) {
        sample_once();
    }
}

fn sample_once() {
    unsafe {
        let hdr = pg_sys::ProcGlobal;
        if hdr.is_null() || (*hdr).allProcs.is_null() {
            return;
        }
        let procs = (*hdr).allProcs;
        let limit = running::slot_count().min((*hdr).allProcCount as usize);
        for i in 0..limit {
            if running::is_active(i) {
                let info = std::ptr::read_volatile(&raw const (*procs.add(i)).wait_event_info);
                running::add_wait(i, category(info));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_map_to_categories() {
        assert_eq!(category(0), 0);
        assert_eq!(category(pg_sys::PG_WAIT_LWLOCK | 5), 1);
        assert_eq!(category(pg_sys::PG_WAIT_LOCK), 2);
        assert_eq!(category(pg_sys::PG_WAIT_IO | 3), 9);
        assert_eq!(category(0xFF00_0000), 11);
    }
}
