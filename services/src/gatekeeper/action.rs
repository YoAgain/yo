//! What a script asks for: a list of few actions, each exactly of a shape below. Anything else and the whole answer
//! is refused. No action says to whom: it is always the one the script was started for.

use base64::Engine;
use serde::de::{Error, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use super::Header;

const MAX_ACTIONS: usize = 4;
/// One answer to a Yo, and one reminder: no bot writes more than it was written to.
const MAX_SENDS: usize = 1;
/// The largest picture, as `yo send --photo` reads.
pub const MAX_PICTURE: usize = 512 * 1024;
/// The soonest reminder, and the furthest.
const MIN_AHEAD_S: i64 = 60;
const MAX_AHEAD_S: i64 = 400 * 86_400;
/// All waits of one answer.
const MAX_WAIT_MS: u64 = 5_000;
/// A reply's, or a call's, JSON.
const MAX_JSON: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `{"send": "yo"}`
    Yo,
    /// `{"send": "picture" | "video", "media": REF}`: what came, back.
    Back,
    /// `{"send": "picture", "jpeg": BASE64}`: one it drew.
    Drawn(Vec<u8>),
    /// `{"send": "picture", "file": NAME}`: one from the folder of its name.
    File(String),
    /// `{"send": "picture", "url": URL}`: one the other side named, fetched by the executor.
    Fetched(String),
    /// `{"send": "probe"}`: the round-trip probe, back.
    Probe,
    /// `{"wait_ms": N}`
    Wait(u64),
    /// `{"remind": UNIX_SECONDS}`
    Remind(i64),
    /// `{"forget": true}`
    Forget,
    /// `{"reply": JSON, "status": 200}`: what a caller gets back.
    Reply(u16, Value),
    /// `{"call": URL, "body": JSON}`: the other side of a connection is called.
    Call(String, Value),
}

/// An action as written. Read with no room for anything else, so an answer takes no more memory than its own size.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    send: Option<String>,
    media: Option<String>,
    jpeg: Option<String>,
    file: Option<String>,
    url: Option<String>,
    wait_ms: Option<u64>,
    remind: Option<i64>,
    forget: Option<bool>,
    reply: Option<Value>,
    status: Option<u16>,
    call: Option<String>,
    body: Option<Value>,
}

/// A list that stops being read at one action too many.
struct Answer(Vec<Written>);

impl<'de> Deserialize<'de> for Answer {
    fn deserialize<D: Deserializer<'de>>(from: D) -> Result<Answer, D::Error> {
        struct Few;
        impl<'de> Visitor<'de> for Few {
            type Value = Answer;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a list of actions")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut list: A) -> Result<Answer, A::Error> {
                let mut actions = Vec::new();
                while let Some(action) = list.next_element()? {
                    if actions.len() == MAX_ACTIONS {
                        return Err(A::Error::custom("too many actions"));
                    }
                    actions.push(action);
                }
                Ok(Answer(actions))
            }
        }
        from.deserialize_seq(Few)
    }
}

/// The event a script's answer is for.
pub struct Asked<'a> {
    pub kind: &'a str,
    pub media: Option<&'a str>,
    pub group: bool,
    pub remember_user: bool,
    pub now: i64,
    /// Whether the user it is about is connected.
    pub connected: bool,
    pub header: &'a Header,
}

impl Action {
    fn sends(&self) -> bool {
        matches!(
            self,
            Action::Yo | Action::Back | Action::Drawn(_) | Action::File(_) | Action::Fetched(_) | Action::Probe
        )
    }
}

