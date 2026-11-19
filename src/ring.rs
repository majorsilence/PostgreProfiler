//! Shared-memory ring buffer of [`TraceRecord`]s.
//!
//! Layout of the single `ShmemInitStruct("pg_profiler")` area:
//!
//! ```text
//! [ RingHeader (HEADER_SIZE bytes) ][ slot 0 ][ slot 1 ] ... [ slot capacity-1 ]
//! slot = TraceRecord + text_cap bytes of inline query text, padded to 8 bytes
//! ```
//!
//! The ring size depends on GUCs, so the raw `pg_sys` shmem API is used instead
//! of pgrx's fixed-size `PgLwLock<T>`.
//!
//! Locking: one named LWLock. `push` prepares everything before taking it and
//! only does a memcpy under `LW_EXCLUSIVE`; `snapshot` copies rows out under
//! `LW_SHARED`. Lock-free per-slot seqlocks would remove the writer/reader
//! contention entirely; that is a possible later optimisation.

use crate::gucs;
use crate::record::TraceRecord;
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::ptr;

const SHMEM_NAME: &std::ffi::CStr = c"pg_profiler";
const HEADER_SIZE: usize = 64;
const REC_SIZE: usize = std::mem::size_of::<TraceRecord>();

#[repr(C)]
struct RingHeader {
    capacity: u64,
    slot_size: u64,
    /// Next sequence number to write; slot index is `head % capacity`.
    head: u64,
    /// Rows with `seq < reset_at` are hidden (set by `reset`).
    reset_at: u64,
    total_written: u64,
    text_cap: u32,
}

const _: () = assert!(std::mem::size_of::<RingHeader>() <= HEADER_SIZE);
const _: () = assert!(std::mem::align_of::<TraceRecord>() <= 8);

static mut PREV_SHMEM_REQUEST: pg_sys::shmem_request_hook_type = None;
static mut PREV_SHMEM_STARTUP: pg_sys::shmem_startup_hook_type = None;

// Per-process pointers into shared memory; set in the startup hook and
// inherited across fork.
static mut RING: *mut RingHeader = ptr::null_mut();
static mut LOCK: *mut pg_sys::LWLock = ptr::null_mut();

fn slot_size(text_cap: usize) -> usize {
    (REC_SIZE + text_cap + 7) & !7
}

fn total_size() -> usize {
    let cap = gucs::BUFFER_SIZE.get() as usize;
    let text_cap = gucs::MAX_QUERY_LEN.get() as usize;
    HEADER_SIZE + cap * slot_size(text_cap)
}

