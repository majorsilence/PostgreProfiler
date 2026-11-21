//! Query Store style aggregates: per interval and per (database, role, query,
//! plan) counters, sums of squares for the standard deviation, a log-scale
//! latency histogram and wait-sample counters, plus a small cache of query
//! texts.
//!
//! Layout inside the stats region (all sizes derive from GUCs, fixed at start):
//!
//! ```text
//! [ header ][ generation 0 ][ generation 1 ] ... [ generation GENS-1 ][ text cache ]
//! generation = GenHeader + `entries` x Entry   (one time interval each)
//! ```
//!
//! A generation holds the statistics of one interval. When the clock moves to a
//! new interval the first backend to notice *rotates*: the generation that
//! comes next in the ring of `GENS` is cleared and takes the new interval, so
//! the last `GENS - 1` closed intervals stay readable. The archiver copies closed
//! generations into tables and marks them archived.
//!
//! Concurrency: finding or creating an entry is lock-free (open addressing
//! over a table that only grows until its generation is cleared); counters are
//! updated under a per-entry spin flag held for a few dozen instructions.
//! Rotation takes an LWLock exclusively, readers (archiver, SQL functions) take
//! it shared, so a generation is never cleared while it is being read.

use crate::gucs::{self, StatsCapture};
use crate::record::TraceRecord;
use crate::ring;
use pgrx::pg_sys;
use std::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};

pub const GENS: usize = 4;
/// Histogram buckets: two per octave, bucket 0 is below ~1.4 microseconds and
/// the last one holds everything above about 15 minutes.
pub const HIST_BUCKETS: usize = 60;
/// Wait categories sampled per statement; see `sampler.rs`.
pub const WAIT_CATS: usize = 12;
const PROBE: usize = 64;
const TEXT_PROBE: usize = 8;
const HEADER_SIZE: usize = 128;
const GEN_HEADER: usize = 64;

#[repr(C)]
struct StatsHeader {
    interval_us: i64,
    entries: u32,
    text_cap: u32,
    text_len: u32,
    _pad: u32,
    /// Interval number the backends are writing to (0 = nothing yet).
    current: AtomicU64,
    rotations: AtomicU64,
    /// Closed intervals overwritten before the archiver stored them.
    lost_intervals: AtomicU64,
    /// Statements that found their generation full.
    dropped_entries: AtomicU64,
    texts_dropped: AtomicU64,
}

const _: () = assert!(std::mem::size_of::<StatsHeader>() <= HEADER_SIZE);

#[repr(C)]
struct GenHeader {
    /// Interval number (`timestamp / interval`), 0 = unused.
    interval_no: AtomicU64,
    archived: AtomicU32,
    used: AtomicU32,
}

const _: () = assert!(std::mem::size_of::<GenHeader>() <= GEN_HEADER);

/// The counters of one entry.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct Data {
    pub calls: u64,
    pub errors: u64,
    pub exec_sum_us: f64,
    pub exec_sumsq_us: f64,
    pub exec_min_us: f64,
    pub exec_max_us: f64,
    pub plan_sum_us: f64,
    pub rows: u64,
    pub shared_blks_hit: u64,
    pub shared_blks_read: u64,
    pub shared_blks_dirtied: u64,
    pub shared_blks_written: u64,
    pub temp_blks_read: u64,
    pub temp_blks_written: u64,
    pub wal_records: u64,
    pub wal_fpi: u64,
    pub wal_bytes: u64,
    pub cpu_user_us: u64,
    pub cpu_sys_us: u64,
    pub blk_read_us: u64,
    pub blk_write_us: u64,
    pub hist: [u32; HIST_BUCKETS],
    pub waits: [u32; WAIT_CATS],
}

impl Data {
    fn empty() -> Data {
        // SAFETY: integers and floats; all-zero is valid.
        let mut d: Data = unsafe { std::mem::zeroed() };
        d.exec_min_us = f64::MAX;
        d
    }
}

