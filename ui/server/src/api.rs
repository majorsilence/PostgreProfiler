//! HTTP API: login, read endpoints over the extension's SQL API, the live
//! stream, and the admin actions.

use crate::assets;
use crate::auth::{cookie_token, Auth, Role, Session, COOKIE};
use crate::db;
use crate::error::ApiError;
use crate::qs;
use crate::stream::Hub;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use deadpool_postgres::Pool;
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

pub struct AppState {
    pub reader: Pool,
    pub admin: Option<Pool>,
    pub auth: Auth,
    pub hub: Arc<Hub>,
    pub cookie_secure: bool,
}

type Shared = Arc<AppState>;

pub fn router(state: Shared) -> Router {
    let api = Router::new()
        .route("/me", get(me))
        .route("/logout", post(logout))
        .route("/health", get(health))
        .route("/trace", get(trace))
        .route("/trace/stream", get(trace_stream))
        .route("/running", get(running))
        .route("/events", get(events))
        .route("/plan", get(plan))
        .route("/settings", get(settings))
        .route("/names", get(names))
        .route("/qs/status", get(qs::status))
        .route("/qs/overall", get(qs::overall))
        .route("/qs/top", get(qs::top))
        .route("/qs/variation", get(qs::variation))
        .route("/qs/regressed", get(qs::regressed))
        .route("/qs/waits", get(qs::waits))
        .route("/qs/wait-series", get(qs::wait_series))
        .route("/qs/query", get(qs::query_history))
        .route("/qs/query/plans", get(qs::query_plans))
        .route("/qs/plan", get(qs::plan_text))
        .route("/rules", post(rule_add))
        .route("/rules/{id}", delete(rule_remove))
        .route("/reset", post(reset))
        .route("/backends/{pid}/cancel", post(cancel_backend))
        .route("/backends/{pid}/terminate", post(terminate_backend))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_login));

    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/login", post(login))
        .nest("/api", api)
        .fallback(assets::serve)
        .layer(middleware::from_fn(csrf_guard))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

// ---------------------------------------------------------------- middleware

/// Mutating requests must carry a header a browser only sends from our own
/// pages (a cross-site form cannot set it), on top of the `SameSite=Strict`
/// cookie.
async fn csrf_guard(req: Request, next: Next) -> Response {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !safe && !req.headers().contains_key("x-profiler-ui") {
        return ApiError::new(StatusCode::FORBIDDEN, "missing X-Profiler-UI header").into_response();
    }
    next.run(req).await
}

async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'",
        ),
    );
    res
}

async fn require_login(State(s): State<Shared>, mut req: Request, next: Next) -> Result<Response, ApiError> {
    let token = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(cookie_token)
        .map(str::to_string);
    let session = token.and_then(|t| s.auth.session(&t)).ok_or_else(ApiError::unauthorized)?;
    req.extensions_mut().insert(session);
    Ok(next.run(req).await)
}

fn require_admin(session: &Session) -> Result<(), ApiError> {
    if session.role == Role::Admin {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

fn admin_pool(s: &AppState) -> Result<&Pool, ApiError> {
    s.admin
        .as_ref()
        .ok_or_else(|| ApiError::new(StatusCode::NOT_IMPLEMENTED, "no admin database connection is configured"))
}

// --------------------------------------------------------------------- login

#[derive(Deserialize)]
struct LoginBody {
    user: String,
    password: String,
}

async fn login(State(s): State<Shared>, Json(body): Json<LoginBody>) -> Result<Response, ApiError> {
    // Argon2 is deliberately slow; keep it off the async threads.
    let state = s.clone();
    let result = tokio::task::spawn_blocking(move || state.auth.login(&body.user, &body.password))
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "login failed"))?;
    let Some((token, session)) = result else {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "wrong user or password"));
    };
    let secure = if s.cookie_secure { "; Secure" } else { "" };
    let cookie = format!(
        "{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{secure}",
        s.auth.ttl_seconds()
    );
    let mut res = Json(json!({ "user": session.user, "role": session.role.as_str() })).into_response();
    res.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(ApiError::db)?);
    Ok(res)
}

