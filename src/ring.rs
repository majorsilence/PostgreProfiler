//! Shared memory: the statement ring, the event ring, the plan ring, the
//! running-statement slots, the capture rules and the table of plans already
//! seen, all carved out of one `ShmemInitStruct("pg_profiler")` area.
//!
//! ```text
//! [ statement ring ][ event ring ][ plan ring ][ running slots ][ rules block ][ seen plans ]
//! ring = RingHeader (64 bytes) + capacity slots
//! slot = record + payload_cap bytes of inline payload (query text, params, ...)
//! ```
//!
//! The sizes depend on GUCs, so the raw `pg_sys` shmem API is used instead of
//! pgrx's fixed-size `PgLwLock<T>`.
//!
//! Locking: one named LWLock tranche with four locks (statement ring, event
//! ring, rules, plan ring). `push` prepares everything before taking the lock and only
//! does a memcpy under `LW_EXCLUSIVE`; readers copy rows out under `LW_SHARED`.
//! Lock-free per-slot seqlocks would remove the writer/reader contention
//! entirely; that is a possible later optimisation. The running slots are
//! lock-free (single writer per slot, seqlock readers).

use crate::gucs;
use crate::record::{EventRecord, PlanRecord, TraceRecord};
use crate::{rules, running};
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::marker::PhantomData;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

const SHMEM_NAME: &std::ffi::CStr = c"pg_profiler";
pub const HEADER_SIZE: usize = 64;
const LOCK_STMT: usize = 0;
const LOCK_EVENTS: usize = 1;
const LOCK_RULES: usize = 2;
const LOCK_PLANS: usize = 3;
const NUM_LOCKS: i32 = 4;
/// Entries in the table of plans that were already stored (8 bytes each).
const SEEN_SLOTS: usize = 16_384;

/// A record that can live in a ring.
pub trait RingRecord: Copy {
    fn set_seq(&mut self, seq: u64);
    /// Bytes of payload stored after the record.
    fn payload_len(&self) -> usize;
}

#[repr(C)]
struct RingHeader {
    capacity: u64,
    slot_size: u64,
    /// Next sequence number to write; slot index is `head % capacity`.
    head: u64,
    /// Rows with `seq < reset_at` are hidden (set by `reset`).
    reset_at: u64,
    total_written: u64,
    payload_cap: u32,
}

const _: () = assert!(std::mem::size_of::<RingHeader>() <= HEADER_SIZE);

static mut PREV_SHMEM_REQUEST: pg_sys::shmem_request_hook_type = None;
static mut PREV_SHMEM_STARTUP: pg_sys::shmem_startup_hook_type = None;

/// Per-process pointers into shared memory; set in the startup hook and
/// inherited across fork.
#[derive(Clone, Copy)]
struct Shared {
    stmt: *mut RingHeader,
    events: *mut RingHeader,
    plans: *mut RingHeader,
    running: *mut u8,
    rules: *mut u8,
    seen: *mut AtomicU64,
    locks: *mut pg_sys::LWLockPadded,
}

static mut SHARED: Shared = Shared {
    stmt: ptr::null_mut(),
    events: ptr::null_mut(),
    plans: ptr::null_mut(),
    running: ptr::null_mut(),
    rules: ptr::null_mut(),
    seen: ptr::null_mut(),
    locks: ptr::null_mut(),
};

fn shared() -> Shared {
    unsafe { SHARED }
}

fn align8(n: usize) -> usize {
    (n + 7) & !7
}

/// Byte offsets and sizes of the regions, derived from the GUCs.
struct Layout {
    stmt_cap: usize,
    stmt_payload: usize,
    stmt_off: usize,
    ev_cap: usize,
    ev_payload: usize,
    ev_off: usize,
    plan_cap: usize,
    plan_payload: usize,
    plan_off: usize,
    run_off: usize,
    rules_off: usize,
    seen_off: usize,
    total: usize,
}

