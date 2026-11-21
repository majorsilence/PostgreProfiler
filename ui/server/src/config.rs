//! Configuration from the environment.
//!
//! * `PROFILER_DB_URL` (required): connection string of a role that is a member
//!   of `profiler_reader`. It supplies everything the UI shows.
//! * `PROFILER_ADMIN_DB_URL` (optional): a role in `profiler_admin` (and
//!   `pg_signal_backend` for cancel/terminate). Without it the admin actions are
//!   switched off.
//! * `UI_LISTEN` (default `0.0.0.0:8080`).
//! * Users: `UI_USERS` / `UI_USERS_FILE` hold lines `name:role:argon2-hash` with
//!   role `admin` or `viewer` (create hashes with `pg_profiler_ui hash-password`);
//!   `UI_ADMIN_PASSWORD` and `UI_VIEWER_PASSWORD` are conveniences for demos that
//!   add the users `admin` and `viewer` with that plain password.
//! * `UI_SESSION_HOURS` (default 12), `UI_COOKIE_SECURE` (set to `1` behind TLS).

use crate::auth::{hash_password, Role, User};
use std::net::SocketAddr;

pub struct Config {
    pub db_url: String,
    pub admin_db_url: Option<String>,
    pub listen: SocketAddr,
    pub users: Vec<User>,
    pub session_hours: u64,
    pub cookie_secure: bool,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Parse `name:role:hash` lines (separated by newlines or `;`).
pub fn parse_users(text: &str) -> Result<Vec<User>, String> {
    let mut users = Vec::new();
    for line in text.split(['\n', ';']).map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let mut parts = line.splitn(3, ':');
        let (Some(name), Some(role), Some(hash)) = (parts.next(), parts.next(), parts.next()) else {
            return Err("a user line must look like name:role:hash".to_string());
        };
        let role = Role::parse(role).ok_or_else(|| format!("unknown role \"{role}\" (use admin or viewer)"))?;
        users.push(User { name: name.to_string(), role, hash: hash.to_string() });
    }
    Ok(users)
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        let db_url = env("PROFILER_DB_URL").ok_or("PROFILER_DB_URL is not set")?;
        let listen = env("UI_LISTEN")
            .unwrap_or_else(|| "0.0.0.0:8080".to_string())
            .parse()
            .map_err(|e| format!("UI_LISTEN: {e}"))?;

        let mut users = Vec::new();
        if let Some(text) = env("UI_USERS") {
            users.extend(parse_users(&text)?);
        }
        if let Some(path) = env("UI_USERS_FILE") {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("UI_USERS_FILE {path}: {e}"))?;
            users.extend(parse_users(&text)?);
        }
        for (var, name, role) in [("UI_ADMIN_PASSWORD", "admin", Role::Admin), ("UI_VIEWER_PASSWORD", "viewer", Role::Viewer)] {
            if let Some(pw) = env(var) {
                users.push(User { name: name.to_string(), role, hash: hash_password(&pw)? });
            }
        }
        if users.is_empty() {
            return Err("no users: set UI_USERS, UI_USERS_FILE or UI_ADMIN_PASSWORD".to_string());
        }

        Ok(Config {
            db_url,
            admin_db_url: env("PROFILER_ADMIN_DB_URL"),
            listen,
            users,
            session_hours: env("UI_SESSION_HOURS").and_then(|v| v.parse().ok()).unwrap_or(12),
            cookie_secure: env("UI_COOKIE_SECURE").is_some_and(|v| v == "1" || v == "true"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_lines() {
        let u = parse_users("alice:admin:$argon2id$x\n# comment\nbob:viewer:$argon2id$y;carol:viewer:h").unwrap();
        assert_eq!(u.len(), 3);
        assert_eq!(u[0].name, "alice");
        assert_eq!(u[0].role, Role::Admin);
        assert_eq!(u[1].hash, "$argon2id$y");
    }

    #[test]
    fn rejects_bad_lines() {
        assert!(parse_users("alice").is_err());
        assert!(parse_users("alice:root:hash").is_err());
    }
}