async fn logout(State(s): State<Shared>, req: Request) -> Response {
    if let Some(token) = req.headers().get(header::COOKIE).and_then(|v| v.to_str().ok()).and_then(cookie_token) {
        s.auth.logout(token);
    }
    let mut res = Json(json!({ "ok": true })).into_response();
    res.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("pgprofiler_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"),
    );
    res
}

async fn me(State(s): State<Shared>, Extension(session): Extension<Session>) -> Json<Value> {
    Json(json!({
        "user": session.user,
        "role": session.role.as_str(),
        "admin_enabled": s.admin.is_some(),
    }))
}

// ------------------------------------------------------------------ read API

#[derive(Deserialize)]
struct Limit {
    limit: Option<i32>,
}

fn limit(q: &Limit, default: i32) -> i32 {
    q.limit.unwrap_or(default).clamp(1, 5000)
}

async fn health(State(s): State<Shared>) -> Result<Json<Value>, ApiError> {
    let ext = db::one(
        &s.reader,
        "SELECT extversion AS extension_version, pg_profiler_api_version() AS api_version, \
                version() AS server_version, current_user AS connected_as, \
                pg_is_in_recovery() AS in_recovery \
           FROM pg_extension WHERE extname = 'pg_profiler'",
        &[],
    )
    .await?;
    let info = db::one(&s.reader, "SELECT * FROM pg_profiler_info()", &[]).await?;
    // Only exists from version 0.2.0 on.
    let stats = db::one(&s.reader, "SELECT * FROM pg_profiler_stats_info()", &[]).await.unwrap_or(Value::Null);
    Ok(Json(json!({ "extension": ext, "buffers": info, "statistics": stats })))
}

async fn trace(State(s): State<Shared>, Query(q): Query<Limit>) -> Result<Json<Value>, ApiError> {
    let n = limit(&q, 500);
    Ok(Json(db::rows(&s.reader, "SELECT * FROM pg_profiler($1::int) WHERE application_name IS DISTINCT FROM 'pg_profiler_ui'", &[&n]).await?))
}

async fn trace_stream(State(s): State<Shared>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = s.hub.subscribe();
    let stream = BroadcastStream::new(rx).map(|r| {
        Ok(match r {
            Ok(batch) => Event::default().event("batch").data(batch.to_json()),
            // this viewer was too slow; tell it that rows were skipped
            Err(BroadcastStreamRecvError::Lagged(n)) => Event::default().event("lagged").data(n.to_string()),
        })
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn running(State(s): State<Shared>) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        db::rows(
            &s.reader,
            "SELECT pid, usename, datname, application_name, client_addr::text AS client_addr, state, \
                    wait_event_type, wait_event, queryid, cmd_type, start_time, xact_start, \
                    extract(epoch FROM elapsed) * 1000 AS elapsed_ms, blocked_by, query \
               FROM pg_profiler_activity WHERE application_name IS DISTINCT FROM 'pg_profiler_ui' \
              ORDER BY elapsed DESC NULLS LAST",
            &[],
        )
        .await?,
    ))
}

async fn events(State(s): State<Shared>, Query(q): Query<Limit>) -> Result<Json<Value>, ApiError> {
    let n = limit(&q, 500);
    Ok(Json(db::rows(&s.reader, "SELECT * FROM pg_profiler_events($1::int) WHERE application_name IS DISTINCT FROM 'pg_profiler_ui'", &[&n]).await?))
}

#[derive(Deserialize)]
struct PlanQuery {
    queryid: i64,
    plan_id: i64,
}

async fn plan(State(s): State<Shared>, Query(q): Query<PlanQuery>) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        db::one(
            &s.reader,
            "SELECT plan, format, reason, ts, exec_time_ms, has_actuals, max_misestimate \
               FROM pg_profiler_plans(2000) WHERE queryid = $1 AND plan_id = $2 ORDER BY seq DESC LIMIT 1",
            &[&q.queryid, &q.plan_id],
        )
        .await?,
    ))
}