impl Layout {
    fn compute() -> Layout {
        let stmt_cap = gucs::BUFFER_SIZE.get() as usize;
        let stmt_payload = (gucs::MAX_QUERY_LEN.get() + gucs::MAX_PARAM_LEN.get()) as usize;
        let ev_cap = gucs::EVENTS_BUFFER_SIZE.get() as usize;
        let ev_payload = gucs::EVENT_DETAIL_LEN.get() as usize;

        let stmt_off = 0;
        let stmt_size = ring_size::<TraceRecord>(stmt_cap, stmt_payload);
        let ev_off = stmt_off + stmt_size;
        let ev_size = ring_size::<EventRecord>(ev_cap, ev_payload);
        let plan_cap = if gucs::MAX_PLAN_LEN.get() > 0 { gucs::PLAN_BUFFER_SIZE.get() as usize } else { 1 };
        let plan_payload = (gucs::MAX_PLAN_LEN.get() + gucs::MAX_QUERY_LEN.get()) as usize;
        let plan_off = ev_off + ev_size;
        let plan_size = ring_size::<PlanRecord>(plan_cap, plan_payload);
        let run_off = plan_off + plan_size;
        let run_size = running::region_size();
        let rules_off = run_off + run_size;
        let seen_off = align8(rules_off + rules::BLOCK_SIZE);
        let total = seen_off + SEEN_SLOTS * 8;
        Layout {
            stmt_cap,
            stmt_payload,
            stmt_off,
            ev_cap,
            ev_payload,
            ev_off,
            plan_cap,
            plan_payload,
            plan_off,
            run_off,
            rules_off,
            seen_off,
            total,
        }
    }
}

fn slot_stride<T>(payload_cap: usize) -> usize {
    align8(std::mem::size_of::<T>() + payload_cap)
}

fn ring_size<T>(capacity: usize, payload_cap: usize) -> usize {
    HEADER_SIZE + capacity * slot_stride::<T>(payload_cap)
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
        pg_sys::RequestAddinShmemSpace(Layout::compute().total);
        pg_sys::RequestNamedLWLockTranche(SHMEM_NAME.as_ptr(), NUM_LOCKS);
    }
}

unsafe fn init_ring<T>(base: *mut u8, capacity: usize, payload_cap: usize) -> *mut RingHeader {
    unsafe {
        let hdr = base as *mut RingHeader;
        ptr::write_bytes(base, 0, HEADER_SIZE);
        (*hdr).capacity = capacity as u64;
        (*hdr).slot_size = slot_stride::<T>(payload_cap) as u64;
        (*hdr).payload_cap = payload_cap as u32;
        hdr
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn shmem_startup() {
    unsafe {
        if let Some(prev) = PREV_SHMEM_STARTUP {
            prev();
        }
        let l = Layout::compute();
        // AddinShmemInitLock is entry 21 of the main LWLock array in every
        // supported version (same approach as pgrx's own pg_shmem_init!).
        let init_lock = &raw mut (*pg_sys::MainLWLockArray.add(21)).lock;
        pg_sys::LWLockAcquire(init_lock, pg_sys::LWLockMode::LW_EXCLUSIVE);

        let mut found = false;
        let base = pg_sys::ShmemInitStruct(SHMEM_NAME.as_ptr(), l.total, &mut found) as *mut u8;
        if !found {
            // Rings only need their headers initialised: slots are never read
            // before being written (readers are bounded by `head`).
            init_ring::<TraceRecord>(base.add(l.stmt_off), l.stmt_cap, l.stmt_payload);
            init_ring::<EventRecord>(base.add(l.ev_off), l.ev_cap, l.ev_payload);
            init_ring::<PlanRecord>(base.add(l.plan_off), l.plan_cap, l.plan_payload);
            ptr::write_bytes(base.add(l.seen_off), 0, SEEN_SLOTS * 8);
            running::init(base.add(l.run_off));
            rules::init(base.add(l.rules_off));
        }
        SHARED = Shared {
            stmt: base.add(l.stmt_off) as *mut RingHeader,
            events: base.add(l.ev_off) as *mut RingHeader,
            plans: base.add(l.plan_off) as *mut RingHeader,
            running: base.add(l.run_off),
            rules: base.add(l.rules_off),
            seen: base.add(l.seen_off) as *mut AtomicU64,
            locks: pg_sys::GetNamedLWLockTranche(SHMEM_NAME.as_ptr()),
        };

        pg_sys::LWLockRelease(init_lock);
    }
}

/// True when the library was preloaded and shared memory is attached.
pub fn is_ready() -> bool {
    !shared().stmt.is_null()
}

/// Raise a clear error unless shared memory is available.
pub fn ensure_ready() {
    if !is_ready() {
        error!("pg_profiler must be loaded via shared_preload_libraries");
    }
}

pub fn running_region() -> *mut u8 {
    shared().running
}

pub fn rules_region() -> *mut u8 {
    shared().rules
}

pub fn rules_lock() -> LockRef {
    LockRef(unsafe { &raw mut (*shared().locks.add(LOCK_RULES)).lock })
}

/// An LWLock that can be taken and released through a guard.
#[derive(Clone, Copy)]
pub struct LockRef(*mut pg_sys::LWLock);

impl LockRef {
    pub fn acquire(self, mode: pg_sys::LWLockMode::Type) -> Guard {
        unsafe { pg_sys::LWLockAcquire(self.0, mode) };
        Guard(self.0)
    }

    /// Never blocks; used where waiting is not acceptable (log hooks).
    pub fn try_acquire(self, mode: pg_sys::LWLockMode::Type) -> Option<Guard> {
        // `LWLockConditionalAcquire` only supports exclusive/shared modes.
        if unsafe { pg_sys::LWLockConditionalAcquire(self.0, mode) } {
            Some(Guard(self.0))
        } else {
            None
        }
    }
}

pub struct Guard(*mut pg_sys::LWLock);

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe { pg_sys::LWLockRelease(self.0) }
    }
}

