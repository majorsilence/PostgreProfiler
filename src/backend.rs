//! Per-backend state. Backends are single-threaded, so plain `thread_local!`
//! cells are enough; nothing here is shared with other backends.

use crate::gucs::{self, TrackLevel};
use crate::record::TraceRecord;
use pgrx::pg_sys;
use std::cell::{Cell, RefCell};

/// What we remember about a tracked statement between `ExecutorStart` and
/// `ExecutorEnd` (or an error).
#[derive(Clone, Copy)]
pub struct StartInfo {
    /// Identity of the `QueryDesc`; only compared, never dereferenced.
    pub qd: *mut pg_sys::QueryDesc,
    pub start_ts: i64,
    pub plan_us: f64,
    pub nesting: i32,
    pub subid: pg_sys::SubTransactionId,
}

thread_local! {
    static NESTING: Cell<i32> = const { Cell::new(0) };
    static SAMPLED: Cell<bool> = const { Cell::new(true) };
    static STARTS: RefCell<Vec<StartInfo>> = const { RefCell::new(Vec::new()) };
}

pub fn nesting() -> i32 {
    NESTING.with(Cell::get)
}

pub fn enter() {
    NESTING.with(|n| n.set(n.get() + 1));
}

pub fn leave() {
    NESTING.with(|n| n.set((n.get() - 1).max(0)));
}

/// Called at transaction abort to drop any state an `ERROR` left behind.
pub fn reset_all() {
    NESTING.with(|n| n.set(0));
    STARTS.with(|s| s.borrow_mut().clear());
}

/// A subtransaction aborted: forget statements that started inside it.
pub fn abort_subxact(my_subid: pg_sys::SubTransactionId) {
    STARTS.with(|s| s.borrow_mut().retain(|i| i.subid < my_subid));
}

/// Decide whether the top-level statement that is starting is sampled.
/// Nested statements keep the decision, so a sampled statement's children are
/// kept together.
pub fn begin_top_level() {
    let rate = gucs::SAMPLE_RATE.get();
    let sampled = if rate >= 1.0 {
        true
    } else if rate <= 0.0 {
        false
    } else {
        unsafe { pg_sys::pg_prng_double(&raw mut pg_sys::pg_global_prng_state) < rate }
    };
    SAMPLED.with(|s| s.set(sampled));
}

/// Should a statement at this nesting level be recorded?
pub fn is_tracked(level: i32) -> bool {
    if !gucs::ENABLED.get() || !SAMPLED.with(Cell::get) {
        return false;
    }
    match gucs::TRACK.get() {
        TrackLevel::None => false,
        TrackLevel::Top => level == 0,
        TrackLevel::All => true,
    }
}

pub fn push_start(info: StartInfo) {
    STARTS.with(|s| s.borrow_mut().push(info));
}

/// Remove and return the start info for `qd`, if it was tracked.
pub fn take_start(qd: *mut pg_sys::QueryDesc) -> Option<StartInfo> {
    STARTS.with(|s| {
        let mut v = s.borrow_mut();
        let pos = v.iter().rposition(|i| i.qd == qd)?;
        Some(v.remove(pos))
    })
}

/// A record pre-filled with the identity fields common to every statement.
pub fn base_record(start_ts: i64, nesting: i32) -> TraceRecord {
    unsafe {
        TraceRecord {
            start_ts,
            userid: pg_sys::GetUserId().to_u32(),
            dbid: pg_sys::MyDatabaseId.to_u32(),
            pid: pg_sys::MyProcPid,
            nesting,
            toplevel: nesting == 0,
            ..TraceRecord::default()
        }
    }
}