#[repr(C)]
struct Entry {
    /// 0 = free, 1 = being created, 2 = ready.
    state: AtomicU32,
    lock: AtomicU32,
    dbid: u32,
    userid: u32,
    queryid: i64,
    plan_id: i64,
    data: Data,
}

const ENTRY_SIZE: usize = std::mem::size_of::<Entry>();

#[repr(C)]
struct TextHeader {
    /// Seqlock: odd while being written.
    ver: AtomicU32,
    len: u32,
    dbid: u32,
    _pad: u32,
    queryid: i64,
    last_used: AtomicU64,
}

const TEXT_HEADER: usize = std::mem::size_of::<TextHeader>();

// ---------------------------------------------------------------------------
// region geometry

fn align8(n: usize) -> usize {
    (n + 7) & !7
}

fn gen_size(entries: usize) -> usize {
    GEN_HEADER + entries * ENTRY_SIZE
}

fn text_stride(text_len: usize) -> usize {
    align8(TEXT_HEADER + text_len)
}

/// Bytes the stats region needs.
pub fn region_size() -> usize {
    let entries = gucs::STATS_ENTRIES.get() as usize;
    let text_len = gucs::STATS_TEXT_LEN.get() as usize;
    HEADER_SIZE + GENS * gen_size(entries) + entries * text_stride(text_len)
}

/// Initialise a fresh region.
///
/// # Safety
/// `base` must point to `region_size()` writable bytes.
pub unsafe fn init(base: *mut u8) {
    unsafe {
        std::ptr::write_bytes(base, 0, region_size());
        let h = base as *mut StatsHeader;
        (*h).interval_us = i64::from(gucs::STATS_INTERVAL.get()) * 1_000_000;
        (*h).entries = gucs::STATS_ENTRIES.get() as u32;
        (*h).text_cap = gucs::STATS_ENTRIES.get() as u32;
        (*h).text_len = gucs::STATS_TEXT_LEN.get() as u32;
    }
}

fn header() -> Option<&'static StatsHeader> {
    let base = ring::stats_region();
    if base.is_null() {
        None
    } else {
        Some(unsafe { &*(base as *const StatsHeader) })
    }
}

unsafe fn gen_ptr(h: &StatsHeader, g: usize) -> *mut GenHeader {
    unsafe { ring::stats_region().add(HEADER_SIZE + g * gen_size(h.entries as usize)) as *mut GenHeader }
}

unsafe fn entry_ptr(gen: *mut GenHeader, i: usize) -> *mut Entry {
    unsafe { (gen as *mut u8).add(GEN_HEADER + i * ENTRY_SIZE) as *mut Entry }
}

unsafe fn text_ptr(h: &StatsHeader, i: usize) -> *mut TextHeader {
    unsafe {
        ring::stats_region()
            .add(HEADER_SIZE + GENS * gen_size(h.entries as usize) + i * text_stride(h.text_len as usize))
            as *mut TextHeader
    }
}

// ---------------------------------------------------------------------------
// helpers

fn mix(dbid: u32, queryid: i64, plan_id: i64, userid: u32) -> u64 {
    let mut x = u64::from(dbid) << 32 | u64::from(userid);
    for v in [queryid as u64, plan_id as u64] {
        x ^= v.wrapping_add(0x9e37_79b9_7f4a_7c15).wrapping_add(x << 6).wrapping_add(x >> 2);
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 33;
    }
    x
}

fn spin_lock(flag: &AtomicU32) {
    let mut spins = 0u32;
    while flag.compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
        spins += 1;
        if spins > 1000 {
            unsafe { pg_sys::pg_usleep(10) };
            spins = 0;
        } else {
            std::hint::spin_loop();
        }
    }
}

fn spin_unlock(flag: &AtomicU32) {
    flag.store(0, Ordering::Release);
}

/// Histogram bucket of an execution time.
pub fn bucket(us: f64) -> usize {
    if us < 1.0 {
        0
    } else {
        ((us.log2() * 2.0) as usize).min(HIST_BUCKETS - 1)
    }
}

