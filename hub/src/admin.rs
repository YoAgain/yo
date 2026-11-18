//! The operator's routes behind `Yo-Admin-Token` (hub/README.md): support threads, to-dos, diagnostics, reserved
//! names, deleting a user. Missing, disabled and incorrect credentials all answer `404`.

use crate::error::{refuse, Res};
use crate::request::{bad, Hubs, Peer, DAY};
use crate::{blobs, db, now_ms, Hub};
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bytes::Bytes;
use serde_json::{json, Value};
use std::sync::Arc;
use yo_wire::{limits, sha256, UserId};

pub(crate) fn routes(hub: &Arc<Hub>) -> Router<Arc<Hub>> {
    Router::new()
        .route("/v1/admin/users/{user}", delete(delete_user))
        .route("/v1/admin/messages", get(get_threads))
        .route("/v1/admin/messages/{user}", get(get_thread).post(post_reply))
        .route("/v1/admin/messages/{user}/handled", post(post_handled))
        .route("/v1/admin/messages/done/{id}", post(post_done))
        .route("/v1/admin/todos", post(post_todo))
        .route("/v1/admin/todos/{id}", post(post_done))
        .route("/v1/admin/diagnostics", get(get_diagnostics).put(put_diagnostics))
        .route("/v1/admin/reserved", get(get_reserved).post(post_reserved).put(put_reserved))
        .route("/v1/admin/reserved/{name}", delete(delete_reserved))
        .route_layer(axum::middleware::from_fn_with_state(hub.clone(), admin_only))
}

