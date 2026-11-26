//! The other side of a service's connections, as the entrance hands it on (`entrance.rs`): OAuth 2 for whoever
//! connects, calls of it for the script to answer, and calls to it. Its keys are here alone, the script sees none.
//!
//!   GET  connect  a request to connect (RFC 6749, 4.1.1): on to the connect page
//!   POST allowed  what became of it, for that page: where it goes back to
//!   POST token    the code it brought there becomes a key (4.1.3)
//!   *    …        with a key, for its user; or with the partner's own, for nobody

use crate::book::Request;
use crate::entrance::{Answer, Asked, MAX_LINE};
use crate::executor::{now, Executor};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const AT_ONCE: usize = 16;
const CALL_TIME: Duration = Duration::from_secs(10);
/// The connect page; the request is told in the fragment, which no server sees.
const PAGE: &str = "https://justyo.co/services/";
const MAX_STATE: usize = 512;

/// Few of `AT_ONCE`: pages that ask take no place from the calls. The entrance does the waiting.
const WAITS: usize = 8;
static WAITING: AtomicUsize = AtomicUsize::new(0);

/// Its settings: from the environment (`main.rs`).
pub struct Partner {
    /// The header with its own key, and the key: checked on its calls, carried by ours.
    pub header: String,
    pub return_url: String,
    pub key: String,
    pub client_id: String,
    pub client_secret: String,
    /// The account its tests run with, and a key for it.
    pub test: Option<(String, String)>,
    pub client: reqwest::blocking::Client,
}

/// A call of the other side.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub id: String,
    pub method: String,
    pub path: String,
    pub body: Value,
    /// With the partner's own key.
    pub keyed: bool,
    /// When who called has given up (Unix, ms).
    pub until_ms: i64,
}

pub fn hash(text: &str) -> [u8; 32] {
    Sha256::digest(text).into()
}

/// Compared by their hashes: how long it takes tells nothing of the secret.
fn same(a: &str, b: &str) -> bool {
    hash(a) == hash(b)
}

fn random() -> Option<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(URL_SAFE_NO_PAD.encode(bytes))
}

impl Partner {
    pub fn client() -> reqwest::Result<reqwest::blocking::Client> {
        let client =
            reqwest::blocking::Client::builder().timeout(CALL_TIME).redirect(reqwest::redirect::Policy::none());
        client.build()
    }

    pub fn call(&self, url: &str, body: &Value) -> bool {
        let sent = self.client.post(url).header(&self.header, &self.key).json(body).send();
        sent.is_ok_and(|answer| answer.status().is_success())
    }

    /// What the test account is known by.
    pub fn test_out(&self, user: &str) -> Option<String> {
        self.test.as_ref().filter(|(test, _)| !user.is_empty() && test == user).map(|_| "test".into())
    }

    pub fn test_key(&self) -> Option<&str> {
        self.test.as_ref().map(|(_, key)| key.as_str())
    }
}

