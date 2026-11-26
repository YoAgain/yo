//! Help (spec 4): the support thread, reports of users, crash and error reports. Each goes by a random install id
//! (`api::HEADER_INSTALL`), made and kept on the phone alone, never by the account and never signed: the hub cannot
//! tie any of it to an account. Losing the id loses the thread.

use crate::{expect, refused, Error, Result, Transport};
use yoip::net::api::{self, status};
use yoip::{decode, encode, UserId};

/// A new install id: 128 random bits as 32 lowercase hex digits.
pub fn new_install_id() -> String {
    crate::state::hex(&crate::identity::random::<16>())
}

/// What a crash or error report is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Diagnostic {
    Crash,
    Error,
}

/// One help request: `Protocol` for an install id that is not one, `Network` without an answer.
fn send(transport: &dyn Transport, install: &str, method: &str, path: &str, body: &[u8]) -> Result<crate::Response> {
    if !api::install_valid(install) {
        return Err(Error::Protocol("install id"));
    }
    let headers = [(api::HEADER_INSTALL.to_owned(), install.to_owned())];
    transport.request(method, path, &headers, body).map_err(Error::Network)
}

/// A message to the operator, readable by the hub (spec 4). Empty or too long text is refused here; `Refused(429)`:
/// the day's are used up, or the thread is full.
pub fn send_message(transport: &dyn Transport, install: &str, text: &str, app_version: &str) -> Result<()> {
    let feedback = api::Feedback { text: text.as_bytes().into(), app_version: app_version.as_bytes().into() };
    feedback.check().map_err(|_| Error::Protocol("feedback empty or too long"))?;
    expect(send(transport, install, "POST", api::HELP_MESSAGES, &encode(&feedback))?, status::NO_CONTENT).map(drop)
}

/// The install's thread with the operator; none written yet is an empty one.
pub fn thread(transport: &dyn Transport, install: &str) -> Result<api::SupportThread> {
    Ok(decode(&expect(send(transport, install, "GET", api::HELP_MESSAGES, &[])?, status::OK)?.body)?)
}

/// The operator's answers read, through the id shown last.
pub fn read(transport: &dyn Transport, install: &str, through: u64) -> Result<()> {
    let body = encode(&api::SupportRead { through });
    expect(send(transport, install, "POST", api::HELP_READ, &body)?, status::NO_CONTENT).map(drop)
}

/// A user reported (`api::REPORT_*`). `NotOnYo`: no such user.
pub fn report(transport: &dyn Transport, install: &str, user: UserId, reason: u8) -> Result<()> {
    let body = encode(&api::Report { user_id: user, reason });
    let response = send(transport, install, "POST", api::HELP_REPORT, &body)?;
    match response.status {
        status::NO_CONTENT => Ok(()),
        status::NOT_FOUND => Err(Error::NotOnYo),
        _ => Err(refused(&response)),
    }
}

/// A crash or error report, a JSON object of strings (README.md, Telemetry), as the app made it: the hub's
/// status, so the app keeps what it should send again (5xx, 429) and drops what the hub will never take (4xx). A
/// crash still queued as plain text by the first builds goes as the same report in JSON (`legacy_crash`).
pub fn send_diagnostic(transport: &dyn Transport, install: &str, kind: Diagnostic, report: &[u8]) -> Result<u16> {
    let path = match kind {
        Diagnostic::Crash => api::HELP_CRASH,
        Diagnostic::Error => api::HELP_ERROR,
    };
    let legacy;
    let report = match kind == Diagnostic::Crash && !report.trim_ascii_start().starts_with(b"{") {
        true => {
            legacy = legacy_crash(&String::from_utf8_lossy(report));
            legacy.as_bytes()
        }
        false => report,
    };
    Ok(send(transport, install, "POST", path, report)?.status)
}

/// The first builds' plain-text crash ("app · os · model", the exception, then frames) as the hub's JSON; its uuid
/// is the text's hash, so a resend counts once.
fn legacy_crash(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let meta: Vec<&str> = lines.first().unwrap_or(&"").split(" · ").collect();
    let field = |i: usize| meta.get(i).map(|v| v.trim()).filter(|v| !v.is_empty()).unwrap_or("?");
    let hash = crate::state::hex(&yoip::sha256(text.as_bytes())[..16]);
    let uuid = format!("{}-{}-{}-{}-{}", &hash[..8], &hash[8..12], &hash[12..16], &hash[16..20], &hash[20..32]);
    let frames: String = lines.iter().skip(2).copied().collect::<Vec<_>>().join("\n").chars().take(40_000).collect();
    serde_json::json!({"uuid": uuid, "app": field(0), "os": field(1), "model": field(2),
        "exception": lines.get(1).copied().unwrap_or(""), "frames": frames, "termination": ""})
    .to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_plain_text_crash_becomes_the_hubs_json() {
        let json: serde_json::Value =
            serde_json::from_str(&super::legacy_crash("ios/12 · 26.1 · iPhone17,1\nSIGSEGV\nYoApp +0x1\nYoApp +0x2"))
                .unwrap();
        assert_eq!((json["app"].as_str(), json["model"].as_str()), (Some("ios/12"), Some("iPhone17,1")));
        assert_eq!(
            (json["exception"].as_str(), json["frames"].as_str()),
            (Some("SIGSEGV"), Some("YoApp +0x1\nYoApp +0x2"))
        );
        assert_eq!(json["uuid"].as_str().unwrap().len(), 36);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&super::legacy_crash("x")).unwrap()["os"], "?");
    }
}
