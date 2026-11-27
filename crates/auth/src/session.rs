//! Sign-in sessions and role checks.
//!
//! - A session is a random 256-bit token in an `HttpOnly; SameSite=Strict`
//!   cookie, kept in memory (a runtime restart signs everyone out) and dropped
//!   after [`IDLE_TIMEOUT`] without requests or operator actions. Watching an
//!   open HMI does not keep a session alive.
//! - The role is read from `users.toml` on every check (cached by file time),
//!   so a role change, a removed user or an edit with `movi users` takes
//!   effect at once.
//! - After [`MAX_FAILURES`] wrong passwords in a row a name is locked for
//!   [`LOCKOUT`]; unknown names cost the same hashing time as known ones.
//!
//! Cross-site requests are refused by the gateway's origin guard; the
//! `SameSite=Strict` cookie is a second line.

use crate::users::{self, Role, User};
use axum::extract::FromRequestParts;
use axum::http::header::COOKIE;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

pub const COOKIE_NAME: &str = "mc_session";
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(8 * 3600);
pub const MAX_FAILURES: u32 = 5;
pub const LOCKOUT: Duration = Duration::from_secs(30);

/// Users and sessions of one runtime.
pub struct Auth {
    file: PathBuf,
    guest_view: bool,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Users as last read, with the file time they were read at.
    cache: Option<(Option<SystemTime>, Vec<User>)>,
    sessions: HashMap<String, Session>,
    failures: HashMap<String, Failures>,
}

struct Session {
    user: String,
    last_seen: Instant,
}

#[derive(Default)]
struct Failures {
    count: u32,
    locked_until: Option<Instant>,
}

/// The signed-in user of a request.
#[derive(Debug, Clone)]
pub struct Signed {
    pub name: String,
    pub role: Role,
    pub(crate) token: String,
}

impl Signed {
    /// Refuses with 403 unless the user's role includes `role`.
    pub fn require(&self, role: Role, doing: &str) -> Result<(), AuthError> {
        if self.role >= role {
            Ok(())
        } else {
            Err(AuthError::Forbidden(format!("{} is signed in as {}; {doing} needs the {role} role", self.name, self.role)))
        }
    }

    /// The session token (to re-check the session later, e.g. on a WebSocket).
    pub fn token(&self) -> &str {
        &self.token
    }
}

/// Who did something, for logs and the alarm journal: inserted into request
/// extensions by the gateway for signed-in requests.
#[derive(Debug, Clone)]
pub struct Actor(pub String);

#[derive(Debug)]
pub enum AuthError {
    /// Not signed in, or the session ended.
    Unauthorized(String),
    Forbidden(String),
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    TooMany(String),
    Internal(String),
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, msg, auth) = match self {
            AuthError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m, Some("required")),
            AuthError::Forbidden(m) => (StatusCode::FORBIDDEN, m, Some("forbidden")),
            AuthError::BadRequest(m) => (StatusCode::BAD_REQUEST, m, None),
            AuthError::NotFound(m) => (StatusCode::NOT_FOUND, m, None),
            AuthError::Conflict(m) => (StatusCode::CONFLICT, m, None),
            AuthError::TooMany(m) => (StatusCode::TOO_MANY_REQUESTS, m, None),
            AuthError::Internal(m) => {
                tracing::error!("auth: {m}");
                (StatusCode::INTERNAL_SERVER_ERROR, m, None)
            }
        };
        let mut body = json!({ "ok": false, "error": msg });
        if let Some(a) = auth {
            body["auth"] = json!(a);
        }
        (status, Json(body)).into_response()
    }
}

