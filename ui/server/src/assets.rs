//! The built single-page app, embedded in the binary.

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../web/dist"]
struct Assets;

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    // Anything under /api that did not match a route is a real 404, not the app.
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let (file, name, hashed) = match Assets::get(path) {
        Some(f) if !path.is_empty() => (f, path.to_string(), path.starts_with("assets/")),
        _ => match Assets::get("index.html") {
            Some(f) => (f, "index.html".to_string(), false),
            None => return (StatusCode::NOT_FOUND, "the web app was not built into this binary").into_response(),
        },
    };
    let mime = mime_guess::from_path(&name).first_or_octet_stream();
    let cache = if hashed { "public, max-age=31536000, immutable" } else { "no-cache" };
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, cache)
        .body(Body::from(file.data.into_owned()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