/// The actions of an answer, or why it is refused.
pub fn parse(answer: &[u8], asked: &Asked) -> Result<Vec<Action>, &'static str> {
    let Answer(list) = serde_json::from_slice(answer).map_err(|_| "not a short list of actions")?;
    let actions = list.into_iter().map(|action| one(action, asked)).collect::<Result<Vec<_>, _>>()?;
    let count = |wanted: fn(&Action) -> bool| actions.iter().filter(|a| wanted(a)).count();
    let wait: u64 = actions.iter().map(|a| if let Action::Wait(ms) = a { *ms } else { 0 }).sum();
    match () {
        () if count(Action::sends) > MAX_SENDS => Err("too much to send"),
        () if count(|a| matches!(a, Action::Remind(_))) > 1 => Err("more than one reminder"),
        () if count(|a| matches!(a, Action::Reply(..))) > 1 => Err("more than one reply"),
        () if count(|a| matches!(a, Action::Call(..))) > 1 => Err("more than one call"),
        // A call of nobody has nobody to send to.
        () if asked.kind == "webhook" && !asked.connected && count(Action::sends) > 0 => Err("sent to nobody"),
        () if wait > MAX_WAIT_MS => Err("too long a wait"),
        // A probe is no Yo: it comes back, or nothing happens.
        () if asked.kind == "probe" && !matches!(actions[..], [] | [Action::Probe]) => Err("more than the probe"),
        () => Ok(actions),
    }
}

fn one(written: Written, asked: &Asked) -> Result<Action, &'static str> {
    let may_remember = asked.remember_user && !asked.group;
    let Written { send, media, jpeg, file, url, wait_ms, remind, forget, reply, status, call, body } = written;
    if let Some(url) = url {
        let alone = media.is_none() && jpeg.is_none() && file.is_none() && wait_ms.is_none() && remind.is_none();
        let alone =
            alone && forget.is_none() && reply.is_none() && status.is_none() && call.is_none() && body.is_none();
        let may = send.as_deref() == Some("picture") && alone && asked.kind == "webhook" && asked.connected;
        return (may && url.len() <= 2048).then_some(Action::Fetched(url)).ok_or("not a picture it may fetch");
    }
    let nothing = send.is_none()
        && media.is_none()
        && jpeg.is_none()
        && file.is_none()
        && wait_ms.is_none()
        && remind.is_none()
        && forget.is_none();
    let short = |json: &Value| (json.is_object() || json.is_array()) && json.to_string().len() <= MAX_JSON;
    match (reply, status, call, body) {
        (None, None, None, None) => {}
        (Some(json), status, None, None) if nothing && asked.kind == "webhook" && short(&json) => {
            let status = status.unwrap_or(200);
            return (200..600).contains(&status).then_some(Action::Reply(status, json)).ok_or("not a status");
        }
        (None, None, Some(url), Some(json)) if nothing && asked.connected && json.is_object() && short(&json) => {
            return asked.header.may_call(&url).then_some(Action::Call(url, json)).ok_or("not an address it may call");
        }
        _ => return Err("not a reply or a call it may make"),
    }
    match (send.as_deref(), media, jpeg, file, wait_ms, remind, forget) {
        (Some("yo"), None, None, None, None, None, None) => Ok(Action::Yo),
        (Some("probe"), None, None, None, None, None, None) if asked.kind == "probe" => Ok(Action::Probe),
        (Some(kind), Some(media), None, None, None, None, None) => {
            let came = kind == asked.kind && Some(media.as_str()) == asked.media;
            came.then_some(Action::Back).ok_or("not the media that came")
        }
        (Some("picture"), None, Some(text), None, None, None, None) => {
            decode(&text).map(Action::Drawn).ok_or("not a JPEG")
        }
        (Some("picture"), None, None, Some(name), None, None, None) => {
            file_name(&name).then_some(Action::File(name)).ok_or("not a file name")
        }
        (None, None, None, None, Some(ms), None, None) => {
            (ms <= MAX_WAIT_MS).then_some(Action::Wait(ms)).ok_or("too long a wait")
        }
        (None, None, None, None, None, Some(due), None)
            if may_remember && !matches!(asked.kind, "cron" | "webhook") =>
        {
            let times = asked.now + MIN_AHEAD_S..=asked.now + MAX_AHEAD_S;
            times.contains(&due).then_some(Action::Remind(due)).ok_or("not a time to come")
        }
        (None, None, None, None, None, None, Some(true)) if may_remember => Ok(Action::Forget),
        _ => Err("not an action it may ask for"),
    }
}

fn decode(text: &str) -> Option<Vec<u8>> {
    if text.len() > MAX_PICTURE * 4 / 3 + 4 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(text).ok()?;
    (bytes.len() <= MAX_PICTURE && bytes.starts_with(b"\xff\xd8\xff")).then_some(bytes)
}

