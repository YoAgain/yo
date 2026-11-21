//! The operator's routes behind `Yo-Admin-Token` (hub/README.md): support threads, the to-dos, diagnostics, reserved
//! names, deleting a user, the live strip's stream of Yos. Missing, disabled and incorrect credentials all answer `404`.

use crate::error::{refuse, Res};
use crate::request::{bad, Hubs, Peer, DAY};
use crate::{blobs, db, now_ms, Event, Hub};
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::sse::{self, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_core::Stream;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use yo_wire::{limits, sha256, UserId};

pub(crate) fn routes(hub: &Arc<Hub>) -> Router<Arc<Hub>> {
    Router::new()
        .route("/v1/admin/users/{user}", delete(delete_user))
        .route("/v1/admin/messages", get(get_threads))
        .route("/v1/admin/messages/{install}", get(get_thread).post(post_reply))
        .route("/v1/admin/messages/{install}/handled", post(post_handled))
        .route("/v1/admin/messages/done/{id}", post(post_message_done))
        .route("/v1/admin/todos", get(get_todos).post(post_todo))
        .route("/v1/admin/todos/{id}", post(post_todo_done).delete(delete_todo))
        .route("/v1/admin/diagnostics", get(get_diagnostics).put(put_diagnostics))
        .route("/v1/admin/reserved", get(get_reserved).post(post_reserved).put(put_reserved))
        .route("/v1/admin/reserved/{name}", delete(delete_reserved))
        .route_layer(axum::middleware::from_fn_with_state(hub.clone(), admin_only))
}

/// The live strip's stream, apart from the routes above: it stays open, off the blocking places and the request
/// deadline.
pub(crate) fn live_route(hub: &Arc<Hub>) -> Router<Arc<Hub>> {
    Router::new()
        .route("/v1/admin/live", get(get_live))
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

/// An install id in a path (spec 6.4): 32 hex digits.
fn install(text: &str) -> Res<db::Install> {
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
    hub.count(crate::Event::AccountGone, 1);
    for blob in &blobs {
        hub.blobs.remove(blob);
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- support threads and to-dos ----

async fn get_threads(State(hub): Hubs) -> Res<Json<Value>> {
    Ok(Json(hub.db.support_threads()?))
}

/// One install's thread.
async fn get_thread(State(hub): Hubs, Path(id): Path<String>) -> Res<Json<Value>> {
    Ok(Json(hub.db.admin_support_thread(&install(&id)?)?))
}

async fn post_reply(State(hub): Hubs, Path(id): Path<String>, body: Bytes) -> Res<StatusCode> {
    let id = install(&id)?;
    let text = std::str::from_utf8(&body).map_err(|_| bad())?;
    if text.trim().is_empty() || body.len() > limits::MAX_FEEDBACK_LEN {
        return Err(bad());
    }
    hub.limit_by("support-reply", &id, 100, DAY)?;
    if !hub.db.support_reply(&id, text, now_ms())? {
        return Err(refuse(StatusCode::CONFLICT));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The install's messages handled, up to an id.
async fn post_handled(State(hub): Hubs, Path(id): Path<String>, body: Bytes) -> Res<StatusCode> {
    let through: u64 = parsed(&body)?;
    if through > i64::MAX as u64 {
        return Err(bad());
    }
    hub.db.support_read(&install(&id)?, false, through, now_ms())?;
    Ok(StatusCode::NO_CONTENT)
}

/// "1" done (now), "0" open again.
fn done_body(body: &[u8]) -> Res<Option<u64>> {
    match body {
        b"1" => Ok(Some(now_ms())),
        b"0" => Ok(None),
        _ => Err(bad()),
    }
}

fn found(changed: bool) -> Res<StatusCode> {
    match changed {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(refuse(StatusCode::NOT_FOUND)),
    }
}

/// A support message done ("1") or open again ("0").
async fn post_message_done(State(hub): Hubs, Path(id): Path<i64>, body: Bytes) -> Res<StatusCode> {
    found(hub.db.message_done(id, done_body(&body)?)?)
}

/// The team's to-dos (`todos`): `[{id, text, created_at, done_at}]`, oldest first.
async fn get_todos(State(hub): Hubs) -> Res<Json<Value>> {
    Ok(Json(hub.db.todos()?))
}

/// A to-do done ("1") or open again ("0").
async fn post_todo_done(State(hub): Hubs, Path(id): Path<i64>, body: Bytes) -> Res<StatusCode> {
    found(hub.db.todo_done(id, done_body(&body)?)?)
}

async fn delete_todo(State(hub): Hubs, Path(id): Path<i64>) -> Res<StatusCode> {
    found(hub.db.todo_delete(id)?)
}

/// A to-do, plain UTF-8 text: `201 {"id"}`.
async fn post_todo(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    body: Bytes,
) -> Res<(StatusCode, Json<Value>)> {
    hub.limit_ip("admin-todo", peer, &headers, hub.config.limits.todo_per_ip_day, DAY)?;
    if body.is_empty() || body.len() > 4000 {
        return Err(bad());
    }
    let text = std::str::from_utf8(&body).map_err(|_| bad())?;
    let id = hub.db.todo(text, now_ms())?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

// ---- the live strip ----

/// Server-Sent Events, one per Yo the moment it is counted: `{"k":"yo"}` or `{"k":"media"}`, bots left out, nothing of
/// who; a comment every 15 s keeps the connection open. Ends when the hub shuts down.
async fn get_live(State(hub): Hubs) -> Sse<impl Stream<Item = Result<sse::Event, Infallible>>> {
    use tokio::sync::broadcast::error::RecvError;
    let state = (hub.live.subscribe(), hub.closing.subscribe());
    let yos = futures_util::stream::unfold(state, |(mut yos, mut closing)| async move {
        loop {
            if *closing.borrow_and_update() {
                return None;
            }
            tokio::select! {
                got = yos.recv() => match got {
                    Ok(event) => {
                        let k = if matches!(event, Event::MediaIn) { "media" } else { "yo" };
                        let event = sse::Event::default().data(format!(r#"{{"k":"{k}"}}"#));
                        return Some((Ok(event), (yos, closing)));
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return None,
                },
                changed = closing.changed() => if changed.is_err() {
                    return None;
                },
            }
        }
    });
    Sse::new(yos).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
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
    // The open problems right after the change, for the operator's history (`levels`).
    hub.db.sample_levels(now_ms(), true)?;
    get_diagnostics(State(hub)).await
}

// ---- reserved names (spec 3.3) ----

/// The names, the switch, and the rules they are compared by.
async fn get_reserved(State(hub): Hubs) -> Res<Json<Value>> {
    let mut state = hub.db.reserved_state()?;
    state["name_min"] = json!(hub.config.name_min);
    state["name_max"] = json!(yo_wire::handle::USERNAME_MAX);
    state["name_comparison"] = json!("skeleton");
    state["name_form"] =
        json!("NFC Latin letters, digits, underscore or emoji; no _ first, at least one letter or emoji");
    Ok(Json(state))
}

/// A list of names to reserve: `"Name"` adds it, `{"name": "Name", "note": "why"}` adds it or, if reserved
/// already, replaces its note (empty or absent clears it).
async fn post_reserved(State(hub): Hubs, body: Bytes) -> Res<Json<Value>> {
    let entries: Vec<Value> = serde_json::from_slice(&body).map_err(|_| bad())?;
    if entries.len() > 1000 {
        return Err(bad());
    }
    let (mut plain, mut noted) = (Vec::new(), Vec::new());
    for entry in entries {
        let (name, note, list) = match entry {
            Value::String(name) => (name, None, &mut plain),
            Value::Object(mut fields) => {
                let name = fields.remove("name").and_then(|v| v.as_str().map(str::to_owned)).ok_or_else(bad)?;
                let note = match fields.remove("note") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(note)) => Some(note.trim().to_owned()).filter(|note| !note.is_empty()),
                    Some(_) => return Err(bad()),
                };
                if !fields.is_empty() || note.as_ref().is_some_and(|note| note.chars().count() > RESERVED_NOTE_MAX) {
                    return Err(bad());
                }
                (name, note, &mut noted)
            }
            _ => return Err(bad()),
        };
        if name.trim().is_empty() || name.len() > 256 {
            return Err(bad());
        }
        list.push(db::Reserved { spelling: name.trim().into(), note });
    }
    hub.db.add_reserved(&plain, now_ms())?;
    hub.db.set_reserved_notes(&noted, now_ms())?;
    get_reserved(State(hub)).await
}

/// The longest reason kept with a reserved name, in characters.
const RESERVED_NOTE_MAX: usize = 200;

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
