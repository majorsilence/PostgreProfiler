//! Hook installation. Each hook saves the previous one in a `static mut` and
//! always chains to it (or to the `standard_*` function).

use pgrx::prelude::*;

pub mod executor;
mod planner;
mod utility;

/// Slice of `source` that belongs to the statement at `loc`/`len`
/// (`PlannedStmt.stmt_location` / `stmt_len`). `-1`/`0` mean "the whole
/// string", so a multi-statement string records only its own statement.
///
/// # Safety
/// `source` must be null or a valid NUL-terminated string that outlives `'a`.
pub unsafe fn statement_text<'a>(source: *const std::ffi::c_char, loc: i32, len: i32) -> &'a [u8] {
    if source.is_null() {
        return &[];
    }
    let all = unsafe { std::ffi::CStr::from_ptr(source).to_bytes() };
    if loc < 0 || loc as usize > all.len() {
        return all;
    }
    let rest = &all[loc as usize..];
    let stmt = if len > 0 && (len as usize) < rest.len() { &rest[..len as usize] } else { rest };
    trim_statement(stmt)
}

/// Strip leading whitespace and trailing whitespace/semicolons, as
/// pg_stat_statements' `CleanQuerytext` does. (The range the parser reports
/// for a statement in a multi-statement string differs by version.)
fn trim_statement(mut s: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = s {
        if first.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = s {
        if last.is_ascii_whitespace() || *last == b';' {
            s = rest;
        } else {
            break;
        }
    }
    s
}

/// Push a statement record unless the capture rules reject it.
pub fn emit(rec: &crate::record::TraceRecord, text: &[u8], params: &[u8]) {
    if crate::rules::allows(rec, text) {
        crate::ring::push_statement(rec, text, params);
    }
}

/// Text of the bind parameters, or empty unless capture is switched on.
///
/// # Safety
/// `params` must be null or a valid `ParamListInfo`.
pub unsafe fn param_text(params: pg_sys::ParamListInfo) -> Vec<u8> {
    let max = crate::gucs::MAX_PARAM_LEN.get();
    if !crate::gucs::CAPTURE_PARAMS.get() || max <= 0 || params.is_null() {
        return Vec::new();
    }
    unsafe {
        if (*params).numParams <= 0 {
            return Vec::new();
        }
        let s = pg_sys::BuildParamLogString(params, std::ptr::null_mut(), max);
        if s.is_null() {
            Vec::new()
        } else {
            std::ffi::CStr::from_ptr(s).to_bytes().to_vec()
        }
    }
}

/// Publish the statement this backend just started running (top level only).
pub fn publish_running(
    queryid: i64,
    start_ts: i64,
    cmd_type: u8,
    text: &[u8],
    ctx: &crate::backend::Ctx,
) {
    use crate::gucs::{self, TrackLevel};
    if !gucs::ENABLED.get() || !gucs::TRACK_RUNNING.get() || gucs::TRACK.get() == TrackLevel::None {
        return;
    }
    let slot = unsafe {
        crate::running::Slot::new(
            pg_sys::MyProcPid,
            pg_sys::GetUserId().to_u32(),
            pg_sys::MyDatabaseId.to_u32(),
            queryid,
            start_ts,
            ctx.xact_start,
            cmd_type,
        )
    };
    crate::running::begin(slot, text);
}

pub fn install() {
    unsafe {
        // Make sure the core computes query ids even without pg_stat_statements.
        pg_sys::EnableQueryId();
    }
    crate::events::install();
    planner::install();
    executor::install();
    utility::install();
    unsafe {
        pg_sys::RegisterXactCallback(Some(xact_callback), std::ptr::null_mut());
        pg_sys::RegisterSubXactCallback(Some(subxact_callback), std::ptr::null_mut());
    }
}

/// After an abort, ExecutorEnd never runs for the statements that were in
/// flight; drop the bookkeeping so it cannot leak or be matched to a recycled
/// `QueryDesc` address.
#[pg_guard]
unsafe extern "C-unwind" fn xact_callback(
    event: pg_sys::XactEvent::Type,
    _arg: *mut std::ffi::c_void,
) {
    if event == pg_sys::XactEvent::XACT_EVENT_ABORT
        || event == pg_sys::XactEvent::XACT_EVENT_PARALLEL_ABORT
    {
        crate::backend::reset_all();
    }
    if event == pg_sys::XactEvent::XACT_EVENT_ABORT || event == pg_sys::XactEvent::XACT_EVENT_COMMIT {
        crate::running::end();
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn subxact_callback(
    event: pg_sys::SubXactEvent::Type,
    my_subid: pg_sys::SubTransactionId,
    _parent_subid: pg_sys::SubTransactionId,
    _arg: *mut std::ffi::c_void,
) {
    if event == pg_sys::SubXactEvent::SUBXACT_EVENT_ABORT_SUB {
        crate::backend::abort_subxact(my_subid);
    }
}
