//! Hook installation. Each hook saves the previous one in a `static mut` and
//! always chains to it (or to the `standard_*` function).

use pgrx::pg_sys;

mod executor;

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
    if len > 0 && (len as usize) < rest.len() {
        &rest[..len as usize]
    } else {
        rest
    }
}

pub fn install() {
    unsafe {
        // Make sure the core computes query ids even without pg_stat_statements.
        pg_sys::EnableQueryId();
    }
    executor::install();
}