fn file_name(name: &str) -> bool {
    let stem = name.strip_suffix(".jpg").unwrap_or_default();
    (1..=64).contains(&stem.len()) && stem.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    fn header() -> &'static Header {
        let text = "names = [\"A\"]\nabout = \"a\"\ntriggers = [\"yo\"]\n[bridge]\n\
                    return_url = \"https://a.com/b\"\ncall_urls = [\"https://a.com/\"]\nkey_header = \"X\"\nhelp_url = \"https://a.com/h\"\n";
        Box::leak(Box::new(Header::parse(text, "a").unwrap()))
    }

    fn asked(kind: &str) -> Asked<'_> {
        Asked { kind, media: None, group: false, remember_user: false, now: NOW, connected: false, header: header() }
    }

    fn all(answer: &str, asked: &Asked) -> Result<Vec<Action>, &'static str> {
        parse(answer.as_bytes(), asked)
    }

    #[test]
    fn what_it_may_ask_for() {
        assert_eq!(all(r#"[{"send": "yo"}]"#, &asked("yo")), Ok(vec![Action::Yo]));
        assert_eq!(all(r#"[{"send": "yo", "media": null}]"#, &asked("yo")), Ok(vec![Action::Yo]));
        assert_eq!(all("[]", &asked("yo")), Ok(vec![]));
        assert_eq!(all(r#"[{"wait_ms": 5000}]"#, &asked("yo")), Ok(vec![Action::Wait(5000)]));
        let media = Asked { media: Some("m1"), ..asked("video") };
        assert_eq!(all(r#"[{"send": "video", "media": "m1"}]"#, &media), Ok(vec![Action::Back]));
        let file = all(r#"[{"send": "picture", "file": "a-1.jpg"}]"#, &asked("yo"));
        assert_eq!(file, Ok(vec![Action::File("a-1.jpg".into())]));
        let jpeg = all(r#"[{"send": "picture", "jpeg": "/9j/4A=="}]"#, &asked("yo"));
        assert_eq!(jpeg, Ok(vec![Action::Drawn(vec![0xff, 0xd8, 0xff, 0xe0])]));
        assert_eq!(all(r#"[{"send": "probe"}]"#, &asked("probe")), Ok(vec![Action::Probe]));
        let keeps = Asked { remember_user: true, ..asked("yo") };
        let both = all(r#"[{"remind": 1000060}, {"forget": true}]"#, &keeps);
        assert_eq!(both, Ok(vec![Action::Remind(NOW + 60), Action::Forget]));
    }

    #[test]
    fn replies_and_calls() {
        let reply = r#"[{"send": "yo"}, {"reply": {"a": 1}, "status": 201}]"#;
        let (call, body) = (r#"[{"call": "https://a.com/x", "body": {}}]"#, serde_json::json!({}));
        let connected = |kind| Asked { connected: true, ..asked(kind) };
        let got = all(reply, &connected("webhook"));
        assert_eq!(got, Ok(vec![Action::Yo, Action::Reply(201, serde_json::json!({"a": 1}))]));
        assert!(all(reply, &connected("yo")).is_err() && all(reply, &asked("webhook")).is_err());
        assert_eq!(all(r#"[{"reply": []}]"#, &asked("webhook")), Ok(vec![Action::Reply(200, serde_json::json!([]))]));
        assert_eq!(all(call, &connected("yo")), Ok(vec![Action::Call("https://a.com/x".into(), body)]));
        assert!(all(call, &asked("yo")).is_err(), "nobody connected");
        for bad in [
            r#"[{"call": "https://b.com/x", "body": {}}]"#,
            r#"[{"call": "https://a.com/x"}]"#,
            r#"[{"call": "https://a.com/x", "body": 1}]"#,
            r#"[{"call": "https://a.com/x", "body": {}, "send": "yo"}]"#,
            r#"[{"reply": "x"}]"#,
            r#"[{"reply": {}, "status": 99}]"#,
            r#"[{"reply": {}}, {"reply": {}}]"#,
            r#"[{"status": 200}]"#,
            r#"[{"remind": 1000060}]"#,
        ] {
            assert!(all(bad, &Asked { remember_user: true, ..connected("webhook") }).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_picture_is_fetched_for_a_call_alone() {
        let answer = r#"[{"send": "picture", "url": "https://a.com/x.jpg"}]"#;
        let connected = |kind| Asked { connected: true, ..asked(kind) };
        assert_eq!(all(answer, &connected("webhook")), Ok(vec![Action::Fetched("https://a.com/x.jpg".into())]));
        assert!(all(answer, &connected("yo")).is_err() && all(answer, &asked("webhook")).is_err());
        assert!(all(r#"[{"send": "yo", "url": "https://a.com/x.jpg"}]"#, &connected("webhook")).is_err());
    }

    #[test]
    fn anything_else_refuses_the_whole_answer() {
        let media = Asked { media: Some("m1"), ..asked("picture") };
        for bad in [
            "",
            "null",
            "{}",
            "[[]]",
            "[1]",
            "[{}]",
            r#"[{"send": "yo"}] x"#,
            r#"[{"send": "yo", "to": "9f2c"}]"#,
            r#"[{"send": "root"}]"#,
            r#"[{"send": ["yo"]}]"#,
            r#"[{"send": "yo"}, {"send": "x"}]"#,
            r#"[{"send": "yo"}, {"send": "yo"}]"#,
            r#"[{"send": "video", "media": "m1"}]"#,
            r#"[{"send": "picture", "media": "m2"}]"#,
            r#"[{"send": "picture", "file": "../a.jpg"}]"#,
            r#"[{"send": "picture", "file": "/a.jpg"}]"#,
            r#"[{"send": "picture", "file": ".jpg"}]"#,
            r#"[{"send": "picture", "file": "a.jpg\u0000"}]"#,
            r#"[{"send": "picture", "file": "a.png"}]"#,
            r#"[{"send": "picture", "jpeg": "PHN2Zz4="}]"#,
            r#"[{"send": "picture", "jpeg": "%%"}]"#,
            r#"[{"send": "picture"}]"#,
            r#"[{"send": "probe"}]"#,
            r#"[{"wait_ms": 5001}]"#,
            r#"[{"wait_ms": 3000}, {"wait_ms": 3000}]"#,
            r#"[{"send": "yo", "send": "yo"}]"#,
            r#"[{"send": "yo", "wait_ms": 1}]"#,
            r#"[{"wait_ms": -1}]"#,
            r#"[{"remind": 1000060}]"#,
            r#"[{"forget": true}]"#,
        ] {
            assert!(all(bad, &media).is_err(), "{bad}");
        }
        assert!(parse(b"\xff\xfe", &media).is_err());
        assert!(all(&"[".repeat(500_000), &media).is_err());
        let keeps = Asked { remember_user: true, ..asked("yo") };
        assert!(all(r#"[{"remind": 1000060}, {"remind": 1000070}]"#, &keeps).is_err());
        for bad in ["999999", "1000059", "99999999999", "1.5", "\"x\"", "9223372036854775808"] {
            assert!(all(&format!(r#"[{{"remind": {bad}}}]"#), &keeps).is_err(), "{bad}");
        }
        let cron = Asked { kind: "cron", remember_user: true, ..asked("yo") };
        assert!(all(r#"[{"send": "yo"}]"#, &cron).is_ok() && all(r#"[{"remind": 1000060}]"#, &cron).is_err());
        let group = Asked { group: true, ..keeps };
        assert!(all(r#"[{"remind": 1000060}]"#, &group).is_err(), "nobody is remembered out of a group");
        assert!(all(r#"[{"forget": true}]"#, &group).is_err(), "nor forgotten");
        let probe = Asked { remember_user: true, ..asked("probe") };
        for bad in [r#"[{"send": "yo"}]"#, r#"[{"remind": 1000060}]"#, r#"[{"send": "probe"}, {"send": "probe"}]"#] {
            assert!(all(bad, &probe).is_err(), "{bad}");
        }
        assert!(all(&format!("[{}]", ["{}"; 5].join(",")), &media).is_err());
    }
}
