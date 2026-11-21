//! Database access. Everything the UI shows comes from the extension's SQL
//! API; results are turned into JSON by the database (`to_jsonb`), so the
//! server forwards rows without knowing their columns.

use crate::error::ApiError;
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use serde_json::Value;
use std::str::FromStr;
use tokio_postgres::types::ToSql;
use tokio_postgres::NoTls;

/// The application name of the UI's own connections. Its polling would
/// otherwise fill the trace it displays, so rows with this name are left out.
pub const OWN_APP: &str = "pg_profiler_ui";

pub type Params<'a> = &'a [&'a (dyn ToSql + Sync)];

pub fn pool(url: &str, size: usize) -> Result<Pool, String> {
    let mut cfg = tokio_postgres::Config::from_str(url).map_err(|e| format!("invalid connection string: {e}"))?;
    cfg.application_name(OWN_APP);
    let mgr = Manager::from_config(
        cfg,
        NoTls,
        ManagerConfig { recycling_method: RecyclingMethod::Fast },
    );
    Pool::builder(mgr).max_size(size).build().map_err(|e| e.to_string())
}

/// JavaScript numbers are exact only up to 2^53. Query and plan ids are 64-bit,
/// so integers beyond that are sent as strings (the browser only compares and
/// displays them).
pub fn stringify_big(v: &mut Value) {
    const LIMIT: i64 = 9_007_199_254_740_991;
    match v {
        Value::Number(n) => {
            let big = n.as_i64().is_some_and(|i| i.abs() > LIMIT) || n.as_u64().is_some_and(|u| u > LIMIT as u64);
            if big {
                *v = Value::String(n.to_string());
            }
        }
        Value::Array(a) => a.iter_mut().for_each(stringify_big),
        Value::Object(o) => o.values_mut().for_each(stringify_big),
        _ => {}
    }
}

/// Run a query and return its rows as a JSON array of objects.
pub async fn rows(pool: &Pool, sql: &str, params: Params<'_>) -> Result<Value, ApiError> {
    let client = pool.get().await.map_err(ApiError::db)?;
    let wrapped = format!("SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb) FROM ({sql}) t");
    let row = client.query_one(&wrapped, params).await.map_err(ApiError::db)?;
    let mut v = row.get::<_, Value>(0);
    stringify_big(&mut v);
    Ok(v)
}

/// Run a query that returns one row and give it as a JSON object (null if none).
pub async fn one(pool: &Pool, sql: &str, params: Params<'_>) -> Result<Value, ApiError> {
    match rows(pool, sql, params).await? {
        Value::Array(mut v) if !v.is_empty() => Ok(v.remove(0)),
        _ => Ok(Value::Null),
    }
}

/// Run a statement for its effect and return the single value it selects.
pub async fn value(pool: &Pool, sql: &str, params: Params<'_>) -> Result<Value, ApiError> {
    let client = pool.get().await.map_err(ApiError::db)?;
    let wrapped = format!("SELECT to_jsonb(t.v) FROM (SELECT ({sql}) AS v) t");
    let row = client.query_one(&wrapped, params).await.map_err(ApiError::db)?;
    Ok(row.get::<_, Option<Value>>(0).unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn big_integers_become_strings() {
        let mut v = json!([{ "queryid": -8886085649470066503i64, "plan_id": 5817396149895219293u64, "seq": 42, "ms": 1.5 }]);
        stringify_big(&mut v);
        assert_eq!(v[0]["queryid"], "-8886085649470066503");
        assert_eq!(v[0]["plan_id"], "5817396149895219293");
        assert_eq!(v[0]["seq"], 42);
        assert_eq!(v[0]["ms"], 1.5);
    }

    #[test]
    fn safe_integers_stay_numbers() {
        let mut v = json!({ "a": 9007199254740991i64, "b": -9007199254740991i64 });
        stringify_big(&mut v);
        assert!(v["a"].is_number() && v["b"].is_number());
    }
}