/// Longest prefix of `text` that is at most `max` bytes and ends on a
/// character boundary of the server encoding (UTF-8 or otherwise).
pub fn clip_text(text: &[u8], max: usize) -> &[u8] {
    if text.len() <= max {
        return text;
    }
    let n = unsafe {
        pg_sys::pg_mbcliplen(
            text.as_ptr() as *const std::ffi::c_char,
            text.len() as i32,
            max as i32,
        )
    };
    &text[..n as usize]
}

/// One ring in shared memory. Cheap to copy; obtain with [`stmt_ring`] or
/// [`event_ring`].
#[derive(Clone, Copy)]
pub struct Ring<T> {
    hdr: *mut RingHeader,
    lock: LockRef,
    _record: PhantomData<T>,
}

pub fn stmt_ring() -> Ring<TraceRecord> {
    let s = shared();
    Ring {
        hdr: s.stmt,
        lock: LockRef(unsafe { &raw mut (*s.locks.add(LOCK_STMT)).lock }),
        _record: PhantomData,
    }
}

pub fn event_ring() -> Ring<EventRecord> {
    let s = shared();
    Ring {
        hdr: s.events,
        lock: LockRef(unsafe { &raw mut (*s.locks.add(LOCK_EVENTS)).lock }),
        _record: PhantomData,
    }
}

pub fn plan_ring() -> Ring<PlanRecord> {
    let s = shared();
    Ring {
        hdr: s.plans,
        lock: LockRef(unsafe { &raw mut (*s.locks.add(LOCK_PLANS)).lock }),
        _record: PhantomData,
    }
}

/// Remember that the plan with this key has been stored. Returns true if it
/// was not known before. The table is a small cache of recent keys: lookups
/// are lock-free, a full neighbourhood evicts one of its entries, so a plan
/// may occasionally be stored again, never missed.
pub fn plan_seen_insert(key: u64) -> bool {
    let table = shared().seen;
    if table.is_null() {
        return false;
    }
    let key = key.max(1);
    let start = (key as usize) % SEEN_SLOTS;
    unsafe {
        for i in 0..4 {
            let slot = &*table.add((start + i) % SEEN_SLOTS);
            let cur = slot.load(Ordering::Relaxed);
            if cur == key {
                return false;
            }
            if cur == 0 {
                match slot.compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => return true,
                    Err(other) if other == key => return false,
                    Err(_) => {}
                }
            }
        }
        (*table.add((start + (key >> 60) as usize % 4) % SEEN_SLOTS)).store(key, Ordering::Relaxed);
    }
    true
}

/// Forget every plan seen so far, so that plans are stored again.
pub fn plan_seen_clear() {
    let table = shared().seen;
    if table.is_null() {
        return;
    }
    for i in 0..SEEN_SLOTS {
        unsafe { (*table.add(i)).store(0, Ordering::Relaxed) };
    }
}

pub struct Stats {
    pub capacity: i64,
    pub total_written: i64,
    pub retained: i64,
    pub overwritten: i64,
    pub oldest_seq: i64,
    pub next_seq: i64,
}

impl<T: RingRecord> Ring<T> {
    pub fn is_ready(&self) -> bool {
        !self.hdr.is_null()
    }

    /// Bytes of payload each slot can hold.
    pub fn payload_cap(&self) -> usize {
        if self.hdr.is_null() {
            0
        } else {
            unsafe { (*self.hdr).payload_cap as usize }
        }
    }

    unsafe fn slot(&self, seq: u64) -> *mut u8 {
        unsafe {
            let hdr = self.hdr;
            let idx = seq % (*hdr).capacity;
            (hdr as *mut u8).add(HEADER_SIZE + (idx * (*hdr).slot_size) as usize)
        }
    }

    unsafe fn write(&self, rec: &T, parts: &[&[u8]]) {
        unsafe {
            let hdr = self.hdr;
            let mut rec = *rec;
            let seq = (*hdr).head;
            rec.set_seq(seq);
            let slot = self.slot(seq);
            ptr::copy_nonoverlapping(&rec as *const T as *const u8, slot, std::mem::size_of::<T>());
            let mut off = std::mem::size_of::<T>();
            for part in parts {
                ptr::copy_nonoverlapping(part.as_ptr(), slot.add(off), part.len());
                off += part.len();
            }
            (*hdr).head = seq + 1;
            (*hdr).total_written += 1;
        }
    }

