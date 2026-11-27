//! Sign-in and user-management routes (`/api/auth/…`), shared by the HMI and the studio.

use crate::session::{cleared_cookie, cookie, session_cookie, Auth, AuthError, Signed};
use crate::users::{self, Role, User};
use axum::extract::{ConnectInfo, Path as UrlPath, State};
use axum::http::header::SET_COOKIE;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

type St = State<Arc<Auth>>;

/// The `/api/auth/…` routes. The first-administrator setup needs the client
/// address: serve with `into_make_service_with_connect_info::<SocketAddr>()`.
pub fn routes(auth: Arc<Auth>) -> Router {
    Router::new()
        .route("/api/auth/session", get(get_session).post(post_session).delete(delete_session))
        .route("/api/auth/setup", post(post_setup))
        .route("/api/auth/password", put(put_password))
        .route("/api/auth/users", get(get_users))
        .route("/api/auth/users/:name", put(put_user).delete(delete_user))
        .with_state(auth)
}

fn user_json(name: &str, role: Role) -> Value {
    json!({ "name": name, "role": role })
}

fn is_loopback(addr: Option<ConnectInfo<SocketAddr>>) -> bool {
    addr.is_some_and(|ConnectInfo(a)| a.ip().is_loopback())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, AuthError> + Send + 'static) -> Result<T, AuthError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| AuthError::Internal(format!("task failed: {e}")))?
}

/// `GET /api/auth/session`: who is signed in, whether the first user is still
/// to be created, and whether the HMI may be watched without signing in.
async fn get_session(State(auth): St, addr: Option<ConnectInfo<SocketAddr>>, headers: HeaderMap) -> Result<Json<Value>, AuthError> {
    let setup = auth.users().map_err(AuthError::Internal)?.is_empty();
    let user = auth.signed(&headers).ok().map(|s| user_json(&s.name, s.role));
    Ok(Json(json!({
        "ok": true,
        "user": user,
        "setup": setup,
        "can_setup": setup && is_loopback(addr),
        "guest_view": auth.guest_view(),
    })))
}

#[derive(Deserialize)]
struct Credentials {
    name: String,
    password: String,
}

fn signed_in(token: &str, user: &User) -> Response {
    let mut res = Json(json!({ "ok": true, "user": user_json(&user.name, user.role) })).into_response();
    res.headers_mut().insert(SET_COOKIE, session_cookie(token));
    res
}

/// `POST /api/auth/session` `{name, password}`: signs in.
async fn post_session(State(auth): St, Json(c): Json<Credentials>) -> Result<Response, AuthError> {
    let (token, user) = blocking(move || auth.sign_in(&c.name, &c.password)).await?;
    Ok(signed_in(&token, &user))
}

/// `DELETE /api/auth/session`: signs out.
async fn delete_session(State(auth): St, headers: HeaderMap) -> Response {
    if let Some(token) = cookie(&headers) {
        auth.sign_out(&token);
    }
    let mut res = Json(json!({ "ok": true })).into_response();
    res.headers_mut().insert(SET_COOKIE, cleared_cookie());
    res
}

/// `POST /api/auth/setup` `{name, password}`: creates the first administrator
/// and signs them in. Only while there are no users, and only from loopback.
async fn post_setup(State(auth): St, addr: Option<ConnectInfo<SocketAddr>>, Json(c): Json<Credentials>) -> Result<Response, AuthError> {
    if !is_loopback(addr) {
        return Err(AuthError::Forbidden(
            "the first administrator can only be created on the machine the runtime runs on (or with `movi users add`)".into(),
        ));
    }
    let name = c.name.trim().to_string();
    users::check_name(&name).map_err(AuthError::BadRequest)?;
    users::check_password(&c.password).map_err(AuthError::BadRequest)?;
    let (token, user) = blocking(move || {
        let hash = users::hash_password(&c.password).map_err(AuthError::Internal)?;
        auth.edit_users(|list| {
            if !list.is_empty() {
                return Err(AuthError::Conflict("users exist already; sign in instead".into()));
            }
            list.push(User { name: name.clone(), role: Role::Admin, password: hash });
            Ok(())
        })?;
        tracing::info!(user = %name, "first administrator created");
        auth.sign_in(&name, &c.password)
    })
    .await?;
    Ok(signed_in(&token, &user))
}

#[derive(Deserialize)]
struct PasswordChange {
    current: String,
    new: String,
}

/// `PUT /api/auth/password` `{current, new}`: changes the signed-in user's own password.
async fn put_password(State(auth): St, me: Signed, Json(p): Json<PasswordChange>) -> Result<Json<Value>, AuthError> {
    users::check_password(&p.new).map_err(AuthError::BadRequest)?;
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
        auth.end_sessions_of(&me.name, Some(me.token()));
        tracing::info!(user = %me.name, "password changed");
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// `GET /api/auth/users` (admin): names and roles, never hashes.
async fn get_users(State(auth): St, me: Signed) -> Result<Json<Value>, AuthError> {
    me.require(Role::Admin, "managing users")?;
    let list = auth.users().map_err(AuthError::Internal)?;
    let users: Vec<Value> = list.iter().map(|u| user_json(&u.name, u.role)).collect();
    Ok(Json(json!({ "ok": true, "users": users, "roles": Role::ALL })))
}

#[derive(Deserialize)]
struct UserChange {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

/// `PUT /api/auth/users/{name}` (admin) `{role?, password?}`: creates a user
/// (both fields needed) or changes their role and/or password.
async fn put_user(State(auth): St, me: Signed, UrlPath(name): UrlPath<String>, Json(change): Json<UserChange>) -> Result<Json<Value>, AuthError> {
    me.require(Role::Admin, "managing users")?;
    users::check_name(&name).map_err(AuthError::BadRequest)?;
    let role = match change.role.as_deref() {
        Some(r) => Some(Role::parse(r).ok_or_else(|| AuthError::BadRequest(format!("unknown role {r:?} (viewer, operator, engineer or admin)")))?),
        None => None,
    };
    if let Some(p) = &change.password {
        users::check_password(p).map_err(AuthError::BadRequest)?;
    }
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
            let keep = user.name.eq_ignore_ascii_case(&me.name).then_some(me.token());
            auth.end_sessions_of(&user.name, keep);
        }
        tracing::info!(by = %me.name, user = %user.name, role = %user.role, created, password = hash.is_some(), "user saved");
        Ok(Json(json!({ "ok": true, "created": created, "user": user_json(&user.name, user.role) })))
    })
    .await
}

/// `DELETE /api/auth/users/{name}` (admin). The signed-in admin cannot remove themself.
async fn delete_user(State(auth): St, me: Signed, UrlPath(name): UrlPath<String>) -> Result<Json<Value>, AuthError> {
    me.require(Role::Admin, "managing users")?;
    if name.eq_ignore_ascii_case(&me.name) {
        return Err(AuthError::Conflict("you cannot remove yourself; another administrator can".into()));
    }
    blocking(move || {
        let removed = auth.edit_users(|list| {
            let i = users::find(list, &name).ok_or_else(|| AuthError::NotFound(format!("user {name:?} does not exist")))?;
            Ok(list.remove(i))
        })?;
        auth.end_sessions_of(&removed.name, None);
        tracing::info!(by = %me.name, user = %removed.name, "user removed");
        Ok(Json(json!({ "ok": true })))
    })
    .await
}
