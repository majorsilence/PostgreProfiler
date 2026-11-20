//! The fixed-size headers stored in shared memory.
//!
//! Records are copied verbatim, so they are plain `#[repr(C)]` data with no
//! pointers; all-zero is a valid value. Variable-length payload (query text,
//! parameters, event detail) follows the record inline in its ring slot.

use crate::ring::RingRecord;

/// Command type codes stored in `TraceRecord::cmd_type`.
pub mod cmd {
    pub const SELECT: u8 = 1;
    pub const INSERT: u8 = 2;
    pub const UPDATE: u8 = 3;
    pub const DELETE: u8 = 4;
    pub const MERGE: u8 = 5;
    pub const UTILITY: u8 = 6;

    pub fn name(v: u8) -> &'static str {
        match v {
            SELECT => "select",
            INSERT => "insert",
            UPDATE => "update",
            DELETE => "delete",
            MERGE => "merge",
            UTILITY => "utility",
            _ => "unknown",
        }
    }
}

/// Event kinds stored in `EventRecord::kind`.
pub mod event {
    pub const LOGIN: u8 = 1;
    pub const LOGOUT: u8 = 2;
    pub const AUTH_FAILURE: u8 = 3;
    pub const DEADLOCK: u8 = 4;
    pub const LOCK_WAIT: u8 = 5;
    pub const LOCK_ACQUIRED: u8 = 6;
    pub const LOCK_TIMEOUT: u8 = 7;
    pub const CANCEL: u8 = 8;
    pub const TERMINATED: u8 = 9;

    pub fn name(v: u8) -> &'static str {
        match v {
            LOGIN => "login",
            LOGOUT => "logout",
            AUTH_FAILURE => "auth_failure",
            DEADLOCK => "deadlock",
            LOCK_WAIT => "lock_wait",
            LOCK_ACQUIRED => "lock_acquired",
            LOCK_TIMEOUT => "lock_timeout",
            CANCEL => "cancel",
            TERMINATED => "terminated",
            _ => "unknown",
        }
    }
}

pub const APP_NAME_LEN: usize = 64;
pub const CLIENT_ADDR_LEN: usize = 48;

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct TraceRecord {
    /// Monotonic sequence number, assigned by the ring on push.
    pub seq: u64,
    pub queryid: i64,
    /// `TimestampTz` (microseconds since 2000-01-01 UTC) at statement start.
    pub start_ts: i64,
    /// Start of the enclosing transaction (`TimestampTz`).
    pub xact_start: i64,
    pub exec_time_us: f64,
    pub plan_time_us: f64,
    pub cpu_user_us: i64,
    pub cpu_sys_us: i64,
    pub blk_read_time_us: i64,
    pub blk_write_time_us: i64,
    pub rows: u64,
    pub shared_blks_hit: i64,
    pub shared_blks_read: i64,
    pub shared_blks_dirtied: i64,
    pub shared_blks_written: i64,
    pub local_blks_hit: i64,
    pub local_blks_read: i64,
    pub temp_blks_read: i64,
    pub temp_blks_written: i64,
    pub wal_records: i64,
    pub wal_fpi: i64,
    pub wal_bytes: u64,
    pub userid: u32,
    pub dbid: u32,
    pub pid: i32,
    pub nesting: i32,
    /// Top-level transaction id, 0 if none had been assigned.
    pub xid: u32,
    pub cmd_type: u8,
    pub toplevel: bool,
    pub is_error: bool,
    pub sqlstate: [u8; 5],
    /// Bytes of query text stored after the record.
    pub query_len: u32,
    /// Bytes of bind-parameter text stored after the query text.
    pub params_len: u32,
    pub application_name: [u8; APP_NAME_LEN],
    pub client_addr: [u8; CLIENT_ADDR_LEN],
}

impl Default for TraceRecord {
    fn default() -> Self {
        // SAFETY: plain integers, floats, bools and byte arrays; all-zero is valid.
        unsafe { std::mem::zeroed() }
    }
}

impl RingRecord for TraceRecord {
    fn set_seq(&mut self, seq: u64) {
        self.seq = seq;
    }
    fn payload_len(&self) -> usize {
        self.query_len as usize + self.params_len as usize
    }
}

/// A non-statement event: login, deadlock, lock wait, cancel, ...
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct EventRecord {
    pub seq: u64,
    /// `TimestampTz` when the event was recorded.
    pub ts: i64,
    /// Duration the event refers to (lock wait, session length), in microseconds.
    pub duration_us: i64,
    pub userid: u32,
    pub dbid: u32,
    pub pid: i32,
    pub kind: u8,
    pub sqlstate: [u8; 5],
    pub detail_len: u32,
    pub application_name: [u8; APP_NAME_LEN],
    pub client_addr: [u8; CLIENT_ADDR_LEN],
}

impl Default for EventRecord {
    fn default() -> Self {
        // SAFETY: see TraceRecord::default.
        unsafe { std::mem::zeroed() }
    }
}

impl RingRecord for EventRecord {
    fn set_seq(&mut self, seq: u64) {
        self.seq = seq;
    }
    fn payload_len(&self) -> usize {
        self.detail_len as usize
    }
}

/// Bytes up to the first NUL (or all of them).
pub fn c_bytes(buf: &[u8]) -> &[u8] {
    let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    &buf[..n]
}

/// Copy `src` into a NUL-padded fixed buffer, truncating.
pub fn copy_into(dst: &mut [u8], src: &[u8]) {
    let n = src.len().min(dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&src[..n]);
}
