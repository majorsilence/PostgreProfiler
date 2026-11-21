//! pg_profiler UI server: a small HTTP service in front of the extension's SQL
//! API, with login, a live stream and the embedded single-page app.

mod api;
mod assets;
mod auth;
mod config;
mod db;
mod error;
mod stream;

use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // `pg_profiler_ui hash-password [PASSWORD]` prints an Argon2 hash for UI_USERS.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("hash-password") {
        let password = match args.get(2) {
            Some(p) => p.clone(),
            None => {
                let mut line = String::new();
                if std::io::stdin().read_line(&mut line).is_err() {
                    eprintln!("could not read the password from stdin");
                    std::process::exit(2);
                }
                line.trim_end_matches(['\r', '\n']).to_string()
            }
        };
        match auth::hash_password(&password) {
            Ok(h) => println!("{h}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }
        return;
    }

    let cfg = match config::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(2);
        }
    };
    let reader = db::pool(&cfg.db_url, 8).unwrap_or_else(|e| {
        eprintln!("configuration error: PROFILER_DB_URL: {e}");
        std::process::exit(2)
    });
    let admin = cfg.admin_db_url.as_deref().map(|u| {
        db::pool(u, 2).unwrap_or_else(|e| {
            eprintln!("configuration error: PROFILER_ADMIN_DB_URL: {e}");
            std::process::exit(2)
        })
    });

    let hub = stream::Hub::new();
    hub.spawn(reader.clone());
    let state = Arc::new(api::AppState {
        reader,
        admin,
        auth: auth::Auth::new(cfg.users, cfg.session_hours),
        hub,
        cookie_secure: cfg.cookie_secure,
    });

    let listener = match tokio::net::TcpListener::bind(cfg.listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on {}: {e}", cfg.listen);
            std::process::exit(1);
        }
    };
    tracing::info!("pg_profiler UI listening on http://{}", cfg.listen);
    let app = api::router(state);
    let shutdown = async {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => {}
                _ = term.recv() => {}
            }
        }
        #[cfg(not(unix))]
        let _ = ctrl_c.await;
    };
    if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown).await {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}
