//! Capture rules: a small shared-memory list that decides which statement
//! records are kept in the ring.
//!
//! With no rules every record is kept. With rules, a record is kept if it
//! matches **any** rule; a rule matches when **all** of its set conditions hold.
//! Rules are evaluated when a record is about to be pushed, so they save ring
//! space and noise, not instrumentation overhead.

use crate::record::TraceRecord;
use crate::ring;
use pgrx::pg_sys;
use std::ptr;

pub const MAX_RULES: usize = 16;
pub const NAME_LEN: usize = 64;

#[derive(Copy, Clone)]
#[repr(C)]
pub struct Rule {
    /// 0 = free slot.
    pub id: u32,
    /// 0 = any database / role.
    pub dbid: u32,
    pub userid: u32,
    pub errors_only: u8,
    pub app_len: u8,
    pub text_len: u8,
    pub min_duration_us: f64,
    pub app: [u8; NAME_LEN],
    pub text: [u8; NAME_LEN],
}

impl Rule {
    pub fn empty() -> Rule {
        // SAFETY: plain data; all-zero is a free slot.
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
struct Block {
    count: u32,
    next_id: u32,
    rules: [Rule; MAX_RULES],
}

pub const BLOCK_SIZE: usize = (std::mem::size_of::<Block>() + 7) & !7;

pub fn init(base: *mut u8) {
    unsafe {
        ptr::write_bytes(base, 0, BLOCK_SIZE);
        (*(base as *mut Block)).next_id = 1;
    }
}

fn block() -> *mut Block {
    ring::rules_region() as *mut Block
}

/// Add a rule; returns its id, or an error message if the list is full.
pub fn add(mut rule: Rule) -> Result<u32, &'static str> {
    ring::ensure_ready();
    let _g = ring::rules_lock().acquire(pg_sys::LWLockMode::LW_EXCLUSIVE);
    unsafe {
        let b = block();
        let Some(slot) = (*b).rules.iter_mut().find(|r| r.id == 0) else {
            return Err("too many capture rules (maximum is 16)");
        };
        let id = (*b).next_id;
        (*b).next_id = id.wrapping_add(1).max(1);
        rule.id = id;
        *slot = rule;
        (*b).count += 1;
        Ok(id)
    }
}

pub fn remove(id: u32) -> bool {
    ring::ensure_ready();
    let _g = ring::rules_lock().acquire(pg_sys::LWLockMode::LW_EXCLUSIVE);
    unsafe {
        let b = block();
        match (*b).rules.iter_mut().find(|r| r.id == id && id != 0) {
            Some(r) => {
                *r = Rule::empty();
                (*b).count -= 1;
                true
            }
            None => false,
        }
    }
}

pub fn list() -> Vec<Rule> {
    ring::ensure_ready();
    let _g = ring::rules_lock().acquire(pg_sys::LWLockMode::LW_SHARED);
    unsafe { (*block()).rules.iter().filter(|r| r.id != 0).copied().collect() }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

fn matches(rule: &Rule, rec: &TraceRecord, text: &[u8]) -> bool {
    if rule.dbid != 0 && rule.dbid != rec.dbid {
        return false;
    }
    if rule.userid != 0 && rule.userid != rec.userid {
        return false;
    }
    if rule.errors_only != 0 && !rec.is_error {
        return false;
    }
    // Errors are always interesting, whatever their duration.
    if !rec.is_error && rec.exec_time_us < rule.min_duration_us {
        return false;
    }
    let app = crate::record::c_bytes(&rec.application_name);
    if !app.starts_with(&rule.app[..rule.app_len as usize]) {
        return false;
    }
    contains(text, &rule.text[..rule.text_len as usize])
}

/// Should this record be kept?
pub fn allows(rec: &TraceRecord, text: &[u8]) -> bool {
    let b = block();
    if b.is_null() {
        return true;
    }
    // Fast path: no rules, no lock.
    if unsafe { ptr::read_volatile(&raw const (*b).count) } == 0 {
        return true;
    }
    let _g = ring::rules_lock().acquire(pg_sys::LWLockMode::LW_SHARED);
    unsafe { (*b).rules.iter().any(|r| r.id != 0 && matches(r, rec, text)) }
}