/// Lower edge of a bucket in microseconds.
pub fn bucket_lower_us(i: usize) -> f64 {
    2f64.powf(i as f64 / 2.0)
}

fn stats_lock() -> ring::LockRef {
    ring::stats_lock()
}

// ---------------------------------------------------------------------------
// recording

/// Find the entry for a key in a generation, creating it if `create` is set.
/// Returns the entry and whether it was created.
unsafe fn find_or_insert(
    h: &StatsHeader,
    gen: *mut GenHeader,
    dbid: u32,
    userid: u32,
    queryid: i64,
    plan_id: i64,
    create: bool,
) -> Option<(*mut Entry, bool)> {
    unsafe {
        let cap = h.entries as usize;
        let start = (mix(dbid, queryid, plan_id, userid) as usize) % cap;
        let limit = PROBE.min(cap);
        for i in 0..limit {
            let e = entry_ptr(gen, (start + i) % cap);
            loop {
                match (*e).state.load(Ordering::Acquire) {
                    2 => {
                        if (*e).dbid == dbid
                            && (*e).userid == userid
                            && (*e).queryid == queryid
                            && (*e).plan_id == plan_id
                        {
                            return Some((e, false));
                        }
                        break;
                    }
                    0 => {
                        if !create {
                            return None;
                        }
                        if (*e).state.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                            (*e).dbid = dbid;
                            (*e).userid = userid;
                            (*e).queryid = queryid;
                            (*e).plan_id = plan_id;
                            (*e).data = Data::empty();
                            (*gen).used.fetch_add(1, Ordering::Relaxed);
                            (*e).state.store(2, Ordering::Release);
                            return Some((e, true));
                        }
                        // lost the race for this slot: look at it again
                    }
                    _ => std::hint::spin_loop(),
                }
            }
        }
        None
    }
}

fn enabled() -> bool {
    gucs::ENABLED.get() && gucs::STATS_CAPTURE.get() != StatsCapture::None
}

/// Add one finished statement to the current interval's statistics.
pub fn record(rec: &TraceRecord, query: &[u8], waits: &[u32; WAIT_CATS]) {
    if !enabled() {
        return;
    }
    let Some(h) = header() else { return };
    let end_ts = rec.start_ts + rec.exec_time_us as i64;
    let mut n = (end_ts.max(0) / h.interval_us.max(1)) as u64;
    let cur = h.current.load(Ordering::Acquire);
    if n > cur {
        rotate(h, n);
    }
    n = n.max(cur).max(h.current.load(Ordering::Acquire));

    unsafe {
        let gen = gen_ptr(h, (n % GENS as u64) as usize);
        if (*gen).interval_no.load(Ordering::Acquire) != n {
            // another backend rotated further in the meantime; drop this one
            return;
        }
        let create = match gucs::STATS_CAPTURE.get() {
            StatsCapture::All => true,
            StatsCapture::Auto => rec.is_error || rec.exec_time_us >= gucs::STATS_AUTO_MIN_MS.get() * 1000.0,
            StatsCapture::None => false,
        };
        // keep the table below ~90 % so that probe chains stay short
        let used = (*gen).used.load(Ordering::Relaxed) as usize;
        let create = create && used * 10 < h.entries as usize * 9;
        let Some((e, created)) = find_or_insert(h, gen, rec.dbid, rec.userid, rec.queryid, rec.plan_id, create)
        else {
            if create {
                h.dropped_entries.fetch_add(1, Ordering::Relaxed);
            }
            return;
        };
        if created {
            text_put(h, rec.dbid, rec.queryid, query, n);
        }
        update(e, rec, waits);
    }
}

