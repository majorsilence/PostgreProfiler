//! Planner hook: times planning, and records statements that fail while being
//! planned (e.g. `SELECT 1/0`, which is constant-folded by the planner and so
//! never reaches the executor).

use super::statement_text;
use crate::backend;
use crate::gucs::{self, TrackLevel};
use crate::record::{cmd, TraceRecord};
use pgrx::pg_sys::panic::CaughtError;
use pgrx::prelude::*;
use std::ffi::{c_char, c_int};
use std::panic::AssertUnwindSafe;

use super::executor::error_sqlstate;

static mut PREV_PLANNER: pg_sys::planner_hook_type = None;

pub fn install() {
    unsafe {
        PREV_PLANNER = pg_sys::planner_hook;
        pg_sys::planner_hook = Some(planner);
    }
}

unsafe fn plan_chained(
    parse: *mut pg_sys::Query,
    query_string: *const c_char,
    cursor_options: c_int,
    bound_params: pg_sys::ParamListInfo,
) -> *mut pg_sys::PlannedStmt {
    unsafe {
        match PREV_PLANNER {
            Some(prev) => prev(parse, query_string, cursor_options, bound_params),
            None => pg_sys::standard_planner(parse, query_string, cursor_options, bound_params),
        }
    }
}

/// Planning is worth timing only if the statement could be recorded.
fn planning_active(level: i32) -> bool {
    if unsafe { pg_sys::ParallelWorkerNumber } >= 0 || !gucs::ENABLED.get() {
        return false;
    }
    match gucs::TRACK.get() {
        TrackLevel::None => false,
        TrackLevel::Top => level == 0,
        TrackLevel::All => true,
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn planner(
    parse: *mut pg_sys::Query,
    query_string: *const c_char,
    cursor_options: c_int,
    bound_params: pg_sys::ParamListInfo,
) -> *mut pg_sys::PlannedStmt {
    unsafe {
        let level = backend::nesting();
        if !planning_active(level) || query_string.is_null() {
            return plan_chained(parse, query_string, cursor_options, bound_params);
        }

        let t0 = pg_sys::GetCurrentTimestamp();
        let (cpu_user0, cpu_sys0) = backend::cpu_now();
        let ctx = backend::capture_ctx();
        // Statements run while planning (e.g. immutable functions evaluated
        // during constant folding) count as nested.
        backend::enter();
        let parse_addr = parse as usize;
        let qs_addr = query_string as usize;
        let result = PgTryBuilder::new(AssertUnwindSafe(|| {
            plan_chained(parse, query_string, cursor_options, bound_params)
        }))
        .catch_others(move |e| {
            record_plan_error(
                parse_addr as *mut pg_sys::Query,
                qs_addr as *const c_char,
                level,
                (t0, cpu_user0, cpu_sys0),
                &ctx,
                &e,
            );
            e.rethrow()
        })
        .finally(backend::leave)
        .execute();

        let plan_us = (pg_sys::GetCurrentTimestamp() - t0) as f64;
        backend::stash_plan(result, plan_us);
        result
    }
}

fn record_plan_error(
    parse: *mut pg_sys::Query,
    query_string: *const c_char,
    level: i32,
    (t0, cpu_user0, cpu_sys0): (i64, i64, i64),
    ctx: &backend::Ctx,
    e: &CaughtError,
) {
    unsafe {
        let q = &*parse;
        let mut rec: TraceRecord = backend::base_record(t0, level, ctx);
        rec.queryid = q.queryId as _;
        rec.cmd_type = match q.commandType {
            pg_sys::CmdType::CMD_SELECT => cmd::SELECT,
            pg_sys::CmdType::CMD_INSERT => cmd::INSERT,
            pg_sys::CmdType::CMD_UPDATE => cmd::UPDATE,
            pg_sys::CmdType::CMD_DELETE => cmd::DELETE,
            pg_sys::CmdType::CMD_MERGE => cmd::MERGE,
            pg_sys::CmdType::CMD_UTILITY => cmd::UTILITY,
            _ => 0,
        };
        rec.plan_time_us = (pg_sys::GetCurrentTimestamp() - t0) as f64;
        let (user, sys) = backend::cpu_now();
        rec.cpu_user_us = (user - cpu_user0).max(0);
        rec.cpu_sys_us = (sys - cpu_sys0).max(0);
        rec.is_error = true;
        rec.sqlstate = error_sqlstate(e);
        let text = statement_text(query_string, q.stmt_location, q.stmt_len);
        super::emit(&rec, text, &[]);
    }
}
