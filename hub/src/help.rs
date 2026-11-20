//! Help (spec 6.4): what a phone tells the operator about the app, by a random install id (`Yo-Install`), never by
//! the account and never signed: messages to the operator and their thread, reports of users, crash and error
//! reports. Messages are plain text to the hub, not end-to-end encrypted (spec 1).

use crate::db::Install;
use crate::error::{refuse, Res};
use crate::request::{bad, header, Hubs, Peer, DAY, HOUR};
use crate::{blobs, now_ms, Hub};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use bytes::Bytes;
use yo_wire::{api, decode, encode};

/// The largest error or crash report.
const MAX_DIAGNOSTIC: usize = 60 * 1024;

/// The request's install id: `Yo-Install`, 32 lowercase hex digits.
fn install(headers: &HeaderMap) -> Res<Install> {
    header(headers, api::HEADER_INSTALL).filter(|text| api::install_valid(text)).and_then(blobs::unhex).ok_or_else(bad)
}

/// A message or report: limited by its install id and by its address.
fn limited(hub: &Hub, kind: &str, peer: Peer, headers: &HeaderMap, per_install: u32) -> Res<Install> {
    let install = install(headers)?;
    hub.limit_ip("help", peer.0, headers, hub.config.limits.help_per_ip_day, DAY)?;
    hub.limit_by(kind, &install, per_install, DAY)?;
    Ok(install)
}

/// `POST /v1/help/report`: a user reported, for the operator to review; the reporter is the install id.
pub(crate) async fn post_report(State(hub): Hubs, peer: Peer, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let request: api::Report = decode(&body)?;
    if !(api::REPORT_SPAM..=api::REPORT_OTHER).contains(&request.reason) {
        return Err(bad());
    }
    let install = limited(&hub, "report", peer, &headers, hub.config.limits.report_per_install_day)?;
    if !hub.db.report(&install, &request.user_id, request.reason, now_ms())? {
        return Err(refuse(StatusCode::NOT_FOUND));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/help/messages`: a message to the operator. `429`: the day's are used up, or the thread is full.
pub(crate) async fn post_message(State(hub): Hubs, peer: Peer, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let request: api::Feedback = decode(&body)?;
    let (text, app_version) = request.check().map_err(|_| bad())?;
    let install = limited(&hub, "support", peer, &headers, hub.config.limits.support_per_install_day)?;
    hub.db.support_message(&install, text, app_version, now_ms())?.done()?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/help/messages`: the install's thread with the operator, empty before its first message.
pub(crate) async fn get_messages(State(hub): Hubs, peer: Peer, headers: HeaderMap) -> Res<Vec<u8>> {
    let install = install(&headers)?;
    hub.limit_ip("help-read", peer.0, &headers, 1200, HOUR)?;
    hub.limit_by("support-read", &install, 120, HOUR)?;
    Ok(encode(&hub.db.support_thread(&install)?))
}

/// `POST /v1/help/read`: the operator's answers read, up to an id.
pub(crate) async fn post_read(State(hub): Hubs, peer: Peer, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let install = install(&headers)?;
    let request: api::SupportRead = decode(&body)?;
    if request.through > i64::MAX as u64 {
        return Err(bad());
    }
    hub.limit_ip("help-read", peer.0, &headers, 1200, HOUR)?;
    hub.limit_by("support-mark", &install, 120, HOUR)?;
    hub.db.support_read(&install, true, request.through, now_ms())?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/help/error`: an app's error report.
pub(crate) async fn post_error(State(hub): Hubs, peer: Peer, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    diagnostic(&hub, "error", peer, &headers, &body)
}

/// `POST /v1/help/crash`: an app's crash report.
pub(crate) async fn post_crash(State(hub): Hubs, peer: Peer, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    diagnostic(&hub, "crash", peer, &headers, &body)
}

/// A crash or error report: a JSON object of strings from a fixed list, nothing else; kept with its install id, so
/// the operator counts installs, not reports.
fn diagnostic(hub: &Hub, kind: &str, peer: Peer, headers: &HeaderMap, body: &[u8]) -> Res<StatusCode> {
    let install = install(headers)?;
    hub.limit_ip("diagnostic", peer.0, headers, hub.config.limits.diagnostics_per_ip_hour, HOUR)?;
    hub.limit_by("diagnostic", &install, hub.config.limits.diagnostics_per_install_hour, HOUR)?;
    if body.len() > MAX_DIAGNOSTIC {
        return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
    }
    let payload: serde_json::Value = serde_json::from_slice(body).map_err(|_| bad())?;
    let fields = payload.as_object().ok_or_else(bad)?;
    const ALLOWED: [&str; 13] = [
        "uuid",
        "app",
        "os",
        "model",
        "domain",
        "code",
        "place",
        "exception",
        "frames",
        "termination",
        "details",
        "count",
        "severity",
    ];
    let text = |key: &str| payload[key].as_str();
    let filled = |keys: [&str; 3]| keys.iter().all(|key| text(key).is_some_and(|value| !value.is_empty()));
    // `count`: one report for that many of the same, counted on the phone; `severity` "expected": known to happen.
    let count_ok = text("count").is_none_or(|count| count.parse::<u32>().is_ok_and(|n| (1..=100_000).contains(&n)));
    let valid = fields.iter().all(|(key, value)| ALLOWED.contains(&key.as_str()) && value.is_string())
        && count_ok
        && payload.get("severity").is_none_or(|severity| severity == "expected")
        && text("uuid").is_none_or(is_uuid)
        && filled(["app", "os", "model"])
        && match kind {
            "error" => filled(["domain", "code", "place"]),
            _ => ["exception", "frames", "termination"].iter().all(|key| text(key).is_some()),
        };
    if !valid {
        return Err(bad());
    }
    hub.db.diagnostic(kind, &payload, Some(&install), now_ms())?;
    Ok(StatusCode::NO_CONTENT)
}

/// `8-4-4-4-12` hex digits.
fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}