unsafe fn update(e: *mut Entry, rec: &TraceRecord, waits: &[u32; WAIT_CATS]) {
    unsafe {
        spin_lock(&(*e).lock);
        let d = &mut (*e).data;
        if rec.is_error {
            d.errors += 1;
        } else {
            let x = rec.exec_time_us;
            d.calls += 1;
            d.exec_sum_us += x;
            d.exec_sumsq_us += x * x;
            d.exec_min_us = d.exec_min_us.min(x);
            d.exec_max_us = d.exec_max_us.max(x);
            d.hist[bucket(x)] += 1;
            d.plan_sum_us += rec.plan_time_us;
            d.rows += rec.rows;
            d.shared_blks_hit += rec.shared_blks_hit.max(0) as u64;
            d.shared_blks_read += rec.shared_blks_read.max(0) as u64;
            d.shared_blks_dirtied += rec.shared_blks_dirtied.max(0) as u64;
            d.shared_blks_written += rec.shared_blks_written.max(0) as u64;
            d.temp_blks_read += rec.temp_blks_read.max(0) as u64;
            d.temp_blks_written += rec.temp_blks_written.max(0) as u64;
            d.wal_records += rec.wal_records.max(0) as u64;
            d.wal_fpi += rec.wal_fpi.max(0) as u64;
            d.wal_bytes += rec.wal_bytes;
            d.cpu_user_us += rec.cpu_user_us.max(0) as u64;
            d.cpu_sys_us += rec.cpu_sys_us.max(0) as u64;
            d.blk_read_us += rec.blk_read_time_us.max(0) as u64;
            d.blk_write_us += rec.blk_write_time_us.max(0) as u64;
        }
        for (slot, add) in d.waits.iter_mut().zip(waits) {
            *slot += add;
        }
        spin_unlock(&(*e).lock);
    }
}

/// Make `n` the current interval: clear the generation it will use.
fn rotate(h: &StatsHeader, n: u64) {
    let _guard = stats_lock().acquire(pg_sys::LWLockMode::LW_EXCLUSIVE);
    if n <= h.current.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        let gen = gen_ptr(h, (n % GENS as u64) as usize);
        let old = (*gen).interval_no.load(Ordering::Relaxed);
        if old != 0 && (*gen).archived.load(Ordering::Relaxed) == 0 && gucs::ARCHIVE.get() {
            h.lost_intervals.fetch_add(1, Ordering::Relaxed);
        }
        std::ptr::write_bytes(entry_ptr(gen, 0) as *mut u8, 0, h.entries as usize * ENTRY_SIZE);
        (*gen).used.store(0, Ordering::Relaxed);
        (*gen).archived.store(0, Ordering::Relaxed);
        fence(Ordering::Release);
        (*gen).interval_no.store(n, Ordering::Release);
    }
    h.current.store(n, Ordering::Release);
    h.rotations.fetch_add(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// reading

/// A copy of one entry, with the interval it belongs to.
#[derive(Copy, Clone)]
pub struct Row {
    pub interval_no: u64,
    #[allow(dead_code)] // read by the archiver
    pub archived: bool,
    pub dbid: u32,
    pub userid: u32,
    pub queryid: i64,
    pub plan_id: i64,
    pub data: Data,
}

/// Which generations a snapshot covers.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Which {
    /// Everything that is in memory.
    All,
    /// Only closed intervals the archiver has not stored yet.
    #[allow(dead_code)] // used by the archiver
    ClosedUnarchived,
    /// Everything not yet stored, including the current interval.
    Unarchived,
}

/// Interval length in microseconds, 0 without shared memory.
pub fn interval_us() -> i64 {
    header().map_or(0, |h| h.interval_us)
}

