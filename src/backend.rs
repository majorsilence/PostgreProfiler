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
    /// Address of the `QueryDesc` (0 for utility statements); only compared,
    /// never dereferenced.
    pub qd: usize,
    pub start_ts: i64,
    pub plan_us: f64,
    pub nesting: i32,
    pub subid: pg_sys::SubTransactionId,
    /// CPU time (user, system) of this process when the statement started.
    pub cpu_user_us: i64,
    pub cpu_sys_us: i64,
}

/// Cumulative (user, system) CPU time of this process in microseconds.
pub fn cpu_now() -> (i64, i64) {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } != 0 {
        return (0, 0);
    }
    #[allow(clippy::unnecessary_cast)] // field widths differ across platforms
    let us = |t: libc::timeval| t.tv_sec as i64 * 1_000_000 + t.tv_usec as i64;
    (us(ru.ru_utime), us(ru.ru_stime))
}

thread_local! {
    static NESTING: Cell<i32> = const { Cell::new(0) };
    static SAMPLED: Cell<bool> = const { Cell::new(true) };
    /// (address of the last PlannedStmt we timed, planning time in us)
    static PLAN_STASH: Cell<(usize, f64)> = const { Cell::new((0, 0.0)) };
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
    PLAN_STASH.with(|p| p.set((0, 0.0)));
}

/// Remember the planning time of the plan the planner just produced.
pub fn stash_plan(stmt: *mut pg_sys::PlannedStmt, plan_us: f64) {
    PLAN_STASH.with(|p| p.set((stmt as usize, plan_us)));
}

/// Planning time for `stmt`, if the planner we hooked produced it just now.
/// Plans from the plan cache (prepared statements) report 0.
pub fn take_plan_time(stmt: *mut pg_sys::PlannedStmt) -> f64 {
    PLAN_STASH.with(|p| {
        let (addr, us) = p.get();
        if addr == stmt as usize && addr != 0 {
            p.set((0, 0.0));
            us
        } else {
            0.0
        }
    })
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

/// True if a successful statement is faster than `pg_profiler.min_duration_ms`
/// and should be dropped. Errors are always recorded.
pub fn below_min_duration(exec_us: f64) -> bool {
    exec_us < gucs::MIN_DURATION_MS.get() * 1000.0
}

pub fn push_start(info: StartInfo) {
    STARTS.with(|s| s.borrow_mut().push(info));
}

/// Remove and return the start info for `qd`, if it was tracked.
pub fn take_start(qd: usize) -> Option<StartInfo> {
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