/// Install the shmem request/startup hooks. Call from `_PG_init` while
/// `process_shared_preload_libraries_in_progress` is true.
pub fn install() {
    unsafe {
        PREV_SHMEM_REQUEST = pg_sys::shmem_request_hook;
        pg_sys::shmem_request_hook = Some(shmem_request);
        PREV_SHMEM_STARTUP = pg_sys::shmem_startup_hook;
        pg_sys::shmem_startup_hook = Some(shmem_startup);
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn shmem_request() {
    unsafe {
        if let Some(prev) = PREV_SHMEM_REQUEST {
            prev();
        }
        pg_sys::RequestAddinShmemSpace(total_size());
        pg_sys::RequestNamedLWLockTranche(SHMEM_NAME.as_ptr(), 1);
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn shmem_startup() {
    unsafe {
        if let Some(prev) = PREV_SHMEM_STARTUP {
            prev();
        }
        // AddinShmemInitLock is entry 21 of the main LWLock array in every
        // supported version (same approach as pgrx's own pg_shmem_init!).
        let init_lock = &raw mut (*pg_sys::MainLWLockArray.add(21)).lock;
        pg_sys::LWLockAcquire(init_lock, pg_sys::LWLockMode::LW_EXCLUSIVE);

        let mut found = false;
        let base = pg_sys::ShmemInitStruct(SHMEM_NAME.as_ptr(), total_size(), &mut found)
            as *mut RingHeader;
        if !found {
            // Only the header needs initialising: slots are never read before
            // being written (readers are bounded by `head`).
            ptr::write_bytes(base as *mut u8, 0, HEADER_SIZE);
            let text_cap = gucs::MAX_QUERY_LEN.get() as usize;
            (*base).capacity = gucs::BUFFER_SIZE.get() as u64;
            (*base).slot_size = slot_size(text_cap) as u64;
            (*base).text_cap = text_cap as u32;
        }
        RING = base;
        LOCK = &raw mut (*pg_sys::GetNamedLWLockTranche(SHMEM_NAME.as_ptr())).lock;

        pg_sys::LWLockRelease(init_lock);
    }
}

/// True when the library was preloaded and shared memory is attached.
pub fn is_ready() -> bool {
    unsafe { !RING.is_null() }
}

/// Raise a clear error unless the ring is available.
pub fn ensure_ready() {
    if !is_ready() {
        error!("pg_profiler must be loaded via shared_preload_libraries");
    }
}

struct Guard(*mut pg_sys::LWLock);

impl Guard {
    fn acquire(mode: pg_sys::LWLockMode::Type) -> Guard {
        unsafe {
            let lock = LOCK;
            pg_sys::LWLockAcquire(lock, mode);
            Guard(lock)
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe { pg_sys::LWLockRelease(self.0) }
    }
}

/// Longest prefix of `text` that is at most `max` bytes and ends on a UTF-8
/// character boundary.
pub fn truncate_utf8(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

unsafe fn slot_ptr(hdr: *mut RingHeader, seq: u64) -> *mut u8 {
    unsafe {
        let idx = seq % (*hdr).capacity;
        (hdr as *mut u8).add(HEADER_SIZE + (idx * (*hdr).slot_size) as usize)
    }
}

/// Append a record. `rec.seq` and `rec.query_len` are filled in here.
pub fn push(rec: &TraceRecord, text: &str) {
    unsafe {
        let hdr = RING;
        if hdr.is_null() {
            return;
        }
        let text = truncate_utf8(text, (*hdr).text_cap as usize);
        let mut rec = *rec;
        rec.query_len = text.len() as u32;

        let _guard = Guard::acquire(pg_sys::LWLockMode::LW_EXCLUSIVE);
        let seq = (*hdr).head;
        rec.seq = seq;
        let slot = slot_ptr(hdr, seq);
        ptr::copy_nonoverlapping(&rec as *const TraceRecord as *const u8, slot, REC_SIZE);
        ptr::copy_nonoverlapping(text.as_ptr(), slot.add(REC_SIZE), text.len());
        (*hdr).head = seq + 1;
        (*hdr).total_written += 1;
    }
}

/// Copy out up to `limit` of the newest visible rows, newest first.
pub fn snapshot(limit: usize) -> Vec<(TraceRecord, Vec<u8>)> {
    ensure_ready();
    unsafe {
        let hdr = RING;
        let mut out: Vec<(TraceRecord, Vec<u8>)> =
            Vec::with_capacity(limit.min((*hdr).capacity as usize));
        let _guard = Guard::acquire(pg_sys::LWLockMode::LW_SHARED);
        let head = (*hdr).head;
        let oldest = (*hdr).reset_at.max(head.saturating_sub((*hdr).capacity));
        let mut seq = head;
        while seq > oldest && out.len() < limit {
            seq -= 1;
            let slot = slot_ptr(hdr, seq);
            let rec = ptr::read_unaligned(slot as *const TraceRecord);
            let text = std::slice::from_raw_parts(slot.add(REC_SIZE), rec.query_len as usize);
            out.push((rec, text.to_vec()));
        }
        out
    }
}

/// Hide every row recorded so far.
pub fn reset() {
    ensure_ready();
    unsafe {
        let hdr = RING;
        let _guard = Guard::acquire(pg_sys::LWLockMode::LW_EXCLUSIVE);
        (*hdr).reset_at = (*hdr).head;
    }
}

pub struct Stats {
    pub capacity: i64,
    pub total_written: i64,
    pub retained: i64,
    pub overwritten: i64,
    pub max_query_len: i32,
}

pub fn stats() -> Stats {
    ensure_ready();
    unsafe {
        let hdr = RING;
        let _guard = Guard::acquire(pg_sys::LWLockMode::LW_SHARED);
        let head = (*hdr).head;
        let cap = (*hdr).capacity;
        let oldest = (*hdr).reset_at.max(head.saturating_sub(cap));
        Stats {
            capacity: cap as i64,
            total_written: (*hdr).total_written as i64,
            retained: (head - oldest) as i64,
            overwritten: head.saturating_sub(cap) as i64,
            max_query_len: (*hdr).text_cap as i32,
        }
    }
}

#[cfg(test)]
mod unit {
    use super::truncate_utf8;

    #[test]
    fn truncates_on_char_boundary() {
        assert_eq!(truncate_utf8("hello", 10), "hello");
        assert_eq!(truncate_utf8("hello", 3), "hel");
        // 'é' is two bytes; cutting in the middle backs up to the boundary.
        assert_eq!(truncate_utf8("aé", 2), "a");
        assert_eq!(truncate_utf8("aé", 3), "aé");
        assert_eq!(truncate_utf8("é", 0), "");
    }
}