impl Auth {
    /// The users of `users_file` (see [`users::users_file`]). Fails when the
    /// file exists but cannot be read, so a broken file never opens anything.
    /// With `guest_view`, the HMI may be watched without signing in.
    pub fn open(users_file: PathBuf, guest_view: bool) -> Result<Arc<Self>, String> {
        let auth = Arc::new(Self { file: users_file, guest_view, inner: Mutex::new(Inner::default()) });
        let n = auth.users()?.len();
        if n == 0 {
            tracing::warn!(file = %auth.file.display(), "no users yet: open the HMI or the studio on this machine to create the first administrator, or run `movi users add`");
        } else {
            tracing::info!(file = %auth.file.display(), users = n, guest_view, "sign-in enabled");
        }
        Ok(auth)
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// Whether the HMI may be watched without signing in.
    pub fn guest_view(&self) -> bool {
        self.guest_view
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The users on disk, re-read when the file changed.
    pub fn users(&self) -> Result<Vec<User>, String> {
        let mut inner = self.lock();
        users_locked(&self.file, &mut inner)
    }

    /// The signed-in user of `headers`, if their session is valid. Counts as
    /// activity (restarts the idle timeout).
    pub fn signed(&self, headers: &HeaderMap) -> Result<Signed, AuthError> {
        let token = cookie(headers).ok_or_else(|| AuthError::Unauthorized("sign in first".into()))?;
        self.check(&token, true)
    }

    /// The user of session `token`, if it is still valid. `touch` restarts the
    /// idle timeout; a periodic re-check of an open WebSocket passes `false`.
    pub fn check(&self, token: &str, touch: bool) -> Result<Signed, AuthError> {
        let mut inner = self.lock();
        let now = Instant::now();
        let name = match inner.sessions.get_mut(token) {
            Some(s) if now.duration_since(s.last_seen) < IDLE_TIMEOUT => {
                if touch {
                    s.last_seen = now;
                }
                s.user.clone()
            }
            Some(_) => {
                inner.sessions.remove(token);
                return Err(AuthError::Unauthorized("your session ended; sign in again".into()));
            }
            None => return Err(AuthError::Unauthorized("sign in first".into())),
        };
        let list = users_locked(&self.file, &mut inner).map_err(AuthError::Internal)?;
        match users::find(&list, &name) {
            Some(i) => Ok(Signed { name: list[i].name.clone(), role: list[i].role, token: token.to_string() }),
            None => {
                inner.sessions.remove(token);
                Err(AuthError::Unauthorized(format!("user {name:?} no longer exists")))
            }
        }
    }

    /// Checks a name and password; on success starts a session and returns its token.
    pub fn sign_in(&self, name: &str, password: &str) -> Result<(String, User), AuthError> {
        let key = name.trim().to_ascii_lowercase();
        let stored = {
            let mut inner = self.lock();
            if let Some(until) = inner.failures.get(&key).and_then(|f| f.locked_until) {
                let now = Instant::now();
                if until > now {
                    let secs = (until - now).as_secs() + 1;
                    return Err(AuthError::TooMany(format!("too many wrong passwords; try again in {secs} s")));
                }
            }
            let list = users_locked(&self.file, &mut inner).map_err(AuthError::Internal)?;
            users::find(&list, name.trim()).map(|i| list[i].clone())
        };
        // Hash outside the lock; unknown names are checked against a dummy hash
        // so the answer time does not tell which names exist.
        let ok = match &stored {
            Some(u) => users::verify_password(password, &u.password),
            None => {
                let _ = users::verify_password(password, dummy_hash());
                false
            }
        };
        let mut inner = self.lock();
        match (ok, stored) {
            (true, Some(user)) => {
                inner.failures.remove(&key);
                let token = new_token().map_err(AuthError::Internal)?;
                let now = Instant::now();
                inner.sessions.retain(|_, s| now.duration_since(s.last_seen) < IDLE_TIMEOUT);
                inner.sessions.insert(token.clone(), Session { user: user.name.clone(), last_seen: now });
                tracing::info!(user = %user.name, role = %user.role, "sign-in");
                Ok((token, user))
            }
            _ => {
                let f = inner.failures.entry(key).or_default();
                f.count += 1;
                if f.count >= MAX_FAILURES {
                    f.count = 0;
                    f.locked_until = Some(Instant::now() + LOCKOUT);
                }
                tracing::warn!(user = %name.trim(), "sign-in refused: wrong name or password");
                Err(AuthError::Unauthorized("wrong user name or password".into()))
            }
        }
    }

    pub fn sign_out(&self, token: &str) {
        if let Some(s) = self.lock().sessions.remove(token) {
            tracing::info!(user = %s.user, "sign-out");
        }
    }

    /// Runs `edit` on the user list under the lock and saves the result.
    pub(crate) fn edit_users<R>(&self, edit: impl FnOnce(&mut Vec<User>) -> Result<R, AuthError>) -> Result<R, AuthError> {
        let mut inner = self.lock();
        let mut list = users_locked(&self.file, &mut inner).map_err(AuthError::Internal)?;
        let out = edit(&mut list)?;
        users::save(&self.file, &list).map_err(AuthError::Internal)?;
        inner.cache = None;
        Ok(out)
    }

    /// Ends every session of `name` except `keep`.
    pub(crate) fn end_sessions_of(&self, name: &str, keep: Option<&str>) {
        self.lock().sessions.retain(|t, s| !s.user.eq_ignore_ascii_case(name) || Some(t.as_str()) == keep);
    }
}

fn users_locked(file: &Path, inner: &mut Inner) -> Result<Vec<User>, String> {
    let mtime = std::fs::metadata(file).and_then(|m| m.modified()).ok();
    if let Some((t, list)) = &inner.cache {
        if *t == mtime && mtime.is_some() {
            return Ok(list.clone());
        }
    }
    let list = users::load(file)?;
    inner.cache = Some((mtime, list.clone()));
    Ok(list)
}

fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| users::hash_password("moviclone-dummy-password").unwrap_or_default())
}

fn new_token() -> Result<String, String> {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).map_err(|e| format!("no random source: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// The session token in the `Cookie` header, if well formed.
pub fn cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|p| p.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE_NAME)
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub(crate) fn session_cookie(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict"))
        .unwrap_or_else(|_| HeaderValue::from_static(""))
}

pub(crate) fn cleared_cookie() -> HeaderValue {
    HeaderValue::from_static("mc_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

/// State that carries an [`Auth`], so [`Signed`] can be extracted.
pub trait HasAuth {
    fn auth(&self) -> &Arc<Auth>;
}

impl HasAuth for Arc<Auth> {
    fn auth(&self) -> &Arc<Auth> {
        self
    }
}

#[axum::async_trait]
impl<S> FromRequestParts<S> for Signed
where
    S: HasAuth + Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        state.auth().signed(&parts.headers)
    }
}
