//! Local users and sessions.
//!
//! Passwords are verified against Argon2id hashes. Sessions are random 256-bit
//! tokens held in memory (a restart logs everybody out) and sent in an
//! `HttpOnly`, `SameSite=Strict` cookie.

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::RngCore;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const COOKIE: &str = "pgprofiler_session";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Viewer,
    Admin,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "admin" => Some(Role::Admin),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Viewer => "viewer",
        }
    }
}

#[derive(Clone)]
pub struct User {
    pub name: String,
    pub role: Role,
    pub hash: String,
}

pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

#[derive(Clone)]
pub struct Session {
    pub user: String,
    pub role: Role,
}

struct Entry {
    session: Session,
    expires: Instant,
}

pub struct Auth {
    users: Vec<User>,
    sessions: Mutex<HashMap<String, Entry>>,
    ttl: Duration,
    /// Hash that is verified for unknown users so that timing does not tell
    /// which names exist.
    dummy: String,
}

impl Auth {
    pub fn new(users: Vec<User>, session_hours: u64) -> Auth {
        Auth {
            users,
            sessions: Mutex::new(HashMap::new()),
            ttl: Duration::from_secs(session_hours.max(1) * 3600),
            dummy: hash_password("not a password").unwrap_or_default(),
        }
    }

    pub fn login(&self, name: &str, password: &str) -> Option<(String, Session)> {
        let user = self.users.iter().find(|u| u.name == name);
        let ok = match user {
            Some(u) => verify(password, &u.hash),
            None => {
                let _ = verify(password, &self.dummy);
                false
            }
        };
        if !ok {
            return None;
        }
        let user = user?;
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let session = Session { user: user.name.clone(), role: user.role };
        let mut map = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        map.retain(|_, e| e.expires > now);
        map.insert(token.clone(), Entry { session: session.clone(), expires: now + self.ttl });
        Some((token, session))
    }

    pub fn session(&self, token: &str) -> Option<Session> {
        let mut map = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(token) {
            Some(e) if e.expires > Instant::now() => Some(e.session.clone()),
            Some(_) => {
                map.remove(token);
                None
            }
            None => None,
        }
    }

    pub fn logout(&self, token: &str) {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(token);
    }

    pub fn ttl_seconds(&self) -> u64 {
        self.ttl.as_secs()
    }
}

/// Value of the session cookie in a `Cookie` header.
pub fn cookie_token(header: &str) -> Option<&str> {
    header.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == COOKIE).map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> Auth {
        Auth::new(
            vec![
                User { name: "alice".into(), role: Role::Admin, hash: hash_password("secret").unwrap() },
                User { name: "bob".into(), role: Role::Viewer, hash: hash_password("hunter2").unwrap() },
            ],
            1,
        )
    }

    #[test]
    fn login_and_session() {
        let a = auth();
        assert!(a.login("alice", "wrong").is_none());
        assert!(a.login("nobody", "secret").is_none());
        let (token, s) = a.login("alice", "secret").unwrap();
        assert_eq!(s.role, Role::Admin);
        assert_eq!(token.len(), 64);
        assert_eq!(a.session(&token).unwrap().user, "alice");
        a.logout(&token);
        assert!(a.session(&token).is_none());
    }

    #[test]
    fn tokens_are_unique() {
        let a = auth();
        let (t1, _) = a.login("bob", "hunter2").unwrap();
        let (t2, _) = a.login("bob", "hunter2").unwrap();
        assert_ne!(t1, t2);
    }

    #[test]
    fn cookie_parsing() {
        assert_eq!(cookie_token("a=1; pgprofiler_session=abc; b=2"), Some("abc"));
        assert_eq!(cookie_token("a=1"), None);
        assert_eq!(cookie_token("xpgprofiler_session=abc"), None);
    }

    #[test]
    fn bad_hash_never_verifies() {
        assert!(!verify("x", "not a phc string"));
    }
}