async fn admin_only(State(hub): Hubs, request: Request, next: Next) -> Response {
    use subtle::ConstantTimeEq;
    let token = hub.config.admin_token.as_deref().filter(|token| !token.is_empty());
    let supplied = request.headers().get("yo-admin-token").map(|v| v.as_bytes()).unwrap_or_default();
    // Hashed to one length before the constant-time comparison.
    let admitted = token.is_some_and(|token| bool::from(sha256(token.as_bytes()).ct_eq(&sha256(supplied))));
    match admitted {
        true => next.run(request).await,
        false => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A user id in a path, as the admin page writes it: 32 hex digits.
fn user_id(text: &str) -> Res<UserId> {
    blobs::unhex(text).ok_or_else(bad)
}

/// A body that is a number, as text.
fn parsed<T: std::str::FromStr>(body: &[u8]) -> Res<T> {
    std::str::from_utf8(body).map_err(|_| bad())?.parse().map_err(|_| bad())
}

fn json_body(body: &[u8]) -> Res<Value> {
    serde_json::from_slice(body).map_err(|_| bad())
}

async fn delete_user(State(hub): Hubs, Path(user): Path<String>) -> Res<StatusCode> {
    let blobs = hub.db.admin_delete_user(&user_id(&user)?, now_ms())?.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    for blob in &blobs {
        hub.blobs.remove(blob);
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- support threads and to-dos ----

async fn get_threads(State(hub): Hubs) -> Res<Json<Value>> {
    Ok(Json(hub.db.support_threads()?))
}

/// One user's thread, or "team": the to-dos.
async fn get_thread(State(hub): Hubs, Path(user): Path<String>) -> Res<Json<Value>> {
    let user = if user == "team" { Vec::new() } else { user_id(&user)?.to_vec() };
    Ok(Json(hub.db.admin_support_thread(&user)?))
}

async fn post_reply(State(hub): Hubs, Path(user): Path<String>, body: Bytes) -> Res<StatusCode> {
    let user = user_id(&user)?;
    let text = std::str::from_utf8(&body).map_err(|_| bad())?;
    if text.trim().is_empty() || body.len() > limits::MAX_FEEDBACK_LEN {
        return Err(bad());
    }
    hub.limit_by("support-reply", &user, 100, DAY)?;
    if !hub.db.support_reply(&user, text, now_ms())? {
        return Err(refuse(StatusCode::CONFLICT));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The user's messages handled, up to an id.
async fn post_handled(State(hub): Hubs, Path(user): Path<String>, body: Bytes) -> Res<StatusCode> {
    let through: u64 = parsed(&body)?;
    if through > i64::MAX as u64 {
        return Err(bad());
    }
    hub.db.support_read(&user_id(&user)?, false, through, now_ms())?;
    Ok(StatusCode::NO_CONTENT)
}

/// A message or to-do done ("1") or open again ("0").
async fn post_done(State(hub): Hubs, Path(id): Path<i64>, body: Bytes) -> Res<StatusCode> {
    let done = match body.as_ref() {
        b"1" => Some(now_ms()),
        b"0" => None,
        _ => return Err(bad()),
    };
    if !hub.db.todo_done(id, done)? {
        return Err(refuse(StatusCode::NOT_FOUND));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// A to-do, plain UTF-8 text: `201 {"id"}`.
async fn post_todo(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    body: Bytes,
) -> Res<(StatusCode, Json<Value>)> {
    hub.limit_ip("admin-todo", peer, &headers, hub.config.limits.crash_per_ip_day, DAY)?;
    if body.is_empty() || body.len() > 4000 {
        return Err(bad());
    }
    let text = std::str::from_utf8(&body).map_err(|_| bad())?;
    let id = hub.db.todo(text, now_ms())?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

// ---- diagnostics ----

async fn get_diagnostics(State(hub): Hubs) -> Res<Json<Value>> {
    Ok(Json(hub.db.diagnostics()?))
}

/// `{"id", "done"}`: a diagnostic marked done or open; answers the list.
async fn put_diagnostics(State(hub): Hubs, body: Bytes) -> Res<Json<Value>> {
    let change = json_body(&body)?;
    let id = change["id"].as_i64().filter(|id| *id > 0).ok_or_else(bad)?;
    let done = change["done"].as_bool().ok_or_else(bad)?;
    hub.db.diagnostic_done(id, done)?;
    get_diagnostics(State(hub)).await
}

// ---- reserved names (spec 3.3) ----

/// The names, the switch, and the rules they are compared by.
async fn get_reserved(State(hub): Hubs) -> Res<Json<Value>> {
    let mut state = hub.db.reserved_state()?;
    state["name_min"] = json!(hub.config.name_min);
    state["name_max"] = json!(yo_wire::handle::USERNAME_MAX);
    state["name_comparison"] = json!("skeleton");
    state["name_form"] = json!("NFC Latin letters, digits, underscore or emoji; starts with a letter or emoji");
    Ok(Json(state))
}

/// A list of names to reserve.
async fn post_reserved(State(hub): Hubs, body: Bytes) -> Res<Json<Value>> {
    let names: Vec<String> = serde_json::from_slice(&body).map_err(|_| bad())?;
    if names.len() > 1000 || names.iter().any(|name| name.trim().is_empty() || name.len() > 256) {
        return Err(bad());
    }
    let names: Vec<_> =
        names.into_iter().map(|name| db::Reserved { spelling: name.trim().into(), note: None }).collect();
    hub.db.add_reserved(&names, now_ms())?;
    get_reserved(State(hub)).await
}

/// `{"enabled": bool}`: the switch.
async fn put_reserved(State(hub): Hubs, body: Bytes) -> Res<Json<Value>> {
    let switch = json_body(&body)?;
    let fields = switch.as_object().filter(|fields| fields.len() == 1).ok_or_else(bad)?;
    let enabled = fields.get("enabled").and_then(Value::as_bool).ok_or_else(bad)?;
    hub.db.set_reserved_enabled(enabled)?;
    get_reserved(State(hub)).await
}

async fn delete_reserved(State(hub): Hubs, Path(name): Path<String>) -> Res<StatusCode> {
    hub.db.remove_reserved(&name)?;
    Ok(StatusCode::NO_CONTENT)
}
