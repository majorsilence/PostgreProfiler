//! Query Store endpoints: the report functions of the extension, plus the
//! per-query pages (history, plans, outlier runs).
//!
//! Everything is read-only and runs on the reader connection. Time ranges come
//! from the browser as ISO timestamps; they are validated here and passed to
//! the database as text that is cast to `timestamptz` there.

use crate::api::AppState;
use crate::db;
use crate::error::ApiError;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

type Shared = Arc<AppState>;

/// The metrics `pg_profiler_top_consumers` understands.
pub const METRICS: [&str; 10] =
    ["exec_time", "avg_time", "calls", "rows", "reads", "temp", "wal", "cpu", "io_time", "plan_time"];

/// A timestamp as a browser sends it (`2026-01-02T03:04:05.678Z`, with an offset,
/// or with a space). Only the characters such a value needs are accepted.
pub fn valid_ts(s: &str) -> bool {
    !s.is_empty() && s.len() <= 40 && s.chars().all(|c| c.is_ascii_digit() || "T:.+-Z ".contains(c))
}

fn ts(name: &str, v: &str) -> Result<String, ApiError> {
    if valid_ts(v) {
        Ok(v.to_string())
    } else {
        Err(ApiError::new(StatusCode::BAD_REQUEST, format!("{name} is not a timestamp")))
    }
}

fn limit(v: Option<i32>, default: i32) -> i32 {
    v.unwrap_or(default).clamp(1, 500)
}

fn bucket_seconds(v: Option<f64>) -> f64 {
    v.unwrap_or(3600.0).clamp(10.0, 86_400.0)
}

fn big(name: &str, v: &str) -> Result<i64, ApiError> {
    v.parse::<i64>().map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, format!("{name} is not a number")))
}

fn oid(name: &str, v: &str) -> Result<u32, ApiError> {
    v.parse::<u32>().map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, format!("{name} is not an OID")))
}

#[derive(Deserialize)]
pub struct Range {
    from: String,
    to: String,
    bucket: Option<f64>,
    limit: Option<i32>,
}

/// Is there history in this database, and what is in memory?
pub async fn status(State(s): State<Shared>) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        db::one(
            &s.reader,
            "SELECT EXISTS (SELECT 1 FROM pg_profiler_hist_stats) AS has_history, \
                    (SELECT min(interval_start) FROM pg_profiler_hist_stats) AS history_from, \
                    (SELECT max(interval_start) FROM pg_profiler_hist_stats) AS history_to, \
                    (SELECT count(*) FROM pg_profiler_stats_memory()) AS memory_entries, \
                    current_database() AS database",
            &[],
        )
        .await?,
    ))
}

pub async fn overall(State(s): State<Shared>, Query(q): Query<Range>) -> Result<Json<Value>, ApiError> {
    let (from, to, bucket) = (ts("from", &q.from)?, ts("to", &q.to)?, bucket_seconds(q.bucket));
    Ok(Json(
        db::rows(
            &s.reader,
            "SELECT * FROM pg_profiler_overall_consumption($1::text::timestamptz, $2::text::timestamptz, make_interval(secs => $3::float8))",
            &[&from, &to, &bucket],
        )
        .await?,
    ))
}

#[derive(Deserialize)]
pub struct Top {
    from: String,
    to: String,
    metric: Option<String>,
    limit: Option<i32>,
}

pub async fn top(State(s): State<Shared>, Query(q): Query<Top>) -> Result<Json<Value>, ApiError> {
    let (from, to) = (ts("from", &q.from)?, ts("to", &q.to)?);
    let metric = q.metric.unwrap_or_else(|| "exec_time".to_string());
    if !METRICS.contains(&metric.as_str()) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "unknown metric"));
    }
    let n = limit(q.limit, 25);
    Ok(Json(
        db::rows(
            &s.reader,
            "SELECT * FROM pg_profiler_top_consumers($1::text, $2::text::timestamptz, $3::text::timestamptz, $4::int)",
            &[&metric, &from, &to, &n],
        )
        .await?,
    ))
}

#[derive(Deserialize)]
pub struct Variation {
    from: String,
    to: String,
    limit: Option<i32>,
    min_calls: Option<i64>,
}

pub async fn variation(State(s): State<Shared>, Query(q): Query<Variation>) -> Result<Json<Value>, ApiError> {
    let (from, to) = (ts("from", &q.from)?, ts("to", &q.to)?);
    let (n, min_calls) = (limit(q.limit, 25), q.min_calls.unwrap_or(10).max(1));
    Ok(Json(
        db::rows(
            &s.reader,
            "SELECT * FROM pg_profiler_high_variation($1::text::timestamptz, $2::text::timestamptz, $3::int, $4::bigint)",
            &[&from, &to, &n, &min_calls],
        )
        .await?,
    ))
}

