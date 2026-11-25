//! The entrance of api.justyo.app, behind the tunnel: one process for all services, no account, no keys, no book. `/` and
//! `/services` list them; it
//! reads HTTP and hands `/NAME/…` to that service's executor, one JSON line each way over its socket.

use crate::list::List;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRequest, Path, Query, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::{
    routing::{any, get},
    Form, Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Semaphore;

const MAX_BODY: usize = 16 * 1024;
/// One call, or one answer.
pub const MAX_LINE: u64 = 256 * 1024;
const AT_ONCE: usize = 32;
/// The connect pages that wait (`partner.rs`), in places of their own, and whose they are.
const WAITS: usize = 8;
const PAGE: &str = "https://justyo.co";
const TIME: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize)]
pub struct Asked {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: String,
    /// What a form says, or a GET's query.
    pub form: HashMap<String, String>,
}

#[derive(Serialize, Deserialize)]
pub struct Answer {
    pub status: u16,
    /// Nothing to say: null, or an empty object.
    pub body: Value,
    /// Where it leads on to.
    pub location: Option<String>,
}

impl Answer {
    pub fn new(status: u16, body: Value) -> Answer {
        Answer { status, body, location: None }
    }
}

struct Entrance {
    sockets: PathBuf,
    list: List,
    at_once: Semaphore,
    waits: Semaphore,
}

/// Serves until the process ends.
pub fn serve(address: &str, sockets: PathBuf, services: &std::path::Path) -> std::io::Result<()> {
    let list = List::read(services);
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(address).await?;
        if !listener.local_addr()?.ip().is_loopback() {
            return Err(std::io::ErrorKind::AddrNotAvailable.into());
        }
        let entrance =
            Arc::new(Entrance { sockets, list, at_once: Semaphore::new(AT_ONCE), waits: Semaphore::new(WAITS) });
        let routes = Router::new()
            .route("/", get(page))
            .route("/services", get(services_json))
            .route("/services/{logo}", get(logo))
            .route("/{name}/allowed", any(allowed))
            .route("/{name}/{*path}", any(called));
        axum::serve(listener, routes.layer(DefaultBodyLimit::max(MAX_BODY)).with_state(entrance)).await
    })
}

/// For people the list is the website's.
async fn page() -> Response {
    axum::response::Redirect::temporary("https://justyo.co/services").into_response()
}

async fn services_json(State(entrance): State<Arc<Entrance>>) -> Response {
    let any = [(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")];
    listed("application/json; charset=utf-8", any, entrance.list.json.clone().into())
}

async fn logo(State(entrance): State<Arc<Entrance>>, Path(file): Path<String>) -> Response {
    match file.strip_suffix(".png").and_then(|name| entrance.list.logos.get(name)) {
        Some(png) => listed("image/png", [(header::X_CONTENT_TYPE_OPTIONS, "nosniff")], png.clone().into()),
        None => refused(StatusCode::NOT_FOUND, "Not found"),
    }
}

fn listed(kind: &'static str, more: [(header::HeaderName, &'static str); 1], body: Bytes) -> Response {
    ([(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, "public, max-age=3600")], more, body).into_response()
}

fn refused(status: StatusCode, why: &str) -> Response {
    (status, Json(json!({ "errors": [{ "message": why }] }))).into_response()
}

/// The connect page's question: the one path a browser may ask, and only that page.
async fn allowed(
    State(entrance): State<Arc<Entrance>>,
    Path(name): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut answer = if method == Method::OPTIONS {
        let asks = [
            (header::ACCESS_CONTROL_ALLOW_METHODS, "POST"),
            (header::ACCESS_CONTROL_ALLOW_HEADERS, "content-type"),
            (header::ACCESS_CONTROL_MAX_AGE, "3600"),
        ];
        (StatusCode::NO_CONTENT, asks).into_response()
    } else {
        hand(&entrance, &entrance.waits, (name, "allowed".into()), method, uri, headers, body).await
    };
    answer.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, header::HeaderValue::from_static(PAGE));
    answer
}

async fn called(
    State(entrance): State<Arc<Entrance>>,
    Path((name, path)): Path<(String, String)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    hand(&entrance, &entrance.at_once, (name, path), method, uri, headers, body).await
}

async fn hand(
    entrance: &Entrance,
    places: &Semaphore,
    (name, path): (String, String),
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if name.is_empty() || name.len() > 32 || !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
        return refused(StatusCode::NOT_FOUND, "Not found");
    }
    let Ok(_place) = places.try_acquire() else {
        return refused(StatusCode::SERVICE_UNAVAILABLE, "Try again later");
    };
    let form = match headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
        Some(kind) if kind.starts_with("application/x-www-form-urlencoded") => {
            let request = axum::http::Request::post("/").header(header::CONTENT_TYPE, kind).body(body.clone().into());
            let Ok(request) = request else { return refused(StatusCode::BAD_REQUEST, "Not a form") };
            match Form::<HashMap<String, String>>::from_request(request, &()).await {
                Ok(Form(form)) => form,
                Err(_) => return refused(StatusCode::BAD_REQUEST, "Not a form"),
            }
        }
        _ if method == Method::GET => Query::try_from_uri(&uri).map(|Query(form)| form).unwrap_or_default(),
        _ => HashMap::new(),
    };
    let Ok(body) = String::from_utf8(body.to_vec()) else { return refused(StatusCode::BAD_REQUEST, "Not text") };
    let headers = headers.iter().filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_owned()))).collect();
    let asked = Asked { method: method.to_string(), path, headers, body, form };
    let socket = entrance.sockets.join(format!("{name}.sock"));
    match tokio::time::timeout(TIME, hand_on(&socket, &asked)).await {
        Ok(Ok(Answer { status, body, location })) => {
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let headers = [
                (header::CONTENT_TYPE, "application/json; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
                (header::REFERRER_POLICY, "no-referrer"),
            ];
            let nothing = body.is_null() || body.as_object().is_some_and(|o| o.is_empty());
            let body = if nothing { String::new() } else { body.to_string() };
            let location = location.and_then(|to| header::HeaderValue::from_str(&to).ok());
            let mut answer = (status, headers, body).into_response();
            if let Some(to) = location {
                answer.headers_mut().insert(header::LOCATION, to);
            }
            answer
        }
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => refused(StatusCode::NOT_FOUND, "Not found"),
        _ => refused(StatusCode::SERVICE_UNAVAILABLE, "Try again later"),
    }
}

async fn hand_on(socket: &std::path::Path, asked: &Asked) -> std::io::Result<Answer> {
    let mut stream = UnixStream::connect(socket).await?;
    stream.write_all(format!("{}\n", json!(asked)).as_bytes()).await?;
    let mut line = String::new();
    BufReader::new(stream.take(MAX_LINE)).read_line(&mut line).await?;
    Ok(serde_json::from_str(&line)?)
}
