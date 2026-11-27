//! Studio sign-in: sessions, role checks and the user-management routes.
//!
//! - A session is a random 256-bit token in an `HttpOnly; SameSite=Strict`
//!   cookie, kept in memory (a runtime restart signs everyone out) and dropped
//!   after [`IDLE_TIMEOUT`] without requests.
//! - The role is read from `users.toml` on every request (cached by file
//!   time), so a role change, a removed user or an edit with `movi users`
//!   takes effect at once.
//! - After [`MAX_FAILURES`] wrong passwords in a row a name is locked for
//!   [`LOCKOUT`]; unknown names cost the same hashing time as known ones.
//! - With no users yet, the first administrator can be created from the
//!   studio, but only from the machine the runtime runs on (loopback).
//!
//! Cross-site requests are already refused by the gateway's origin guard; the
//! `SameSite=Strict` cookie is a second line.

use crate::users::{self, Role, User};
use axum::extract::{ConnectInfo, FromRequestParts, Path as UrlPath, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

pub const COOKIE_NAME: &str = "mc_studio";
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(8 * 3600);
pub const MAX_FAILURES: u32 = 5;
pub const LOCKOUT: Duration = Duration::from_secs(30);

/// Users and sessions of one studio.
pub struct Auth {
    file: PathBuf,
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

/// The user a request is made by.
#[derive(Debug, Clone)]
pub struct Signed {
    pub name: String,
    pub role: Role,
    token: String,
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
}

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
                tracing::error!("studio auth: {m}");
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
    /// The studio users of `users_file` (see [`users::users_file`]). Fails when
    /// the file exists but cannot be read, so a broken file never opens the studio.
    pub fn open(users_file: PathBuf) -> Result<Arc<Self>, String> {
        let auth = Arc::new(Self { file: users_file, inner: Mutex::new(Inner::default()) });
        let n = auth.users()?.len();
        if n == 0 {
            tracing::warn!(file = %auth.file.display(), "no studio users yet: open the studio on this machine to create the first administrator, or run `movi users add`");
        } else {
            tracing::info!(file = %auth.file.display(), users = n, "studio sign-in enabled");
        }
        Ok(auth)
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The users on disk, re-read when the file changed.
    pub fn users(&self) -> Result<Vec<User>, String> {
        let mut inner = self.lock();
        users_locked(&self.file, &mut inner)
    }

    /// The signed-in user of `headers`, if their session is valid.
    pub fn signed(&self, headers: &HeaderMap) -> Result<Signed, AuthError> {
        let token = cookie(headers).ok_or_else(|| AuthError::Unauthorized("sign in to use the studio".into()))?;
        let mut inner = self.lock();
        let now = Instant::now();
        let name = match inner.sessions.get_mut(&token) {
            Some(s) if now.duration_since(s.last_seen) < IDLE_TIMEOUT => {
                s.last_seen = now;
                s.user.clone()
            }
            Some(_) => {
                inner.sessions.remove(&token);
                return Err(AuthError::Unauthorized("your session ended; sign in again".into()));
            }
            None => return Err(AuthError::Unauthorized("sign in to use the studio".into())),
        };
        let list = users_locked(&self.file, &mut inner).map_err(AuthError::Internal)?;
        match users::find(&list, &name) {
            Some(i) => Ok(Signed { name: list[i].name.clone(), role: list[i].role, token }),
            None => {
                inner.sessions.remove(&token);
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
                tracing::info!(user = %user.name, role = %user.role, "studio sign-in");
                Ok((token, user))
            }
            _ => {
                let f = inner.failures.entry(key).or_default();
                f.count += 1;
                if f.count >= MAX_FAILURES {
                    f.count = 0;
                    f.locked_until = Some(Instant::now() + LOCKOUT);
                }
                tracing::warn!(user = %name.trim(), "studio sign-in refused: wrong name or password");
                Err(AuthError::Unauthorized("wrong user name or password".into()))
            }
        }
    }

    pub fn sign_out(&self, token: &str) {
        self.lock().sessions.remove(token);
    }

    /// Runs `edit` on the user list under the lock and saves the result.
    fn edit_users<R>(&self, edit: impl FnOnce(&mut Vec<User>) -> Result<R, AuthError>) -> Result<R, AuthError> {
        let mut inner = self.lock();
        let mut list = users_locked(&self.file, &mut inner).map_err(AuthError::Internal)?;
        let out = edit(&mut list)?;
        users::save(&self.file, &list).map_err(AuthError::Internal)?;
        inner.cache = None;
        Ok(out)
    }

    /// Ends every session of `name` except `keep`.
    fn end_sessions_of(&self, name: &str, keep: Option<&str>) {
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

fn cookie(headers: &HeaderMap) -> Option<String> {
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

fn session_cookie(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict"))
        .unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn cleared_cookie() -> HeaderValue {
    HeaderValue::from_static("mc_studio=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

/// Studio state that carries an [`Auth`].
pub trait HasAuth {
    fn auth(&self) -> &Arc<Auth>;
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

// ---------------------------------------------------------------- handlers

fn user_json(name: &str, role: Role) -> Value {
    json!({ "name": name, "role": role })
}

fn is_loopback(addr: Option<ConnectInfo<SocketAddr>>) -> bool {
    addr.is_some_and(|ConnectInfo(a)| a.ip().is_loopback())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, AuthError> + Send + 'static) -> Result<T, AuthError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| AuthError::Internal(format!("task failed: {e}")))?
}

/// `GET /api/studio/session`: who is signed in, and whether the first user is still to be created.
pub async fn get_session<S: HasAuth + Send + Sync>(
    State(state): State<S>,
    addr: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
) -> Result<Json<Value>, AuthError> {
    let auth = state.auth();
    let setup = auth.users().map_err(AuthError::Internal)?.is_empty();
    let user = auth.signed(&headers).ok().map(|s| user_json(&s.name, s.role));
    Ok(Json(json!({ "ok": true, "user": user, "setup": setup, "can_setup": setup && is_loopback(addr) })))
}

#[derive(Deserialize)]
pub struct Credentials {
    name: String,
    password: String,
}

fn signed_in(token: &str, user: &User) -> Response {
    let mut res = Json(json!({ "ok": true, "user": user_json(&user.name, user.role) })).into_response();
    res.headers_mut().insert(SET_COOKIE, session_cookie(token));
    res
}

/// `POST /api/studio/session` `{name, password}`: signs in.
pub async fn post_session<S: HasAuth + Send + Sync>(
    State(state): State<S>,
    Json(c): Json<Credentials>,
) -> Result<Response, AuthError> {
    let auth = state.auth().clone();
    let (token, user) = blocking(move || auth.sign_in(&c.name, &c.password)).await?;
    Ok(signed_in(&token, &user))
}

/// `DELETE /api/studio/session`: signs out.
pub async fn delete_session<S: HasAuth + Send + Sync>(State(state): State<S>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie(&headers) {
        state.auth().sign_out(&token);
    }
    let mut res = Json(json!({ "ok": true })).into_response();
    res.headers_mut().insert(SET_COOKIE, cleared_cookie());
    res
}

/// `POST /api/studio/setup` `{name, password}`: creates the first administrator
/// and signs them in. Only while there are no users, and only from loopback.
pub async fn post_setup<S: HasAuth + Send + Sync>(
    State(state): State<S>,
    addr: Option<ConnectInfo<SocketAddr>>,
    Json(c): Json<Credentials>,
) -> Result<Response, AuthError> {
    if !is_loopback(addr) {
        return Err(AuthError::Forbidden(
            "the first administrator can only be created on the machine the runtime runs on (or with `movi users add`)".into(),
        ));
    }
    let name = c.name.trim().to_string();
    users::check_name(&name).map_err(AuthError::BadRequest)?;
    users::check_password(&c.password).map_err(AuthError::BadRequest)?;
    let auth = state.auth().clone();
    let (token, user) = blocking(move || {
        let hash = users::hash_password(&c.password).map_err(AuthError::Internal)?;
        auth.edit_users(|list| {
            if !list.is_empty() {
                return Err(AuthError::Conflict("the studio already has users; sign in instead".into()));
            }
            list.push(User { name: name.clone(), role: Role::Admin, password: hash });
            Ok(())
        })?;
        tracing::info!(user = %name, "first studio administrator created");
        auth.sign_in(&name, &c.password)
    })
    .await?;
    Ok(signed_in(&token, &user))
}

#[derive(Deserialize)]
pub struct PasswordChange {
    current: String,
    new: String,
}

/// `PUT /api/studio/password` `{current, new}`: changes the signed-in user's own password.
pub async fn put_password<S: HasAuth + Send + Sync>(
    State(state): State<S>,
    me: Signed,
    Json(p): Json<PasswordChange>,
) -> Result<Json<Value>, AuthError> {
    users::check_password(&p.new).map_err(AuthError::BadRequest)?;
    let auth = state.auth().clone();
    blocking(move || {
        let list = auth.users().map_err(AuthError::Internal)?;
        let ok = users::find(&list, &me.name).is_some_and(|i| users::verify_password(&p.current, &list[i].password));
        if !ok {
            return Err(AuthError::BadRequest("the current password is wrong".into()));
        }
        let hash = users::hash_password(&p.new).map_err(AuthError::Internal)?;
        auth.edit_users(|list| {
            let i = users::find(list, &me.name).ok_or_else(|| AuthError::NotFound("user not found".into()))?;
            list[i].password = hash;
            Ok(())
        })?;
        auth.end_sessions_of(&me.name, Some(&me.token));
        tracing::info!(user = %me.name, "studio password changed");
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// `GET /api/studio/users` (admin): names and roles, never hashes.
pub async fn get_users<S: HasAuth + Send + Sync>(State(state): State<S>, me: Signed) -> Result<Json<Value>, AuthError> {
    me.require(Role::Admin, "managing users")?;
    let list = state.auth().users().map_err(AuthError::Internal)?;
    let users: Vec<Value> = list.iter().map(|u| user_json(&u.name, u.role)).collect();
    Ok(Json(json!({ "ok": true, "users": users, "roles": Role::ALL })))
}

#[derive(Deserialize)]
pub struct UserChange {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

/// `PUT /api/studio/users/{name}` (admin) `{role?, password?}`: creates a user
/// (both fields needed) or changes their role and/or password.
pub async fn put_user<S: HasAuth + Send + Sync>(
    State(state): State<S>,
    me: Signed,
    UrlPath(name): UrlPath<String>,
    Json(change): Json<UserChange>,
) -> Result<Json<Value>, AuthError> {
    me.require(Role::Admin, "managing users")?;
    users::check_name(&name).map_err(AuthError::BadRequest)?;
    let role = match change.role.as_deref() {
        Some(r) => Some(Role::parse(r).ok_or_else(|| AuthError::BadRequest(format!("unknown role {r:?} (viewer, engineer or admin)")))?),
        None => None,
    };
    if let Some(p) = &change.password {
        users::check_password(p).map_err(AuthError::BadRequest)?;
    }
    let auth = state.auth().clone();
    blocking(move || {
        let hash = change.password.as_deref().map(users::hash_password).transpose().map_err(AuthError::Internal)?;
        let (created, user) = auth.edit_users(|list| match users::find(list, &name) {
            Some(i) => {
                if let Some(role) = role {
                    if list[i].role == Role::Admin && role != Role::Admin && users::admins(list) == 1 {
                        return Err(AuthError::Conflict(format!("{} is the only administrator; make someone else admin first", list[i].name)));
                    }
                    list[i].role = role;
                }
                if let Some(h) = hash.clone() {
                    list[i].password = h;
                }
                Ok((false, list[i].clone()))
            }
            None => {
                let (Some(role), Some(h)) = (role, hash.clone()) else {
                    return Err(AuthError::BadRequest("a new user needs a role and a password".into()));
                };
                list.push(User { name: name.clone(), role, password: h });
                Ok((true, list[list.len() - 1].clone()))
            }
        })?;
        if hash.is_some() && !created {
            let keep = user.name.eq_ignore_ascii_case(&me.name).then_some(me.token.as_str());
            auth.end_sessions_of(&user.name, keep);
        }
        tracing::info!(by = %me.name, user = %user.name, role = %user.role, created, password = hash.is_some(), "studio user saved");
        Ok(Json(json!({ "ok": true, "created": created, "user": user_json(&user.name, user.role) })))
    })
    .await
}

/// `DELETE /api/studio/users/{name}` (admin). The signed-in admin cannot remove themself.
pub async fn delete_user<S: HasAuth + Send + Sync>(
    State(state): State<S>,
    me: Signed,
    UrlPath(name): UrlPath<String>,
) -> Result<Json<Value>, AuthError> {
    me.require(Role::Admin, "managing users")?;
    if name.eq_ignore_ascii_case(&me.name) {
        return Err(AuthError::Conflict("you cannot remove yourself; another administrator can".into()));
    }
    let auth = state.auth().clone();
    blocking(move || {
        let removed = auth.edit_users(|list| {
            let i = users::find(list, &name).ok_or_else(|| AuthError::NotFound(format!("user {name:?} does not exist")))?;
            Ok(list.remove(i))
        })?;
        auth.end_sessions_of(&removed.name, None);
        tracing::info!(by = %me.name, user = %removed.name, "studio user removed");
        Ok(Json(json!({ "ok": true })))
    })
    .await
}