#[derive(Deserialize)]
pub struct Regressed {
    recent_from: String,
    recent_to: String,
    baseline_from: String,
    baseline_to: String,
    min_calls: Option<i64>,
    min_ratio: Option<f64>,
    min_z: Option<f64>,
    limit: Option<i32>,
}

pub async fn regressed(State(s): State<Shared>, Query(q): Query<Regressed>) -> Result<Json<Value>, ApiError> {
    let (rf, rt) = (ts("recent_from", &q.recent_from)?, ts("recent_to", &q.recent_to)?);
    let (bf, bt) = (ts("baseline_from", &q.baseline_from)?, ts("baseline_to", &q.baseline_to)?);
    let min_calls = q.min_calls.unwrap_or(10).max(1);
    let min_ratio = q.min_ratio.unwrap_or(1.5).max(1.0);
    let min_z = q.min_z.unwrap_or(3.0).max(0.0);
    let n = limit(q.limit, 50);
    Ok(Json(
        db::rows(
            &s.reader,
            "SELECT * FROM pg_profiler_regressed_queries($1::text::timestamptz, $2::text::timestamptz, \
                $3::text::timestamptz, $4::text::timestamptz, $5::bigint, $6::float8, $7::float8, $8::int)",
            &[&rf, &rt, &bf, &bt, &min_calls, &min_ratio, &min_z, &n],
        )
        .await?,
    ))
}

/// Wait samples per query and category.
pub async fn waits(State(s): State<Shared>, Query(q): Query<Range>) -> Result<Json<Value>, ApiError> {
    let (from, to, n) = (ts("from", &q.from)?, ts("to", &q.to)?, limit(q.limit, 50));
    Ok(Json(
        db::rows(
            &s.reader,
            "SELECT * FROM pg_profiler_wait_stats($1::text::timestamptz, $2::text::timestamptz, $3::int)",
            &[&from, &to, &n],
        )
        .await?,
    ))
}

/// Wait samples per time bucket and category (all queries together).
pub async fn wait_series(State(s): State<Shared>, Query(q): Query<Range>) -> Result<Json<Value>, ApiError> {
    let (from, to, bucket) = (ts("from", &q.from)?, ts("to", &q.to)?, bucket_seconds(q.bucket));
    Ok(Json(
        db::rows(
            &s.reader,
            "SELECT date_bin(make_interval(secs => $3::float8), r.interval_start, $1::text::timestamptz) AS bucket_start, \
                    c.name AS category, sum(w.cnt)::bigint AS samples \
               FROM pg_profiler_runtime_stats r \
              CROSS JOIN LATERAL unnest(r.wait_samples) WITH ORDINALITY AS w(cnt, ord) \
               JOIN unnest(pg_profiler_wait_categories()) WITH ORDINALITY AS c(name, ord) ON c.ord = w.ord \
              WHERE r.interval_start >= $1::text::timestamptz AND r.interval_start < $2::text::timestamptz \
              GROUP BY 1, 2 HAVING sum(w.cnt) > 0 ORDER BY 1, 2",
            &[&from, &to, &bucket],
        )
        .await?,
    ))
}

#[derive(Deserialize)]
pub struct QueryRef {
    dbid: String,
    queryid: String,
    from: Option<String>,
    to: Option<String>,
    plan_id: Option<String>,
}

/// The query text and the intervals of one query (per plan).
pub async fn query_history(State(s): State<Shared>, Query(q): Query<QueryRef>) -> Result<Json<Value>, ApiError> {
    let (dbid, queryid) = (oid("dbid", &q.dbid)?, big("queryid", &q.queryid)?);
    let from = ts("from", q.from.as_deref().unwrap_or(""))?;
    let to = ts("to", q.to.as_deref().unwrap_or(""))?;
    let text = db::one(&s.reader, "SELECT query FROM pg_profiler_queries WHERE dbid = $1::oid AND queryid = $2::bigint", &[&dbid, &queryid]).await?;
    let history = db::rows(
        &s.reader,
        "SELECT * FROM pg_profiler_query_history($1::bigint, $2::text::timestamptz, $3::text::timestamptz, $4::oid)",
        &[&queryid, &from, &to, &dbid],
    )
    .await?;
    Ok(Json(serde_json::json!({ "query": text.get("query"), "history": history })))
}