/// Role and database names by OID, so the trace can show names.
async fn names(State(s): State<Shared>) -> Result<Json<Value>, ApiError> {
    let roles = db::rows(&s.reader, "SELECT oid::bigint AS oid, rolname AS name FROM pg_roles", &[]).await?;
    let databases = db::rows(&s.reader, "SELECT oid::bigint AS oid, datname AS name FROM pg_database", &[]).await?;
    Ok(Json(json!({ "roles": roles, "databases": databases })))
}

async fn settings(State(s): State<Shared>) -> Result<Json<Value>, ApiError> {
    let settings = db::rows(
        &s.reader,
        "SELECT name, setting, unit, context, short_desc FROM pg_settings WHERE name LIKE 'pg_profiler.%' ORDER BY name",
        &[],
    )
    .await?;
    let rules = db::rows(&s.reader, "SELECT * FROM pg_profiler_rules()", &[]).await?;
    Ok(Json(json!({ "settings": settings, "rules": rules })))
}

// --------------------------------------------------------------- admin API

#[derive(Deserialize)]
struct RuleBody {
    database: Option<String>,
    role: Option<String>,
    application_name: Option<String>,
    min_duration_ms: Option<f64>,
    query_contains: Option<String>,
    errors_only: Option<bool>,
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

async fn rule_add(
    State(s): State<Shared>,
    Extension(session): Extension<Session>,
    Json(b): Json<RuleBody>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&session)?;
    let pool = admin_pool(&s)?;
    let (db_name, role, app, text) =
        (non_empty(b.database), non_empty(b.role), non_empty(b.application_name), non_empty(b.query_contains));
    let min = b.min_duration_ms.unwrap_or(0.0).max(0.0);
    let errors = b.errors_only.unwrap_or(false);
    let id = db::value(
        pool,
        "pg_profiler_rule_add($1::text, $2::text, $3::text, $4::float8, $5::text, $6::bool)",
        &[&db_name, &role, &app, &min, &text, &errors],
    )
    .await?;
    tracing::info!("{} added capture rule {id}", session.user);
    Ok(Json(json!({ "id": id })))
}

async fn rule_remove(
    State(s): State<Shared>,
    Extension(session): Extension<Session>,
    Path(id): Path<i32>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&session)?;
    let removed = db::value(admin_pool(&s)?, "pg_profiler_rule_remove($1::int)", &[&id]).await?;
    tracing::info!("{} removed capture rule {id}", session.user);
    Ok(Json(json!({ "removed": removed })))
}

async fn reset(State(s): State<Shared>, Extension(session): Extension<Session>) -> Result<Json<Value>, ApiError> {
    require_admin(&session)?;
    let pool = admin_pool(&s)?;
    let client = pool.get().await.map_err(ApiError::db)?;
    client.execute("SELECT pg_profiler_reset()", &[]).await.map_err(ApiError::db)?;
    tracing::info!("{} reset the trace buffers", session.user);
    Ok(Json(json!({ "ok": true })))
}

async fn signal_backend(
    s: &AppState,
    session: &Session,
    pid: i32,
    function: &'static str,
) -> Result<Json<Value>, ApiError> {
    require_admin(session)?;
    let pool = admin_pool(s)?;
    let sql = if function == "cancel" { "pg_cancel_backend($1::int)" } else { "pg_terminate_backend($1::int)" };
    let done = db::value(pool, sql, &[&pid]).await?;
    tracing::info!("{} requested {function} of backend {pid}: {done}", session.user);
    Ok(Json(json!({ "done": done })))
}

async fn cancel_backend(
    State(s): State<Shared>,
    Extension(session): Extension<Session>,
    Path(pid): Path<i32>,
) -> Result<Json<Value>, ApiError> {
    signal_backend(&s, &session, pid, "cancel").await
}

async fn terminate_backend(
    State(s): State<Shared>,
    Extension(session): Extension<Session>,
    Path(pid): Path<i32>,
) -> Result<Json<Value>, ApiError> {
    signal_backend(&s, &session, pid, "terminate").await
}
