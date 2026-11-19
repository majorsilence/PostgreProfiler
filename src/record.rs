//! The fixed-size header stored for every recorded execution.
//!
//! The record lives in shared memory and is copied verbatim, so it is plain
//! `#[repr(C)]` data with no pointers. Query text follows it inline in the slot.

/// Command type of a recorded statement.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CmdKind {
    Unknown = 0,
    Select = 1,
    Insert = 2,
    Update = 3,
    Delete = 4,
    Merge = 5,
    Utility = 6,
}

impl CmdKind {
    pub fn as_str(v: u8) -> &'static str {
        match v {
            1 => "select",
            2 => "insert",
            3 => "update",
            4 => "delete",
            5 => "merge",
            6 => "utility",
            _ => "unknown",
        }
    }
}

#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct TraceRecord {
    /// Monotonic sequence number, assigned by `ring::push`.
    pub seq: u64,
    pub queryid: i64,
    /// `TimestampTz` (microseconds since 2000-01-01 UTC) at statement start.
    pub start_ts: i64,
    pub exec_time_us: f64,
    pub plan_time_us: f64,
    pub cpu_user_us: i64,
    pub cpu_sys_us: i64,
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
    pub cmd_type: u8,
    pub toplevel: bool,
    pub is_error: bool,
    pub sqlstate: [u8; 5],
    /// Number of valid bytes of query text stored after the record.
    pub query_len: u32,
}
