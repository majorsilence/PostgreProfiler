//! `ProcessUtility` hook: records DDL and other utility commands.
//!
//! Like pg_stat_statements, `EXECUTE`, `PREPARE` and `DEALLOCATE` are skipped
//! (the executor already records what `EXECUTE` runs), and a tracked utility
//! command runs one nesting level deeper so that statements it executes
//! internally count as nested.

use super::executor::{error_sqlstate, fill_usage, set_cpu};
use super::statement_text;
use crate::backend::{self, StartInfo};
use crate::gucs;
use crate::record::{cmd, TraceRecord};
use crate::ring;
use pgrx::pg_sys::panic::CaughtError;
use pgrx::prelude::*;
use std::ffi::c_char;
use std::panic::AssertUnwindSafe;

static mut PREV_UTILITY: pg_sys::ProcessUtility_hook_type = None;

pub fn install() {
    unsafe {
        PREV_UTILITY = pg_sys::ProcessUtility_hook;
        pg_sys::ProcessUtility_hook = Some(process_utility);
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn chain(
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext::Type,
    params: pg_sys::ParamListInfo,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    qc: *mut pg_sys::QueryCompletion,
) {
    unsafe {
        match PREV_UTILITY {
            Some(prev) => {
                prev(pstmt, query_string, read_only_tree, context, params, query_env, dest, qc)
            }
            None => pg_sys::standard_ProcessUtility(
                pstmt,
                query_string,
                read_only_tree,
                context,
                params,
                query_env,
                dest,
                qc,
            ),
        }
    }
}

/// Everything we need after the call, copied out beforehand because the
/// `PlannedStmt` may be freed or modified by the utility command.
#[derive(Clone, Copy)]
struct Saved {
    query_id: i64,
    loc: i32,
    len: i32,
}

#[pg_guard]
#[allow(clippy::too_many_arguments)]
unsafe extern "C-unwind" fn process_utility(
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext::Type,
    params: pg_sys::ParamListInfo,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    qc: *mut pg_sys::QueryCompletion,
) {
    unsafe {
        let level = backend::nesting();
        if level == 0 && pg_sys::ParallelWorkerNumber < 0 {
            backend::begin_top_level();
        }
        let track = pg_sys::ParallelWorkerNumber < 0
            && gucs::TRACK_UTILITY.get()
            && !query_string.is_null()
            && !is_excluded((*pstmt).utilityStmt)
            && backend::is_tracked(level);
        if !track {
            return chain(
                pstmt,
                query_string,
                read_only_tree,
                context,
                params,
                query_env,
                dest,
                qc,
            );
        }

        let saved = Saved {
            query_id: (*pstmt).queryId as _,
            loc: (*pstmt).stmt_location,
            len: (*pstmt).stmt_len,
        };
        let start_ts = pg_sys::GetCurrentTimestamp();
        let (cpu_user_us, cpu_sys_us) = backend::cpu_now();
        let info = StartInfo {
            qd: 0,
            start_ts,
            plan_us: 0.0,
            nesting: level,
            subid: pg_sys::GetCurrentSubTransactionId(),
            cpu_user_us,
            cpu_sys_us,
        };
        let buf0 = pg_sys::pgBufferUsage;
        let wal0 = pg_sys::pgWalUsage;

        backend::enter();
        let qs_addr = query_string as usize;
        PgTryBuilder::new(AssertUnwindSafe(|| {
            chain(pstmt, query_string, read_only_tree, context, params, query_env, dest, qc)
        }))
        .catch_others(move |e| {
            record(&info, &saved, qs_addr as *const c_char, None, Some(&e), &buf0, &wal0);
            e.rethrow()
        })
        .finally(backend::leave)
        .execute();

        let rows = if qc.is_null() { 0 } else { (*qc).nprocessed };
        record(&info, &saved, query_string, Some(rows), None, &buf0, &wal0);
    }
}

/// EXECUTE/PREPARE/DEALLOCATE are not recorded as utility statements.
unsafe fn is_excluded(node: *mut pg_sys::Node) -> bool {
    if node.is_null() {
        return false;
    }
    matches!(
        unsafe { (*node).type_ },
        pg_sys::NodeTag::T_ExecuteStmt
            | pg_sys::NodeTag::T_PrepareStmt
            | pg_sys::NodeTag::T_DeallocateStmt
    )
}

fn record(
    info: &StartInfo,
    saved: &Saved,
    query_string: *const c_char,
    rows: Option<u64>,
    err: Option<&CaughtError>,
    buf0: &pg_sys::BufferUsage,
    wal0: &pg_sys::WalUsage,
) {
    unsafe {
        let mut rec: TraceRecord = backend::base_record(info.start_ts, info.nesting);
        rec.queryid = saved.query_id;
        rec.cmd_type = cmd::UTILITY;
        rec.exec_time_us = (pg_sys::GetCurrentTimestamp() - info.start_ts) as f64;
        rec.rows = rows.unwrap_or(0);
        set_cpu(&mut rec, info);

        let mut buf: pg_sys::BufferUsage = std::mem::zeroed();
        let mut wal: pg_sys::WalUsage = std::mem::zeroed();
        pg_sys::BufferUsageAccumDiff(&mut buf, &raw const pg_sys::pgBufferUsage, buf0);
        pg_sys::WalUsageAccumDiff(&mut wal, &raw const pg_sys::pgWalUsage, wal0);
        fill_usage(&mut rec, &buf, &wal);

        if let Some(e) = err {
            rec.is_error = true;
            rec.sqlstate = error_sqlstate(e);
        }
        let text = statement_text(query_string, saved.loc, saved.len);
        ring::push(&rec, text);
    }
}