/// Plan shapes seen for a query, and outlier runs stored with actual rows.
pub async fn query_plans(State(s): State<Shared>, Query(q): Query<QueryRef>) -> Result<Json<Value>, ApiError> {
    let (dbid, queryid) = (oid("dbid", &q.dbid)?, big("queryid", &q.queryid)?);
    let plans = db::rows(
        &s.reader,
        "SELECT plan_id, min(first_seen) AS first_seen, max(last_seen) AS last_seen \
           FROM (SELECT plan_id, first_seen, last_seen FROM pg_profiler_hist_plans \
                  WHERE dbid = $1::oid AND queryid = $2::bigint \
                 UNION ALL \
                 SELECT plan_id, ts, ts FROM pg_profiler_plans(2000) \
                  WHERE dbid = $1::oid AND queryid = $2::bigint AND NOT has_actuals) x \
          GROUP BY plan_id ORDER BY max(last_seen) DESC",
        &[&dbid, &queryid],
    )
    .await?;
    let samples = db::rows(
        &s.reader,
        "SELECT * FROM (SELECT ts, plan_id, reason, exec_time_ms, max_misestimate, temp_blks_written, format \
                          FROM pg_profiler_hist_plan_samples WHERE dbid = $1::oid AND queryid = $2::bigint \
                        UNION ALL \
                        SELECT ts, plan_id, reason, exec_time_ms, max_misestimate, temp_blks_written, format \
                          FROM pg_profiler_plans(2000) \
                         WHERE dbid = $1::oid AND queryid = $2::bigint AND has_actuals) x \
          ORDER BY ts DESC LIMIT 30",
        &[&dbid, &queryid],
    )
    .await?;
    Ok(Json(serde_json::json!({ "plans": plans, "samples": samples })))
}

/// The text of one plan: the stored shape, or (with `ts`) one outlier sample.
pub async fn plan_text(State(s): State<Shared>, Query(q): Query<QueryRef>) -> Result<Json<Value>, ApiError> {
    let (dbid, queryid) = (oid("dbid", &q.dbid)?, big("queryid", &q.queryid)?);
    let plan_id = big("plan_id", q.plan_id.as_deref().unwrap_or(""))?;
    // `from` carries the sample time when a sample (with actual rows) is wanted
    if let Some(at) = q.from.as_deref() {
        let at = ts("from", at)?;
        return Ok(Json(
            db::one(
                &s.reader,
                "SELECT plan, format, reason, ts, exec_time_ms, max_misestimate, true AS has_actuals FROM ( \
                   SELECT ts, plan, format, reason, exec_time_ms, max_misestimate FROM pg_profiler_hist_plan_samples \
                    WHERE dbid = $1::oid AND queryid = $2::bigint AND plan_id = $3::bigint \
                   UNION ALL \
                   SELECT ts, plan, format, reason, exec_time_ms, max_misestimate FROM pg_profiler_plans(2000) \
                    WHERE dbid = $1::oid AND queryid = $2::bigint AND plan_id = $3::bigint AND has_actuals) x \
                 ORDER BY abs(extract(epoch FROM ts - $4::text::timestamptz)) LIMIT 1",
                &[&dbid, &queryid, &plan_id, &at],
            )
            .await?,
        ));
    }
    Ok(Json(
        db::one(
            &s.reader,
            "SELECT plan, format, 'new' AS reason, last_seen AS ts, false AS has_actuals FROM ( \
               SELECT plan, format, last_seen, 0 AS src FROM pg_profiler_hist_plans \
                WHERE dbid = $1::oid AND queryid = $2::bigint AND plan_id = $3::bigint \
               UNION ALL \
               SELECT plan, format, ts, 1 FROM pg_profiler_plans(2000) \
                WHERE dbid = $1::oid AND queryid = $2::bigint AND plan_id = $3::bigint AND NOT has_actuals) x \
             ORDER BY src, last_seen DESC LIMIT 1",
            &[&dbid, &queryid, &plan_id],
        )
        .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_validated() {
        assert!(valid_ts("2026-01-02T03:04:05.678Z"));
        assert!(valid_ts("2026-01-02 03:04:05+00"));
        assert!(!valid_ts(""));
        assert!(!valid_ts("now(); DROP TABLE x"));
        assert!(!valid_ts("2026-01-02T03:04:05'"));
        assert!(!valid_ts(&"1".repeat(41)));
    }

    #[test]
    fn numbers_are_parsed_strictly() {
        assert_eq!(big("q", "-8886085649470066503").unwrap(), -8886085649470066503);
        assert!(big("q", "1; select").is_err());
        assert_eq!(oid("d", "16385").unwrap(), 16385);
        assert!(oid("d", "-1").is_err());
    }

    #[test]
    fn parameters_are_clamped() {
        assert_eq!(limit(Some(100000), 10), 500);
        assert_eq!(limit(None, 10), 10);
        assert_eq!(bucket_seconds(Some(1.0)), 10.0);
        assert_eq!(bucket_seconds(None), 3600.0);
    }

    #[test]
    fn the_metric_list_matches_the_extension() {
        for m in ["exec_time", "avg_time", "calls", "rows", "reads", "temp", "wal", "cpu", "io_time", "plan_time"] {
            assert!(METRICS.contains(&m));
        }
    }
}