/// Copy entries out of memory. Holds the rotation lock shared, so the
/// generations cannot be cleared underneath.
pub fn snapshot(which: Which) -> Vec<Row> {
    let Some(h) = header() else { return Vec::new() };
    let _guard = stats_lock().acquire(pg_sys::LWLockMode::LW_SHARED);
    let current = h.current.load(Ordering::Acquire);
    let mut out = Vec::new();
    unsafe {
        for g in 0..GENS {
            let gen = gen_ptr(h, g);
            let n = (*gen).interval_no.load(Ordering::Acquire);
            if n == 0 {
                continue;
            }
            let archived = (*gen).archived.load(Ordering::Acquire) != 0;
            match which {
                Which::All => {}
                Which::ClosedUnarchived if archived || n >= current => continue,
                Which::Unarchived if archived => continue,
                _ => {}
            }
            for i in 0..h.entries as usize {
                let e = entry_ptr(gen, i);
                if (*e).state.load(Ordering::Acquire) != 2 {
                    continue;
                }
                spin_lock(&(*e).lock);
                let data = (*e).data;
                spin_unlock(&(*e).lock);
                out.push(Row {
                    interval_no: n,
                    archived,
                    dbid: (*e).dbid,
                    userid: (*e).userid,
                    queryid: (*e).queryid,
                    plan_id: (*e).plan_id,
                    data,
                });
            }
        }
    }
    out
}

/// Record that the archiver has stored interval `n`.
#[allow(dead_code)] // used by the archiver
pub fn mark_archived(n: u64) {
    let Some(h) = header() else { return };
    let _guard = stats_lock().acquire(pg_sys::LWLockMode::LW_SHARED);
    unsafe {
        for g in 0..GENS {
            let gen = gen_ptr(h, g);
            if (*gen).interval_no.load(Ordering::Acquire) == n {
                (*gen).archived.store(1, Ordering::Release);
            }
        }
    }
}

/// Forget all in-memory statistics and cached query texts.
pub fn reset() {
    let Some(h) = header() else { return };
    let _guard = stats_lock().acquire(pg_sys::LWLockMode::LW_EXCLUSIVE);
    unsafe {
        for g in 0..GENS {
            let gen = gen_ptr(h, g);
            (*gen).interval_no.store(0, Ordering::Release);
            (*gen).used.store(0, Ordering::Relaxed);
            (*gen).archived.store(0, Ordering::Relaxed);
            std::ptr::write_bytes(entry_ptr(gen, 0) as *mut u8, 0, h.entries as usize * ENTRY_SIZE);
        }
        std::ptr::write_bytes(
            text_ptr(h, 0) as *mut u8,
            0,
            h.text_cap as usize * text_stride(h.text_len as usize),
        );
    }
    h.current.store(0, Ordering::Release);
}

pub struct GenInfo {
    pub interval_no: u64,
    pub entries: u32,
    pub archived: bool,
}

pub struct Info {
    pub interval_us: i64,
    pub capacity: u32,
    pub current: u64,
    pub rotations: u64,
    pub lost_intervals: u64,
    pub dropped_entries: u64,
    pub texts_dropped: u64,
    pub gens: Vec<GenInfo>,
}

pub fn info() -> Option<Info> {
    let h = header()?;
    let mut gens = Vec::new();
    unsafe {
        for g in 0..GENS {
            let gen = gen_ptr(h, g);
            let n = (*gen).interval_no.load(Ordering::Acquire);
            if n != 0 {
                gens.push(GenInfo {
                    interval_no: n,
                    entries: (*gen).used.load(Ordering::Relaxed),
                    archived: (*gen).archived.load(Ordering::Relaxed) != 0,
                });
            }
        }
    }
    gens.sort_by_key(|g| g.interval_no);
    Some(Info {
        interval_us: h.interval_us,
        capacity: h.entries,
        current: h.current.load(Ordering::Acquire),
        rotations: h.rotations.load(Ordering::Relaxed),
        lost_intervals: h.lost_intervals.load(Ordering::Relaxed),
        dropped_entries: h.dropped_entries.load(Ordering::Relaxed),
        texts_dropped: h.texts_dropped.load(Ordering::Relaxed),
        gens,
    })
}

// ---------------------------------------------------------------------------
// query text cache: a small cache keyed by (database, queryid). Windows of
// TEXT_PROBE slots; a full window evicts its least recently used slot.

