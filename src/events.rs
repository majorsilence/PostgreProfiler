//! Non-statement events: logins and logouts, authentication failures,
//! deadlocks, lock waits, cancellations and terminations.
//!
//! Server-side events are picked out of the log stream with `emit_log_hook`
//! (the hook sees every message that is written to the server log, with its
//! SQLSTATE). Lock-wait messages only exist when `log_lock_waits = on`.
//! Sessions are announced lazily, the first time one of our statement hooks
//! runs in a backend, because Postgres has no per-backend start hook that
//! knows the user and database.

use crate::backend;
use crate::gucs;
use crate::hooks::executor::sqlstate_bytes;
use crate::record::{copy_into, event, EventRecord};
use crate::ring;
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::cell::Cell;
use std::ffi::{c_int, CStr};

const LOG: c_int = 15;
const FATAL: c_int = 22;

static mut PREV_EMIT_LOG: pg_sys::emit_log_hook_type = None;

thread_local! {
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
    static SESSION_STARTED: Cell<bool> = const { Cell::new(false) };
}

pub fn install() {
    unsafe {
        PREV_EMIT_LOG = pg_sys::emit_log_hook;
        pg_sys::emit_log_hook = Some(emit_log);
    }
}

fn enabled() -> bool {
    gucs::ENABLED.get() && gucs::TRACK_EVENTS.get()
}

/// SQLSTATE as the packed integer Postgres uses.
fn code(s: &[u8; 5]) -> c_int {
    s.iter().enumerate().fold(0, |acc, (i, &c)| acc | ((c_int::from(c) - 0x30) & 0x3F) << (6 * i))
}

/// Build and push an event. With `try_only` the ring lock is never waited
/// for, which is required inside the log hook.
fn record(kind: u8, ts: i64, sqlstate: c_int, duration_us: i64, detail: &[u8], try_only: bool) {
    let ring = ring::event_ring();
    if !ring.is_ready() {
        return;
    }
    let mut rec = EventRecord { kind, ts, duration_us, ..EventRecord::default() };
    unsafe {
        rec.userid = pg_sys::GetUserId().to_u32();
        rec.dbid = pg_sys::MyDatabaseId.to_u32();
        rec.pid = pg_sys::MyProcPid;
        let name = pg_sys::application_name;
        if !name.is_null() {
            copy_into(&mut rec.application_name, CStr::from_ptr(name).to_bytes());
        }
    }
    rec.client_addr = backend::client_addr();
    if sqlstate != 0 {
        rec.sqlstate = sqlstate_bytes(sqlstate);
    }
    let detail = ring::clip_text(detail, ring.payload_cap());
    rec.detail_len = detail.len() as u32;
    if try_only {
        ring.try_push(&rec, &[detail]);
    } else {
        ring.push(&rec, &[detail]);
    }
}

/// Milliseconds from a "... after 1000.123 ms" lock-wait message, as microseconds.
fn wait_micros(msg: &[u8]) -> i64 {
    let text = String::from_utf8_lossy(msg);
    let Some(idx) = text.rfind(" after ") else { return 0 };
    let num: String = text[idx + 7..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    num.parse::<f64>().map_or(0, |ms| (ms * 1000.0) as i64)
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

unsafe fn cstr_bytes<'a>(p: *const std::ffi::c_char) -> &'a [u8] {
    if p.is_null() {
        &[]
    } else {
        unsafe { CStr::from_ptr(p).to_bytes() }
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn emit_log(edata: *mut pg_sys::ErrorData) {
    unsafe {
        if let Some(prev) = PREV_EMIT_LOG {
            prev(edata);
        }
        if edata.is_null() || !enabled() || IN_HOOK.with(Cell::get) {
            return;
        }
        let e = &*edata;
        let msg = cstr_bytes(e.message);
        let classified = classify(e, msg);
        let Some((kind, duration_us)) = classified else { return };

        IN_HOOK.with(|h| h.set(true));
        // message, then the log detail (which names queries and lock holders)
        // or else the client detail
        let detail_src = if e.detail_log.is_null() { e.detail } else { e.detail_log };
        let mut detail = msg.to_vec();
        let extra = cstr_bytes(detail_src);
        if !extra.is_empty() {
            detail.push(b'\n');
            detail.extend_from_slice(extra);
        }
        record(kind, pg_sys::GetCurrentTimestamp(), e.sqlerrcode, duration_us, &detail, true);
        IN_HOOK.with(|h| h.set(false));
    }
}

/// Which event, if any, a log message is.
fn classify(e: &pg_sys::ErrorData, msg: &[u8]) -> Option<(u8, i64)> {
    let level = e.elevel;
    let sqlstate = e.sqlerrcode;
    if sqlstate == code(b"40P01") {
        return Some((event::DEADLOCK, 0));
    }
    if sqlstate == code(b"55P03") {
        return Some((event::LOCK_TIMEOUT, 0));
    }
    if sqlstate == code(b"57014") {
        return Some((event::CANCEL, 0));
    }
    if sqlstate == code(b"57P01") {
        return Some((event::TERMINATED, 0));
    }
    // class 28: invalid authorization specification / invalid password
    if level == FATAL && (sqlstate & 0xFFF) == (code(b"28000") & 0xFFF) {
        return Some((event::AUTH_FAILURE, 0));
    }
    // log_lock_waits messages (LOG level, written by ProcSleep)
    if level == LOG && msg.starts_with(b"process ") && contains(msg, " after ") {
        if contains(msg, "still waiting for") {
            return Some((event::LOCK_WAIT, wait_micros(msg)));
        }
        if contains(msg, " acquired ") {
            return Some((event::LOCK_ACQUIRED, wait_micros(msg)));
        }
    }
    None
}

/// Announce the session (login event, logout callback) the first time a
/// statement hook runs in this backend.
pub fn ensure_session() {
    if SESSION_STARTED.with(Cell::get) {
        return;
    }
    SESSION_STARTED.with(|s| s.set(true));
    unsafe {
        if pg_sys::MyBackendType != pg_sys::BackendType::B_BACKEND || pg_sys::ParallelWorkerNumber >= 0
        {
            return;
        }
        if !ring::is_ready() {
            return;
        }
        pg_sys::before_shmem_exit(Some(logout), pg_sys::Datum::from(0usize));
        if enabled() {
            record(event::LOGIN, pg_sys::MyStartTimestamp, 0, 0, b"", false);
        }
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn logout(_code: c_int, _arg: pg_sys::Datum) {
    unsafe {
        if !enabled() {
            return;
        }
        let now = pg_sys::GetCurrentTimestamp();
        record(event::LOGOUT, now, 0, now - pg_sys::MyStartTimestamp, b"", true);
    }
}