/// Serves what the entrance hands on, a thread each, until the process ends.
pub fn serve(executor: Arc<Executor>, listener: UnixListener) {
    let at_once = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming().flatten() {
        if at_once.fetch_add(1, Ordering::SeqCst) >= AT_ONCE {
            at_once.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let (executor, at_once) = (executor.clone(), at_once.clone());
        std::thread::spawn(move || {
            let _ = one(&executor, stream);
            at_once.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

/// Its own socket, for the entrance: what comes is checked, whoever it is from.
pub fn listen(path: &Path) -> io::Result<UnixListener> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))?;
    Ok(listener)
}

fn one(executor: &Executor, mut stream: UnixStream) -> io::Result<()> {
    stream.set_read_timeout(Some(CALL_TIME))?;
    stream.set_write_timeout(Some(CALL_TIME))?;
    let mut line = String::new();
    BufReader::new((&stream).take(MAX_LINE)).read_line(&mut line)?;
    let asked: Asked = serde_json::from_str(&line)?;
    let Some(partner) = &executor.partner else { return Ok(()) };
    let answer = match asked.path.as_str() {
        "token" if asked.method == "POST" => token(executor, partner, &asked),
        "token" => refused(405, "POST only"),
        "connect" if asked.method == "GET" => connect(executor, partner, &asked),
        "connect" => refused(405, "GET only"),
        "allowed" if asked.method == "POST" => allowed(executor, partner, &asked),
        "allowed" => refused(405, "POST only"),
        _ => called(executor, partner, asked),
    };
    stream.write_all(format!("{}\n", json!(answer)).as_bytes())
}

fn refused(status: u16, why: &str) -> Answer {
    Answer::new(status, json!({ "errors": [{ "message": why }] }))
}

/// The other side sent someone here: a request, and on to the connect page. Nothing of the link is gone to.
fn connect(executor: &Executor, partner: &Partner, asked: &Asked) -> Answer {
    let field = |name: &str| asked.form.get(name).map(String::as_str).unwrap_or_default();
    let state = field("state");
    if field("client_id") != partner.client_id || field("redirect_uri") != partner.return_url {
        return refused(400, "Unknown client_id or redirect_uri");
    }
    if field("response_type") != "code" || state.is_empty() || state.len() > MAX_STATE {
        return refused(400, "No response_type=code, or no state");
    }
    let (Some(link), Some(poll)) = (random(), random()) else { return refused(500, "Failed") };
    if !executor.book.request(&link, &hash(&poll), state, now()).unwrap_or(false) {
        return refused(503, "Try again later");
    }
    let page = format!("{PAGE}{}#code={link}&poll={poll}", executor.header.names[0].to_lowercase());
    Answer { status: 303, body: Value::Null, location: Some(page) }
}

/// Without a key: the connect page asks with what only it was told.
fn allowed(executor: &Executor, partner: &Partner, asked: &Asked) -> Answer {
    let body: Value = serde_json::from_str(&asked.body).unwrap_or_default();
    let Some(poll) = body["poll"].as_str().filter(|poll| poll.len() == 43) else {
        return refused(400, "No poll");
    };
    if WAITING.fetch_add(1, Ordering::SeqCst) >= WAITS {
        WAITING.fetch_sub(1, Ordering::SeqCst);
        return refused(503, "Try again later");
    }
    let request = executor.book.asked(poll, now());
    WAITING.fetch_sub(1, Ordering::SeqCst);
    let said = match request {
        Ok(Request::Waits) => json!({ "request": "waiting" }),
        Ok(Request::Over) => json!({ "request": "over" }),
        Ok(Request::Allowed(code, state)) => back(partner, "allowed", [("code", code), ("state", state)]),
        Ok(Request::Declined(state)) => {
            back(partner, "declined", [("error", "access_denied".into()), ("state", state)])
        }
        Err(_) => return refused(503, "Try again later"),
    };
    Answer::new(200, said)
}

fn called(executor: &Executor, partner: &Partner, asked: Asked) -> Answer {
    let text = |name: &str| asked.headers.get(name).map(String::as_str);
    let keyed = text(&partner.header).is_some_and(|key| same(key, &partner.key));
    let user = match text("authorization") {
        Some(bearer) => {
            let key = bearer.strip_prefix("Bearer ").unwrap_or_default();
            let test = partner.test.as_ref().filter(|(_, test)| same(test, key)).map(|(user, _)| user.clone());
            match test.or_else(|| executor.book.connected_by(&hash(key)).ok().flatten().map(|(user, _)| user)) {
                Some(user) => Some(user),
                None => return refused(401, "Not connected"),
            }
        }
        None if keyed => None,
        None => return refused(401, "No key"),
    };
    let body = match serde_json::from_str(&asked.body) {
        Ok(Value::Object(body)) => Value::Object(body),
        _ if asked.body.is_empty() => json!({}),
        _ => return refused(400, "Not a JSON object"),
    };
    let Some(id) = random() else { return refused(500, "Failed") };
    match executor
        .called(Call { id, method: asked.method, path: asked.path, body, keyed, until_ms: asked.until_ms }, user)
    {
        Ok((status, body)) => Answer::new(status, body),
        // Not to be tried again.
        Err(400) => Answer::new(400, json!({ "errors": [{ "status": "SKIP", "message": "Failed" }] })),
        Err(status) => refused(status, "Failed"),
    }
}

/// Where the page goes back to.
fn back(partner: &Partner, request: &str, with: [(&str, String); 2]) -> Value {
    let url = reqwest::Url::parse_with_params(&partner.return_url, with).map(String::from).unwrap_or_default();
    json!({ "request": request, "url": url })
}

/// The code of a request becomes a key, for the partner alone.
fn token(executor: &Executor, partner: &Partner, asked: &Asked) -> Answer {
    let wrong = |status: u16, why: &str| Answer::new(status, json!({ "error": why }));
    let field = |name: &str| asked.form.get(name).map(String::as_str).unwrap_or_default();
    let basic = asked.headers.get("authorization").and_then(|value| basic(value));
    let (id, secret) = basic.unwrap_or_else(|| (field("client_id").into(), field("client_secret").into()));
    if !same(&id, &partner.client_id) | !same(&secret, &partner.client_secret) {
        return wrong(401, "invalid_client");
    }
    if field("grant_type") != "authorization_code" {
        return wrong(400, "unsupported_grant_type");
    }
    if field("redirect_uri") != partner.return_url {
        return wrong(400, "invalid_grant");
    }
    let Some(key) = random() else { return wrong(500, "server_error") };
    match executor.book.exchange(field("code"), &hash(&key), now()) {
        Ok(Some((user, link))) => {
            executor.tell(&user, "connected", &link);
            Answer::new(200, json!({ "access_token": key, "token_type": "Bearer" }))
        }
        Ok(None) => wrong(400, "invalid_grant"),
        Err(_) => wrong(500, "server_error"),
    }
}

/// The client's id and secret as a header (RFC 6749, 2.3.1); ours need no escapes.
fn basic(value: &str) -> Option<(String, String)> {
    let (scheme, text) = value.split_once(' ')?;
    let text = scheme.eq_ignore_ascii_case("basic").then(|| STANDARD.decode(text.trim()).ok()).flatten()?;
    let (id, secret) = std::str::from_utf8(&text).ok()?.split_once(':')?;
    Some((id.into(), secret.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_says_who_it_is_in_a_header() {
        let of = |text: &str| basic(&format!("basic {}", STANDARD.encode(text)));
        assert_eq!(of("ifttt:a:b"), Some(("ifttt".into(), "a:b".into())));
        assert!(of("ifttt").is_none() && basic("Bearer aTpz").is_none() && basic("Basic !").is_none());
    }
}
