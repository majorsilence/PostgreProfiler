//! Plans: a fingerprint of the plan shape, capture of plan text for new plans,
//! and capture with actual row counts for sampled outliers.
//!
//! The fingerprint (`plan_id`) hashes what makes two plans *different* to a
//! person: the node types, the tables and indexes they use, join types and
//! aggregate strategies, and the structure of the tree. Costs, row estimates,
//! constants and parameters are not part of it, so one query keeps one
//! `plan_id` until the planner chooses something else. The value is only
//! meaningful on the server version that produced it (node tags change).

use crate::gucs::{self, PlanFormat};
use crate::record::{plan_reason, PlanRecord};
use crate::ring;
use pgrx::pg_sys::{self, NodeTag};
use std::cell::Cell;
use std::ffi::c_int;

thread_local! {
    /// Set while building a plan text, so nothing we trigger there is captured.
    static IN_CAPTURE: Cell<bool> = const { Cell::new(false) };
}

struct CaptureGuard;

impl CaptureGuard {
    fn enter() -> Option<CaptureGuard> {
        if IN_CAPTURE.with(Cell::get) {
            None
        } else {
            IN_CAPTURE.with(|c| c.set(true));
            Some(CaptureGuard)
        }
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        IN_CAPTURE.with(|c| c.set(false));
    }
}

// ---------------------------------------------------------------------------
// fingerprint

/// FNV-1a over 32-bit words, with a final avalanche.
struct Hasher(u64);