    /// Append a record. The caller has already clipped `parts` so that their
    /// total length does not exceed [`Ring::payload_cap`] and matches what the
    /// record's `payload_len` says.
    pub fn push(&self, rec: &T, parts: &[&[u8]]) {
        if self.hdr.is_null() {
            return;
        }
        let _guard = self.lock.acquire(pg_sys::LWLockMode::LW_EXCLUSIVE);
        unsafe { self.write(rec, parts) }
    }

    /// Like [`Ring::push`] but gives up instead of waiting for the lock.
    /// Returns false if the record was dropped.
    pub fn try_push(&self, rec: &T, parts: &[&[u8]]) -> bool {
        if self.hdr.is_null() {
            return false;
        }
        match self.lock.try_acquire(pg_sys::LWLockMode::LW_EXCLUSIVE) {
            Some(_guard) => {
                unsafe { self.write(rec, parts) };
                true
            }
            None => false,
        }
    }

    unsafe fn read_at(&self, seq: u64) -> (T, Vec<u8>) {
        unsafe {
            let slot = self.slot(seq);
            let rec = ptr::read_unaligned(slot as *const T);
            let text = std::slice::from_raw_parts(
                slot.add(std::mem::size_of::<T>()),
                rec.payload_len().min((*self.hdr).payload_cap as usize),
            );
            (rec, text.to_vec())
        }
    }

    /// Copy out up to `limit` of the newest visible rows, newest first.
    pub fn newest(&self, limit: usize) -> Vec<(T, Vec<u8>)> {
        ensure_ready();
        unsafe {
            let hdr = self.hdr;
            let mut out = Vec::with_capacity(limit.min((*hdr).capacity as usize));
            let _guard = self.lock.acquire(pg_sys::LWLockMode::LW_SHARED);
            let head = (*hdr).head;
            let oldest = (*hdr).reset_at.max(head.saturating_sub((*hdr).capacity));
            let mut seq = head;
            while seq > oldest && out.len() < limit {
                seq -= 1;
                out.push(self.read_at(seq));
            }
            out
        }
    }

    /// Copy out up to `limit` rows with `seq > after`, oldest first. If the
    /// cursor has been lapped (rows were overwritten) the result starts at the
    /// oldest retained row, so the caller can detect the gap.
    pub fn since(&self, after: i64, limit: usize) -> Vec<(T, Vec<u8>)> {
        ensure_ready();
        unsafe {
            let hdr = self.hdr;
            let mut out = Vec::with_capacity(limit.min((*hdr).capacity as usize));
            let _guard = self.lock.acquire(pg_sys::LWLockMode::LW_SHARED);
            let head = (*hdr).head;
            let oldest = (*hdr).reset_at.max(head.saturating_sub((*hdr).capacity));
            let start = if after < 0 { oldest } else { (after as u64 + 1).max(oldest) };
            let mut seq = start;
            while seq < head && out.len() < limit {
                out.push(self.read_at(seq));
                seq += 1;
            }
            out
        }
    }

    /// Hide every row recorded so far.
    pub fn reset(&self) {
        ensure_ready();
        unsafe {
            let _guard = self.lock.acquire(pg_sys::LWLockMode::LW_EXCLUSIVE);
            (*self.hdr).reset_at = (*self.hdr).head;
        }
    }

    pub fn stats(&self) -> Stats {
        ensure_ready();
        unsafe {
            let hdr = self.hdr;
            let _guard = self.lock.acquire(pg_sys::LWLockMode::LW_SHARED);
            let head = (*hdr).head;
            let cap = (*hdr).capacity;
            let oldest = (*hdr).reset_at.max(head.saturating_sub(cap));
            Stats {
                capacity: cap as i64,
                total_written: (*hdr).total_written as i64,
                retained: (head - oldest) as i64,
                overwritten: head.saturating_sub(cap) as i64,
                oldest_seq: oldest as i64,
                next_seq: head as i64,
            }
        }
    }
}

/// Append a statement record; `query` and `params` are clipped to fit the slot.
pub fn push_statement(rec: &TraceRecord, query: &[u8], params: &[u8]) {
    let ring = stmt_ring();
    if !ring.is_ready() {
        return;
    }
    let l_query = gucs::MAX_QUERY_LEN.get() as usize;
    let query = clip_text(query, l_query);
    let params = clip_text(params, gucs::MAX_PARAM_LEN.get() as usize);
    let mut rec = *rec;
    rec.query_len = query.len() as u32;
    rec.params_len = params.len() as u32;
    ring.push(&rec, &[query, params]);
}
