//! Executor hooks: `ExecutorStart` allocates the per-query instrumentation and
//! remembers when the statement began; `ExecutorEnd` turns the totals into a
//! [`TraceRecord`] and pushes it into the ring buffer.

use super::statement_text;
use crate::backend::{self, StartInfo};
use crate::record::{cmd, TraceRecord};
use crate::ring;
use pgrx::prelude::*;
use pgrx::{pg_sys, PgMemoryContexts};
use std::ffi::c_int;

static mut PREV_START: pg_sys::ExecutorStart_hook_type = None;
static mut PREV_END: pg_sys::ExecutorEnd_hook_type = None;

pub fn install() {
    unsafe {
        PREV_START = pg_sys::ExecutorStart_hook;
        pg_sys::ExecutorStart_hook = Some(executor_start);
        PREV_END = pg_sys::ExecutorEnd_hook;
        pg_sys::ExecutorEnd_hook = Some(executor_end);
    }
}

fn in_parallel_worker() -> bool {
    unsafe { pg_sys::ParallelWorkerNumber >= 0 }
}

fn cmd_kind(op: pg_sys::CmdType::Type) -> u8 {
    match op {
        pg_sys::CmdType::CMD_SELECT => cmd::SELECT,
        pg_sys::CmdType::CMD_INSERT => cmd::INSERT,
        pg_sys::CmdType::CMD_UPDATE => cmd::UPDATE,
        pg_sys::CmdType::CMD_DELETE => cmd::DELETE,
        pg_sys::CmdType::CMD_MERGE => cmd::MERGE,
        pg_sys::CmdType::CMD_UTILITY => cmd::UTILITY,
        _ => 0,
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn executor_start(qd: *mut pg_sys::QueryDesc, eflags: c_int) {
    unsafe {
        match PREV_START {
            Some(prev) => prev(qd, eflags),
            None => pg_sys::standard_ExecutorStart(qd, eflags),
        }

        if in_parallel_worker() || (eflags & pg_sys::EXEC_FLAG_EXPLAIN_ONLY as c_int) != 0 {
            return;
        }
        let level = backend::nesting();
        if level == 0 {
            backend::begin_top_level();
        }
        if !backend::is_tracked(level) {
            return;
        }

        // Allocate in the per-query context (after the chain, so that the
        // executor state exists) so the instrumentation goes away at
        // ExecutorEnd. This is the pg_stat_statements pattern.
        if (*qd).totaltime.is_null() {
            let cxt = (*(*qd).estate).es_query_cxt;
            PgMemoryContexts::For(cxt).switch_to(|_| {
                (*qd).totaltime =
                    pg_sys::InstrAlloc(1, pg_sys::InstrumentOption::INSTRUMENT_ALL as c_int, false);
            });
        }

        backend::push_start(StartInfo {
            qd,
            start_ts: pg_sys::GetCurrentTimestamp(),
            plan_us: 0.0,
            nesting: level,
            subid: pg_sys::GetCurrentSubTransactionId(),
        });
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn executor_end(qd: *mut pg_sys::QueryDesc) {
    unsafe {
        if let Some(info) = backend::take_start(qd) {
            if !(*qd).totaltime.is_null() {
                record_finished(qd, &info);
            }
        }
        match PREV_END {
            Some(prev) => prev(qd),
            None => pg_sys::standard_ExecutorEnd(qd),
        }
    }
}

/// Build and push the record for a statement that completed normally.
unsafe fn record_finished(qd: *mut pg_sys::QueryDesc, info: &StartInfo) {
    unsafe {
        let tt = (*qd).totaltime;
        pg_sys::InstrEndLoop(tt);
        let tt = &*tt;
        let plan = &*(*qd).plannedstmt;

        let mut rec: TraceRecord = backend::base_record(info.start_ts, info.nesting);
        rec.queryid = plan.queryId as _;
        rec.cmd_type = cmd_kind((*qd).operation);
        rec.exec_time_us = tt.total * 1_000_000.0;
        rec.plan_time_us = info.plan_us;
        rec.rows = (*(*qd).estate).es_total_processed;
        fill_usage(&mut rec, &tt.bufusage, &tt.walusage);

        let text = statement_text((*qd).sourceText, plan.stmt_location, plan.stmt_len);
        ring::push(&rec, text);
    }
}

pub fn fill_usage(rec: &mut TraceRecord, buf: &pg_sys::BufferUsage, wal: &pg_sys::WalUsage) {
    rec.shared_blks_hit = buf.shared_blks_hit;
    rec.shared_blks_read = buf.shared_blks_read;
    rec.shared_blks_dirtied = buf.shared_blks_dirtied;
    rec.shared_blks_written = buf.shared_blks_written;
    rec.local_blks_hit = buf.local_blks_hit;
    rec.local_blks_read = buf.local_blks_read;
    rec.temp_blks_read = buf.temp_blks_read;
    rec.temp_blks_written = buf.temp_blks_written;
    rec.wal_records = wal.wal_records;
    rec.wal_fpi = wal.wal_fpi;
    rec.wal_bytes = wal.wal_bytes;
}
