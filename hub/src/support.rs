//! What users tell the operator (spec 6.3, 6.4): reports of users, feedback and its thread, crash and error reports.
//! Feedback is plain text to the hub, not end-to-end encrypted (spec 1).

use crate::error::{refuse, Res};
use crate::request::{bad, Auth, Hubs, DAY, HOUR};
use crate::{now_ms, Hub};
use axum::extract::State;
use axum::http::StatusCode;
use bytes::Bytes;
use yo_wire::{api, decode, encode};

/// The largest error or crash report.
const MAX_DIAGNOSTIC: usize = 60 * 1024;

/// `POST /v1/report`: a user reported by the signing device's user, for the operator to review.
pub(crate) async fn post_report(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let request: api::Report = decode(&body)?;
    if !(api::REPORT_SPAM..=api::REPORT_OTHER).contains(&request.reason) || request.user_id == device.user_id {
        return Err(bad());
    }
    hub.limit_by("report", &device.user_id, hub.config.limits.report_per_user_day, DAY)?;
    if !hub.db.report(&device.device_id, &request.user_id, request.reason, now_ms())?.done()? {
        return Err(refuse(StatusCode::NOT_FOUND));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/feedback`, `POST /v1/support`: a message to the operator.
pub(crate) async fn post_feedback(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let request: api::Feedback = decode(&body)?;
    let (text, app_version) = request.check().map_err(|_| bad())?;
    hub.limit_by("feedback", &device.user_id, hub.config.limits.feedback_per_user_day, DAY)?;
    hub.db.feedback(&device.device_id, text, app_version, now_ms())?.done()?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/support`: the user's thread with the operator.
pub(crate) async fn get_support(State(hub): Hubs, Auth(device, _): Auth) -> Res<Vec<u8>> {
    hub.limit_by("support-read", &device.user_id, 120, HOUR)?;
    Ok(encode(&hub.db.support_thread(&device.user_id)?))
}

/// `POST /v1/support/read`: the operator's answers read, up to an id.
pub(crate) async fn post_support_read(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let request: api::SupportRead = decode(&body)?;
    if request.through > i64::MAX as u64 {
        return Err(bad());
    }
    hub.limit_by("support-mark", &device.user_id, 120, HOUR)?;
    hub.db.support_read(&device.user_id, true, request.through, now_ms())?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/crash` (spec 6.4): an old app's crash report, unsigned, no account.
pub(crate) async fn post_crash(State(hub): Hubs, body: Bytes) -> Res<StatusCode> {
    let request: api::Feedback = decode(&body)?;
    let (text, app_version) = request.check().map_err(|_| bad())?;
    hub.db.crash(text, app_version, now_ms())?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/errors`: an app's error report.
pub(crate) async fn post_error(State(hub): Hubs, body: Bytes) -> Res<StatusCode> {
    diagnostic(&hub, "error", &body)
}

/// `POST /v1/diagnostics/crash`: an app's crash report.
pub(crate) async fn post_crash_report(State(hub): Hubs, body: Bytes) -> Res<StatusCode> {
    diagnostic(&hub, "crash", &body)
}

/// An unsigned report: a JSON object of strings from a fixed list, nothing else.
fn diagnostic(hub: &Hub, kind: &str, body: &[u8]) -> Res<StatusCode> {
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
    hub.db.diagnostic(kind, &payload, now_ms())?;
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