impl Hasher {
    fn new() -> Self {
        Hasher(0xcbf2_9ce4_8422_2325)
    }
    fn word(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn finish(self) -> u64 {
        let mut x = self.0;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 33;
        x
    }
}

const MARK_NULL: u32 = 0xFFFF_0000;
const MARK_END: u32 = 0xFFFF_0001;
const MARK_LEFT: u32 = 0xFFFF_0002;
const MARK_RIGHT: u32 = 0xFFFF_0003;
const MARK_CHILD: u32 = 0xFFFF_0004;
const MARK_SUBPLAN: u32 = 0xFFFF_0005;

unsafe fn list_len(l: *mut pg_sys::List) -> usize {
    if l.is_null() {
        0
    } else {
        unsafe { (*l).length as usize }
    }
}

unsafe fn list_item<T>(l: *mut pg_sys::List, i: usize) -> *mut T {
    unsafe { pg_sys::list_nth(l, i as c_int) as *mut T }
}

/// What a scan's range-table entry refers to: the table's OID, or a marker for
/// subqueries, functions, CTEs and the like.
unsafe fn scan_target(stmt: *mut pg_sys::PlannedStmt, scanrelid: u32) -> u32 {
    unsafe {
        if scanrelid == 0 || scanrelid as usize > list_len((*stmt).rtable) {
            return 0;
        }
        let rte: *mut pg_sys::RangeTblEntry = list_item((*stmt).rtable, scanrelid as usize - 1);
        if rte.is_null() {
            0
        } else if (*rte).rtekind == pg_sys::RTEKind::RTE_RELATION {
            (*rte).relid.to_u32()
        } else {
            0x8000_0000 + (*rte).rtekind
        }
    }
}

/// Fingerprint of a planned statement, never 0.
///
/// # Safety
/// `stmt` must be a valid `PlannedStmt` of a plannable statement.
pub unsafe fn fingerprint(stmt: *mut pg_sys::PlannedStmt) -> i64 {
    unsafe {
        let mut h = Hasher::new();
        h.word((*stmt).commandType);
        walk_plan((*stmt).planTree, stmt, &mut h);
        // Sub-plans (subqueries in expressions, init plans) hang off the
        // statement, in a fixed order.
        for i in 0..list_len((*stmt).subplans) {
            h.word(MARK_SUBPLAN);
            walk_plan(list_item((*stmt).subplans, i), stmt, &mut h);
        }
        let id = h.finish() as i64;
        if id == 0 {
            1
        } else {
            id
        }
    }
}

unsafe fn walk_plan(plan: *mut pg_sys::Plan, stmt: *mut pg_sys::PlannedStmt, h: &mut Hasher) {
    unsafe {
        if plan.is_null() {
            h.word(MARK_NULL);
            return;
        }
        pg_sys::check_stack_depth();
        let tag = (*plan).type_;
        h.word(tag as u32);
        h.word(u32::from((*plan).parallel_aware));

        match tag {
            NodeTag::T_SeqScan
            | NodeTag::T_SampleScan
            | NodeTag::T_BitmapHeapScan
            | NodeTag::T_TidScan
            | NodeTag::T_TidRangeScan
            | NodeTag::T_SubqueryScan
            | NodeTag::T_FunctionScan
            | NodeTag::T_ValuesScan
            | NodeTag::T_TableFuncScan
            | NodeTag::T_CteScan
            | NodeTag::T_NamedTuplestoreScan
            | NodeTag::T_WorkTableScan
            | NodeTag::T_ForeignScan
            | NodeTag::T_CustomScan => {
                h.word(scan_target(stmt, (*(plan as *mut pg_sys::Scan)).scanrelid));
            }
            NodeTag::T_IndexScan => {
                let n = plan as *mut pg_sys::IndexScan;
                h.word(scan_target(stmt, (*n).scan.scanrelid));
                h.word((*n).indexid.to_u32());
                h.word((*n).indexorderdir as u32);
            }
            NodeTag::T_IndexOnlyScan => {
                let n = plan as *mut pg_sys::IndexOnlyScan;
                h.word(scan_target(stmt, (*n).scan.scanrelid));
                h.word((*n).indexid.to_u32());
                h.word((*n).indexorderdir as u32);
            }
            NodeTag::T_BitmapIndexScan => {
                let n = plan as *mut pg_sys::BitmapIndexScan;
                h.word(scan_target(stmt, (*n).scan.scanrelid));
                h.word((*n).indexid.to_u32());
            }
            NodeTag::T_NestLoop | NodeTag::T_MergeJoin | NodeTag::T_HashJoin => {
                h.word((*(plan as *mut pg_sys::Join)).jointype);
            }
            NodeTag::T_Agg => {
                h.word((*(plan as *mut pg_sys::Agg)).aggstrategy);
                h.word((*(plan as *mut pg_sys::Agg)).aggsplit);
            }
            NodeTag::T_SetOp => {
                h.word((*(plan as *mut pg_sys::SetOp)).cmd);
                h.word((*(plan as *mut pg_sys::SetOp)).strategy);
            }
            NodeTag::T_ModifyTable => {
                let n = plan as *mut pg_sys::ModifyTable;
                h.word((*n).operation);
                h.word(scan_target(stmt, (*n).nominalRelation));
            }
            _ => {}
        }

        h.word(MARK_LEFT);
        walk_plan((*plan).lefttree, stmt, h);
        h.word(MARK_RIGHT);
        walk_plan((*plan).righttree, stmt, h);

        let extra = match tag {
            NodeTag::T_Append => (*(plan as *mut pg_sys::Append)).appendplans,
            NodeTag::T_MergeAppend => (*(plan as *mut pg_sys::MergeAppend)).mergeplans,
            NodeTag::T_BitmapAnd => (*(plan as *mut pg_sys::BitmapAnd)).bitmapplans,
            NodeTag::T_BitmapOr => (*(plan as *mut pg_sys::BitmapOr)).bitmapplans,
            NodeTag::T_CustomScan => (*(plan as *mut pg_sys::CustomScan)).custom_plans,
            _ => std::ptr::null_mut(),
        };
        for i in 0..list_len(extra) {
            h.word(MARK_CHILD);
            walk_plan(list_item(extra, i), stmt, h);
        }
        if tag == NodeTag::T_SubqueryScan {
            h.word(MARK_CHILD);
            walk_plan((*(plan as *mut pg_sys::SubqueryScan)).subplan, stmt, h);
        }
        h.word(MARK_END);
    }
}

// ---------------------------------------------------------------------------
// estimate vs actual

/// Visit every node of an executor plan tree below `ps`, except subtrees under
/// a `Limit` (which stop early on purpose and would look mis-estimated).
unsafe fn walk_states(ps: *mut pg_sys::PlanState, f: &mut impl FnMut(*mut pg_sys::PlanState)) {
    unsafe {
        if ps.is_null() {
            return;
        }
        pg_sys::check_stack_depth();
        let tag = (*(*ps).plan).type_;
        f(ps);
        if tag == NodeTag::T_Limit {
            return;
        }
        walk_states((*ps).lefttree, f);
        walk_states((*ps).righttree, f);
        for list in [(*ps).initPlan, (*ps).subPlan] {
            for i in 0..list_len(list) {
                let sp: *mut pg_sys::SubPlanState = list_item(list, i);
                if !sp.is_null() {
                    walk_states((*sp).planstate, f);
                }
            }
        }
        match tag {
            NodeTag::T_Append => {
                let s = ps as *mut pg_sys::AppendState;
                for i in 0..(*s).as_nplans.max(0) as usize {
                    walk_states(*(*s).appendplans.add(i), f);
                }
            }
            NodeTag::T_MergeAppend => {
                let s = ps as *mut pg_sys::MergeAppendState;
                for i in 0..(*s).ms_nplans.max(0) as usize {
                    walk_states(*(*s).mergeplans.add(i), f);
                }
            }
            NodeTag::T_BitmapAnd => {
                let s = ps as *mut pg_sys::BitmapAndState;
                for i in 0..(*s).nplans.max(0) as usize {
                    walk_states(*(*s).bitmapplans.add(i), f);
                }
            }
            NodeTag::T_BitmapOr => {
                let s = ps as *mut pg_sys::BitmapOrState;
                for i in 0..(*s).nplans.max(0) as usize {
                    walk_states(*(*s).bitmapplans.add(i), f);
                }
            }
            NodeTag::T_SubqueryScan => {
                walk_states((*(ps as *mut pg_sys::SubqueryScanState)).subplan, f);
            }
            NodeTag::T_CustomScan => {
                let l = (*(ps as *mut pg_sys::CustomScanState)).custom_ps;
                for i in 0..list_len(l) {
                    walk_states(list_item(l, i), f);
                }
            }
            _ => {}
        }
    }
}

/// Largest factor by which a node's row estimate differs from what it really
/// returned (per loop), over nodes that have counts. 0 if there are none.
/// Nodes where both numbers are small are ignored, as are parallel-aware nodes
/// (their estimates are per process).
unsafe fn max_misestimate(root: *mut pg_sys::PlanState) -> f64 {
    let mut worst = 0.0f64;
    unsafe {
        walk_states(root, &mut |ps| {
            let instr = (*ps).instrument;
            if instr.is_null() || (*(*ps).plan).parallel_aware {
                return;
            }
            // Fold the open loop into the totals (EXPLAIN does the same later).
            pg_sys::InstrEndLoop(instr);
            if (*instr).nloops <= 0.0 {
                return;
            }
            let est = (*(*ps).plan).plan_rows;
            let act = (*instr).ntuples / (*instr).nloops;
            if est.max(act) < 100.0 {
                return;
            }
            let (est, act) = (est.max(1.0), act.max(1.0));
            worst = worst.max((est / act).max(act / est));
        });
    }
    worst
}

// ---------------------------------------------------------------------------
// capture

fn key_of(dbid: u32, queryid: i64, plan_id: i64, salt: u8) -> u64 {
    let mut h = Hasher::new();
    h.word(dbid);
    h.word(queryid as u32);
    h.word((queryid >> 32) as u32);
    h.word(plan_id as u32);
    h.word((plan_id >> 32) as u32);
    h.word(u32::from(salt));
    h.finish()
}

fn enabled() -> bool {
    gucs::ENABLED.get() && gucs::CAPTURE_PLANS.get() && gucs::MAX_PLAN_LEN.get() > 0
}

/// Should this statement be run with row counting, so that it can be stored
/// with actual rows if it turns out to be an outlier?
pub fn sample_for_actuals() -> bool {
    if !enabled() {
        return false;
    }
    let rate = gucs::PLAN_SAMPLE_RATE.get();
    if rate <= 0.0 {
        return false;
    }
    rate >= 1.0 || unsafe { pg_sys::pg_prng_double(&raw mut pg_sys::pg_global_prng_state) < rate }
}

/// EXPLAIN output for a started (or finished) query.
unsafe fn explain(qd: *mut pg_sys::QueryDesc, analyze: bool) -> Vec<u8> {
    unsafe {
        let cxt = pg_sys::AllocSetContextCreateInternal(
            pg_sys::CurrentMemoryContext,
            c"pg_profiler plan".as_ptr(),
            pg_sys::ALLOCSET_DEFAULT_MINSIZE as usize,
            pg_sys::ALLOCSET_DEFAULT_INITSIZE as usize,
            pg_sys::ALLOCSET_DEFAULT_MAXSIZE as usize,
        );
        let old = pg_sys::MemoryContextSwitchTo(cxt);

        let es = pg_sys::NewExplainState();
        (*es).format = match gucs::PLAN_FORMAT.get() {
            PlanFormat::Text => pg_sys::ExplainFormat::EXPLAIN_FORMAT_TEXT,
            PlanFormat::Json => pg_sys::ExplainFormat::EXPLAIN_FORMAT_JSON,
        };
        (*es).analyze = analyze;
        (*es).timing = false;
        (*es).summary = false;
        pg_sys::ExplainBeginOutput(es);
        // EXPLAIN opens a "Query" group around the plan; without it the JSON
        // output is not a valid document.
        pg_sys::ExplainOpenGroup(c"Query".as_ptr(), std::ptr::null(), true, es);
        pg_sys::ExplainPrintPlan(es, qd);
        pg_sys::ExplainCloseGroup(c"Query".as_ptr(), std::ptr::null(), true, es);
        pg_sys::ExplainEndOutput(es);
        let buf = (*es).str_;
        let mut out = std::slice::from_raw_parts((*buf).data as *const u8, (*buf).len as usize).to_vec();
        while out.last() == Some(&b'\n') {
            out.pop();
        }

        pg_sys::MemoryContextSwitchTo(old);
        pg_sys::MemoryContextDelete(cxt);
        out
    }
}

struct Stored {
    plan_id: i64,
    reasons: u8,
    exec_time_us: f64,
    max_misestimate: f64,
    temp_blks_written: i64,
    analyzed: bool,
}

unsafe fn store(qd: *mut pg_sys::QueryDesc, s: &Stored, plan_text: &[u8]) {
    unsafe {
        let ring = ring::plan_ring();
        if !ring.is_ready() {
            return;
        }
        let stmt = &*(*qd).plannedstmt;
        let query = crate::hooks::statement_text((*qd).sourceText, stmt.stmt_location, stmt.stmt_len);
        let plan_text = ring::clip_text(plan_text, gucs::MAX_PLAN_LEN.get() as usize);
        let query = ring::clip_text(query, gucs::MAX_QUERY_LEN.get() as usize);
        let rec = PlanRecord {
            ts: pg_sys::GetCurrentTimestamp(),
            queryid: stmt.queryId as _,
            plan_id: s.plan_id,
            exec_time_us: s.exec_time_us,
            plan_rows: (*(*(*qd).planstate).plan).plan_rows,
            max_misestimate: s.max_misestimate,
            temp_blks_written: s.temp_blks_written,
            userid: pg_sys::GetUserId().to_u32(),
            dbid: pg_sys::MyDatabaseId.to_u32(),
            pid: pg_sys::MyProcPid,
            reasons: s.reasons,
            format: match gucs::PLAN_FORMAT.get() {
                PlanFormat::Text => 0,
                PlanFormat::Json => 1,
            },
            analyzed: s.analyzed,
            plan_len: plan_text.len() as u32,
            query_len: query.len() as u32,
            ..PlanRecord::default()
        };
        ring.push(&rec, &[plan_text, query]);
    }
}

/// Store the plan of a just-started statement if this plan shape has not been
/// seen before. Called after `ExecutorStart`, when the plan state exists.
///
/// # Safety
/// `qd` must be a started `QueryDesc`.
pub unsafe fn capture_new(qd: *mut pg_sys::QueryDesc, plan_id: i64) {
    unsafe {
        if !enabled() || plan_id == 0 || (*qd).planstate.is_null() {
            return;
        }
        let key = key_of(
            pg_sys::MyDatabaseId.to_u32(),
            (*(*qd).plannedstmt).queryId as _,
            plan_id,
            plan_reason::NEW,
        );
        let Some(_guard) = CaptureGuard::enter() else { return };
        if !ring::plan_seen_insert(key) {
            return;
        }
        let text = explain(qd, false);
        store(
            qd,
            &Stored {
                plan_id,
                reasons: plan_reason::NEW,
                exec_time_us: 0.0,
                max_misestimate: 0.0,
                temp_blks_written: 0,
                analyzed: false,
            },
            &text,
        );
    }
}

/// After a statement that ran with row counting: store its plan with actual
/// rows if it was slow, spilled to disk or had a badly wrong estimate. Each
/// plan is stored once per combination of reasons (until the table of seen
/// plans is cleared), so a recurring outlier does not flood the ring.
///
/// # Safety
/// `qd` must be a finished, not yet ended `QueryDesc` run with row counting.
pub unsafe fn capture_outlier(qd: *mut pg_sys::QueryDesc, plan_id: i64, exec_us: f64, temp_blks_written: i64) {
    unsafe {
        if !enabled() || plan_id == 0 || (*qd).planstate.is_null() {
            return;
        }
        let Some(_guard) = CaptureGuard::enter() else { return };
        let ratio = max_misestimate((*qd).planstate);
        let mut reasons = 0u8;
        let slow = gucs::PLAN_OUTLIER_MS.get();
        if slow > 0.0 && exec_us >= slow * 1000.0 {
            reasons |= plan_reason::SLOW;
        }
        let limit = gucs::PLAN_MISESTIMATE_RATIO.get();
        if limit > 0.0 && ratio >= limit {
            reasons |= plan_reason::MISESTIMATE;
        }
        if temp_blks_written > 0 {
            reasons |= plan_reason::SPILL;
        }
        if reasons == 0 {
            return;
        }
        let key = key_of(pg_sys::MyDatabaseId.to_u32(), (*(*qd).plannedstmt).queryId as _, plan_id, reasons);
        if !ring::plan_seen_insert(key) {
            return;
        }
        let text = explain(qd, true);
        store(
            qd,
            &Stored { plan_id, reasons, exec_time_us: exec_us, max_misestimate: ratio, temp_blks_written, analyzed: true },
            &text,
        );
    }
}
