//! Hook installation. Each hook saves the previous one in a `static mut` and
//! always chains to it (or to the `standard_*` function).

use pgrx::prelude::*;

mod executor;
mod planner;

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

pub fn install() {
    unsafe {
        // Make sure the core computes query ids even without pg_stat_statements.
        pg_sys::EnableQueryId();
    }
    planner::install();
    executor::install();
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
