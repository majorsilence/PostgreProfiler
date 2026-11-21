//! The live feed: one background poller reads new trace rows and events from
//! the database and broadcasts them to every connected browser, so the load on
//! the database does not grow with the number of viewers.

use crate::db;
use deadpool_postgres::Pool;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

const POLL: Duration = Duration::from_millis(500);
const IDLE: Duration = Duration::from_millis(1000);
const BATCH: i32 = 500;

/// New rows since the last poll.
pub struct Batch {
    pub trace: Vec<Value>,
    pub events: Vec<Value>,
    /// Rows were overwritten in the ring before the poller could read them.
    pub lapped: bool,
}

impl Batch {
    pub fn to_json(&self) -> String {
        json!({ "trace": self.trace, "events": self.events, "lapped": self.lapped }).to_string()
    }
}

pub struct Hub {
    tx: broadcast::Sender<Arc<Batch>>,
}

impl Hub {
    pub fn new() -> Arc<Hub> {
        let (tx, _) = broadcast::channel(64);
        Arc::new(Hub { tx })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Batch>> {
        self.tx.subscribe()
    }

    /// Start the poller; it only touches the database while somebody listens.
    pub fn spawn(self: &Arc<Self>, pool: Pool) {
        let hub = self.clone();
        tokio::spawn(async move { hub.run(pool).await });
    }

    async fn run(&self, pool: Pool) {
        let mut trace_cursor: Option<i64> = None;
        let mut event_cursor: Option<i64> = None;
        loop {
            if self.tx.receiver_count() == 0 {
                // nobody listens: forget the cursors so the next viewer starts "now"
                trace_cursor = None;
                event_cursor = None;
                tokio::time::sleep(IDLE).await;
                continue;
            }
            match poll(&pool, &mut trace_cursor, &mut event_cursor).await {
                Ok(Some(batch)) => {
                    let _ = self.tx.send(Arc::new(batch));
                }
                Ok(None) => {}
                Err(_) => tokio::time::sleep(IDLE).await, // already logged
            }
            tokio::time::sleep(POLL).await;
        }
    }
}

fn max_seq(rows: &[Value]) -> Option<i64> {
    rows.iter().filter_map(|r| r.get("seq").and_then(Value::as_i64)).max()
}

fn first_seq(rows: &[Value]) -> Option<i64> {
    rows.iter().filter_map(|r| r.get("seq").and_then(Value::as_i64)).min()
}

async fn poll(
    pool: &Pool,
    trace_cursor: &mut Option<i64>,
    event_cursor: &mut Option<i64>,
) -> Result<Option<Batch>, crate::error::ApiError> {
    // First poll after (re)start: begin at the end of both rings.
    if trace_cursor.is_none() || event_cursor.is_none() {
        let info = db::one(pool, "SELECT next_seq, events_total_written FROM pg_profiler_info()", &[]).await?;
        let events_next = db::value(pool, "SELECT coalesce(max(seq) + 1, 0) FROM pg_profiler_events(1)", &[])
            .await
            .ok()
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        *trace_cursor = Some(info.get("next_seq").and_then(Value::as_i64).unwrap_or(0) - 1);
        *event_cursor = Some(events_next - 1);
        return Ok(None);
    }
    let after_t = trace_cursor.unwrap_or(-1);
    let after_e = event_cursor.unwrap_or(-1);
    let trace = db::rows(pool, "SELECT * FROM pg_profiler_since($1::bigint, $2::int)", &[&after_t, &BATCH]).await?;
    let events =
        db::rows(pool, "SELECT * FROM pg_profiler_events_since($1::bigint, $2::int)", &[&after_e, &BATCH]).await?;
    let trace = trace.as_array().cloned().unwrap_or_default();
    let events = events.as_array().cloned().unwrap_or_default();
    let lapped = first_seq(&trace).is_some_and(|s| s > after_t + 1) || first_seq(&events).is_some_and(|s| s > after_e + 1);
    if let Some(m) = max_seq(&trace) {
        *trace_cursor = Some(m);
    }
    if let Some(m) = max_seq(&events) {
        *event_cursor = Some(m);
    }
    // The cursors have moved past our own statements; just do not show them.
    let foreign = |rows: Vec<Value>| -> Vec<Value> {
        rows.into_iter()
            .filter(|r| r.get("application_name").and_then(Value::as_str) != Some(db::OWN_APP))
            .collect()
    };
    let (trace, events) = (foreign(trace), foreign(events));
    if trace.is_empty() && events.is_empty() {
        return Ok(None);
    }
    Ok(Some(Batch { trace, events, lapped }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_helpers() {
        let rows = vec![json!({"seq": 5}), json!({"seq": 9}), json!({"x": 1})];
        assert_eq!(max_seq(&rows), Some(9));
        assert_eq!(first_seq(&rows), Some(5));
        assert_eq!(max_seq(&[]), None);
    }

    #[test]
    fn batch_json_shape() {
        let b = Batch { trace: vec![json!({"seq": 1})], events: vec![], lapped: true };
        let v: Value = serde_json::from_str(&b.to_json()).unwrap();
        assert_eq!(v["lapped"], true);
        assert_eq!(v["trace"][0]["seq"], 1);
    }
}