unsafe fn text_put(h: &StatsHeader, dbid: u32, queryid: i64, text: &[u8], interval: u64) {
    unsafe {
        let cap = h.text_cap as usize;
        if cap == 0 || h.text_len == 0 || text.is_empty() {
            return;
        }
        let start = (mix(dbid, queryid, 0, 0) as usize) % cap;
        let mut victim: Option<*mut TextHeader> = None;
        let mut oldest = u64::MAX;
        for i in 0..TEXT_PROBE.min(cap) {
            let t = text_ptr(h, (start + i) % cap);
            if (*t).len > 0 && (*t).dbid == dbid && (*t).queryid == queryid {
                (*t).last_used.store(interval, Ordering::Relaxed);
                return;
            }
            let age = if (*t).len == 0 { 0 } else { (*t).last_used.load(Ordering::Relaxed) + 1 };
            if age < oldest {
                oldest = age;
                victim = Some(t);
            }
        }
        let Some(t) = victim else { return };
        let text = ring::clip_text(text, h.text_len as usize);
        let ver = (*t).ver.load(Ordering::Acquire);
        if ver & 1 == 1 || (*t).ver.compare_exchange(ver, ver + 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return; // somebody else is writing this slot; the text comes next time
        }
        if (*t).len > 0 {
            h.texts_dropped.fetch_add(1, Ordering::Relaxed); // evicts another query's text
        }
        (*t).dbid = dbid;
        (*t).queryid = queryid;
        (*t).len = text.len() as u32;
        (*t).last_used.store(interval, Ordering::Relaxed);
        std::ptr::copy_nonoverlapping(text.as_ptr(), (t as *mut u8).add(TEXT_HEADER), text.len());
        (*t).ver.store(ver + 2, Ordering::Release);
    }
}

/// The cached text of a query, if there is one.
pub fn text_get(dbid: u32, queryid: i64) -> Option<Vec<u8>> {
    let h = header()?;
    let cap = h.text_cap as usize;
    if cap == 0 {
        return None;
    }
    let start = (mix(dbid, queryid, 0, 0) as usize) % cap;
    unsafe {
        for i in 0..TEXT_PROBE.min(cap) {
            let t = text_ptr(h, (start + i) % cap);
            for _ in 0..4 {
                let v1 = (*t).ver.load(Ordering::Acquire);
                if v1 & 1 == 1 {
                    continue;
                }
                let (len, d, q) = ((*t).len as usize, (*t).dbid, (*t).queryid);
                if len == 0 || d != dbid || q != queryid {
                    break;
                }
                let bytes =
                    std::slice::from_raw_parts((t as *const u8).add(TEXT_HEADER), len.min(h.text_len as usize)).to_vec();
                fence(Ordering::Acquire);
                if (*t).ver.load(Ordering::Acquire) == v1 {
                    return Some(bytes);
                }
            }
        }
    }
    None
}

/// Every cached query text.
pub fn all_texts() -> Vec<(u32, i64, Vec<u8>)> {
    let Some(h) = header() else { return Vec::new() };
    let mut out = Vec::new();
    unsafe {
        for i in 0..h.text_cap as usize {
            let t = text_ptr(h, i);
            if (*t).len == 0 {
                continue;
            }
            if let Some(text) = text_get((*t).dbid, (*t).queryid) {
                out.push(((*t).dbid, (*t).queryid, text));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_are_log_scale() {
        assert_eq!(bucket(0.3), 0);
        assert_eq!(bucket(1.0), 0);
        assert_eq!(bucket(2.0), 2);
        assert_eq!(bucket(1_000.0), 19);
        assert_eq!(bucket(1.0e12), HIST_BUCKETS - 1);
        assert!(bucket_lower_us(20) > 1000.0 && bucket_lower_us(19) <= 1000.0 * 1.0001);
    }

    #[test]
    fn entry_size_is_stable() {
        // Documented in the README's memory estimate; update both together.
        assert_eq!(ENTRY_SIZE % 8, 0);
        assert_eq!(ENTRY_SIZE, 488);
    }
}
