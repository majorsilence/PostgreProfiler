//! One shared-memory slot per backend publishing its currently running
//! top-level statement. The owning backend is the only writer of its slot;
//! readers use a seqlock (odd sequence = write in progress) and never block
//! anybody.

use crate::gucs;
use crate::ring;
use pgrx::pg_sys;
use std::ptr;
use std::sync::atomic::{fence, Ordering};

/// Fixed part of a slot. The statement text follows it.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct Slot {
    seq: u32,
    pub active: u32,
    pub pid: i32,
    pub userid: u32,
    pub dbid: u32,
    pub nesting: i32,
    pub queryid: i64,
    pub start_ts: i64,
    pub xact_start: i64,
    pub cmd_type: u8,
    pub text_len: u32,
}

const SLOT_HDR: usize = std::mem::size_of::<Slot>();

fn text_cap() -> usize {
    gucs::RUNNING_QUERY_LEN.get() as usize
}

fn stride() -> usize {
    (SLOT_HDR + text_cap() + 7) & !7
}

fn nslots() -> usize {
    unsafe { pg_sys::MaxBackends as usize }
}

pub fn region_size() -> usize {
    nslots() * stride()
}

pub fn init(base: *mut u8) {
    unsafe { ptr::write_bytes(base, 0, region_size()) }
}

unsafe fn slot_ptr(index: usize) -> *mut u8 {
    unsafe { ring::running_region().add(index * stride()) }
}

fn my_index() -> Option<usize> {
    let base = ring::running_region();
    if base.is_null() {
        return None;
    }
    let n = unsafe { pg_sys::MyProcNumber };
    if n < 0 || n as usize >= nslots() {
        return None;
    }
    Some(n as usize)
}

/// Publish the statement this backend just started running.
pub fn begin(mut slot: Slot, text: &[u8]) {
    let Some(idx) = my_index() else { return };
    unsafe {
        let p = slot_ptr(idx);
        let sp = p as *mut Slot;
        let seq = ptr::read_volatile(&raw const (*sp).seq);
        let odd = seq | 1;
        ptr::write_volatile(&raw mut (*sp).seq, odd);
        fence(Ordering::Release);

        let text = ring::clip_text(text, text_cap());
        slot.seq = odd;
        slot.active = 1;
        slot.text_len = text.len() as u32;
        ptr::write_volatile(sp, slot);
        ptr::copy_nonoverlapping(text.as_ptr(), p.add(SLOT_HDR), text.len());

        fence(Ordering::Release);
        ptr::write_volatile(&raw mut (*sp).seq, odd.wrapping_add(1));
    }
}

/// Mark this backend's slot as idle.
pub fn end() {
    let Some(idx) = my_index() else { return };
    unsafe {
        let sp = slot_ptr(idx) as *mut Slot;
        if ptr::read_volatile(&raw const (*sp).active) == 0 {
            return;
        }
        let seq = ptr::read_volatile(&raw const (*sp).seq);
        let odd = seq | 1;
        ptr::write_volatile(&raw mut (*sp).seq, odd);
        fence(Ordering::Release);
        ptr::write_volatile(&raw mut (*sp).active, 0);
        fence(Ordering::Release);
        ptr::write_volatile(&raw mut (*sp).seq, odd.wrapping_add(1));
    }
}

/// A consistent copy of every active slot.
pub fn snapshot() -> Vec<(Slot, Vec<u8>)> {
    ring::ensure_ready();
    let mut out = Vec::new();
    for i in 0..nslots() {
        unsafe {
            let p = slot_ptr(i);
            let sp = p as *const Slot;
            for _ in 0..16 {
                let s1 = ptr::read_volatile(&raw const (*sp).seq);
                if s1 & 1 == 1 {
                    std::hint::spin_loop();
                    continue;
                }
                fence(Ordering::Acquire);
                let slot = ptr::read_volatile(sp);
                let len = (slot.text_len as usize).min(text_cap());
                let text = std::slice::from_raw_parts(p.add(SLOT_HDR), len).to_vec();
                fence(Ordering::Acquire);
                let s2 = ptr::read_volatile(&raw const (*sp).seq);
                if s1 == s2 {
                    if slot.active == 1 {
                        out.push((slot, text));
                    }
                    break;
                }
            }
        }
    }
    out
}

impl Slot {
    pub fn new(
        pid: i32,
        userid: u32,
        dbid: u32,
        queryid: i64,
        start_ts: i64,
        xact_start: i64,
        cmd_type: u8,
    ) -> Slot {
        Slot {
            seq: 0,
            active: 1,
            pid,
            userid,
            dbid,
            nesting: 0,
            queryid,
            start_ts,
            xact_start,
            cmd_type,
            text_len: 0,
        }
    }
}
