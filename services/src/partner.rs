//! The other side of a service's connections, as the entrance hands it on (`entrance.rs`): OAuth 2 for whoever
//! connects, calls of it for the script to answer, and calls to it. Its keys are here alone, the script sees none.
//!
//!   POST token   a connect link's code becomes a key (RFC 6749, 4.1.3)
//!   *    …       with a key, for its user; or with the partner's own, for nobody

use crate::entrance::{Answer, Asked, MAX_LINE};
use crate::executor::{now, Executor};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const AT_ONCE: usize = 16;
const CALL_TIME: Duration = Duration::from_secs(10);
/// Going back from the connect page may be quicker than the app's message.
const CODE_WAIT: Duration = Duration::from_secs(8);

/// Its settings: from the environment (`main.rs`).
pub struct Partner {
    /// The header with its own key, and the key: checked on its calls, carried by ours, and the OAuth 2 client's secret.
    pub header: String,
    pub return_url: String,
    pub key: String,
    pub client_id: String,
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

fn one(executor: &Arc<Executor>, mut stream: UnixStream) -> io::Result<()> {
    stream.set_read_timeout(Some(CALL_TIME))?;
    stream.set_write_timeout(Some(CALL_TIME))?;
    let mut line = String::new();
    BufReader::new((&stream).take(MAX_LINE)).read_line(&mut line)?;
    let asked: Asked = serde_json::from_str(&line)?;
    let Some(partner) = &executor.partner else { return Ok(()) };
    let answer = match asked.path.as_str() {
        "token" if asked.method == "POST" => token(executor, partner, &asked),
        "token" => refused(405, "POST only"),
        _ => called(executor, partner, asked),
    };
    stream.write_all(format!("{}\n", json!(answer)).as_bytes())
}

fn refused(status: u16, why: &str) -> Answer {
    Answer { status, body: json!({ "errors": [{ "message": why }] }) }
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
    match executor.called(Call { id, method: asked.method, path: asked.path, body, keyed }, user) {
        Ok((status, body)) => Answer { status, body },
        Err(status) => refused(status, "Failed"),
    }
}

/// The code of a connect link becomes a key, for the partner alone.
fn token(executor: &Arc<Executor>, partner: &Partner, asked: &Asked) -> Answer {
    let wrong = |status: u16, why: &str| Answer { status, body: json!({ "error": why }) };
    let field = |name: &str| asked.form.get(name).map(String::as_str).unwrap_or_default();
    if !same(field("client_id"), &partner.client_id) | !same(field("client_secret"), &partner.key) {
        return wrong(401, "invalid_client");
    }
    if field("grant_type") != "authorization_code" {
        return wrong(400, "unsupported_grant_type");
    }
    if field("redirect_uri") != partner.return_url {
        return wrong(400, "invalid_grant");
    }
    let (Some(key), Some(out)) = (random(), random()) else { return wrong(500, "server_error") };
    // The app told the hash of the code (the connect page), and that is kept hashed.
    let told = URL_SAFE_NO_PAD.encode(hash(field("code")));
    let code = hash(&told);
    let end = Instant::now() + CODE_WAIT;
    loop {
        match executor.book.exchange(&code, &hash(&key), &out, now()) {
            Ok(true) => {
                if let Ok(Some((user, _))) = executor.book.connected_by(&hash(&key)) {
                    executor.tell(&user, "connected", &told);
                }
                return Answer { status: 200, body: json!({ "access_token": key, "token_type": "Bearer" }) };
            }
            Ok(false) if Instant::now() < end => std::thread::sleep(Duration::from_millis(250)),
            Ok(false) => return wrong(400, "invalid_grant"),
            Err(_) => return wrong(500, "server_error"),
        }
    }
}
