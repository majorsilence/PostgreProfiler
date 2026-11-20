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
    /// Fingerprint of the plan shape (see `plan.rs`), 0 for statements without one.
    pub plan_id: i64,
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

/// Why a plan was stored (`PlanRecord::reasons` bits).
pub mod plan_reason {
    /// First time this (database, queryid, plan_id) was seen.
    pub const NEW: u8 = 1;
    /// The execution was slower than `pg_profiler.plan_outlier_ms`.
    pub const SLOW: u8 = 2;
    /// An estimate was off by more than `pg_profiler.plan_misestimate_ratio`.
    pub const MISESTIMATE: u8 = 4;
    /// The execution wrote temporary files (a sort or hash spilled).
    pub const SPILL: u8 = 8;

    pub fn names(bits: u8) -> String {
        let mut v = Vec::new();
        for (bit, name) in [(NEW, "new"), (SLOW, "slow"), (MISESTIMATE, "misestimate"), (SPILL, "spill")] {
            if bits & bit != 0 {
                v.push(name);
            }
        }
        v.join(",")
    }
}

/// A stored plan: EXPLAIN output (text or JSON) followed by the query text.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct PlanRecord {
    pub seq: u64,
    /// `TimestampTz` when the plan was stored.
    pub ts: i64,
    pub queryid: i64,
    pub plan_id: i64,
    /// Execution time of the sampled run (0 for plans stored when first seen).
    pub exec_time_us: f64,
    /// Row estimate of the top plan node.
    pub plan_rows: f64,
    /// Largest estimate-vs-actual ratio found (0 without actuals).
    pub max_misestimate: f64,
    pub temp_blks_written: i64,
    pub userid: u32,
    pub dbid: u32,
    pub pid: i32,
    pub reasons: u8,
    /// 0 = text, 1 = JSON.
    pub format: u8,
    /// True if the plan carries actual row counts.
    pub analyzed: bool,
    pub plan_len: u32,
    pub query_len: u32,
}

impl Default for PlanRecord {
    fn default() -> Self {
        // SAFETY: see TraceRecord::default.
        unsafe { std::mem::zeroed() }
    }
}

impl RingRecord for PlanRecord {
    fn set_seq(&mut self, seq: u64) {
        self.seq = seq;
    }
    fn payload_len(&self) -> usize {
        self.plan_len as usize + self.query_len as usize
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The README quotes these sizes for the shared-memory estimate; update it
    /// when they change.
    #[test]
    fn record_sizes() {
        assert_eq!(std::mem::size_of::<TraceRecord>(), 336);
        assert_eq!(std::mem::size_of::<EventRecord>(), 160);
        assert_eq!(std::mem::size_of::<PlanRecord>(), 88);
    }
}
