//! The commands, on a client that is either this process's or a listener's (`Context::listening`).

use crate::{jpeg, Failure};
use serde_json::{json, Value};
use std::sync::{Mutex, MutexGuard};
use yo_core::media::{self, Media};
use yo_core::shared::{self, Locked, Target, GROUP_PREFIX as GROUP};
use yo_core::{Client, Content, Event};

/// The largest photo read for `send --photo`, as a reply picture may be.
const MAX_PHOTO: usize = 512 * 1024;

pub struct Context<'a> {
    pub client: &'a Mutex<Client>,
    pub config: &'a crate::Config,
    pub listening: bool,
    /// Events a command received on the way: a listener prints them as it prints its own.
    pub emit: &'a (dyn Fn(Vec<Event>) + Sync),
}

impl Context<'_> {
    pub fn client(&self) -> MutexGuard<'_, Client> {
        self.client.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The core's steps that give the client back while a request travels: a listener goes on receiving.
impl Locked for Context<'_> {
    type Error = Failure;

    fn step<T>(&self, step: impl FnOnce(&mut Client) -> T) -> Result<T, Failure> {
        Ok(step(&mut self.client()))
    }
}

type Outcome = Result<Value, Failure>;

pub fn run(context: &Context, command: &[String]) -> Outcome {
    let args: Vec<&str> = command.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["whoami"] => whoami(context, false),
        ["status"] => whoami(context, true),
        ["friends"] => Ok(friends(&context.client())),
        ["contacts"] => Ok(contacts(&context.client())),
        ["whois", name] => whois(context, name).map(|(user, username)| json!({ "user": user, "username": username })),
        ["add", name] => add(context, name),
        ["send", to, rest @ ..] => send(context, to, rest),
        ["groups"] | ["groups", "list"] => Ok(groups(&context.client())),
        ["groups", "create", name, members @ ..] if !members.is_empty() => {
            let members = users(context, members)?;
            let (id, events) = context.client().create_group(name, &members)?;
            (context.emit)(events);
            Ok(json!({ "group": id }))
        }
        ["groups", "leave", id] => done(context, |c| c.leave_group(id)),
        ["groups", "accept", id] => done(context, |c| c.accept_group(id)),
        ["groups", "rename", id, name] => done(context, |c| c.rename_group(id, name)),
        ["groups", "add", id, members @ ..] if !members.is_empty() => {
            let members = users(context, members)?;
            done(context, |c| c.add_to_group(id, &members))
        }
        ["groups", "remove", id, members @ ..] if !members.is_empty() => {
            let members = users(context, members)?;
            done(context, |c| c.remove_from_group(id, &members))
        }
        ["sync", rest @ ..] if !context.listening => sync(context, rest),
        ["sync", ..] => Err(Failure::new("a listener has the account: its output is the events")),
        ["ack"] => {
            context.client().acknowledge_only(|_| true);
            Ok(json!({ "acked": true }))
        }
        ["echo-probe", user, token] => {
            let token = yo_wire::unb64(token).ok().and_then(|t| t.try_into().ok());
            let token = token.ok_or_else(|| Failure::usage("echo-probe USER TOKEN: a token of 32 bytes"))?;
            done(context, |c| c.echo_probe(user, token))
        }
        ["probe", rest @ ..] => {
            let timeout_ms = match rest {
                [] => 10_000,
                ["--timeout-ms", ms] => ms.parse().map_err(|_| Failure::usage("--timeout-ms: a number"))?,
                _ => return Err(Failure::usage("probe [--timeout-ms N]")),
            };
            let ms = context.client().round_trip_probe("PINGTEST", timeout_ms)?;
            Ok(json!({ "ms": ms }))
        }
        _ => Err(Failure::usage(crate::HELP)),
    }
}

/// An operation that gives events and nothing else.
fn done(context: &Context, f: impl FnOnce(&mut Client) -> yo_core::Result<Vec<Event>>) -> Outcome {
    let events = f(&mut context.client())?;
    (context.emit)(events);
    Ok(json!({ "done": true }))
}

pub fn whoami(context: &Context, status: bool) -> Outcome {
    let client = context.client();
    let mut out = json!({
        "user": client.user_id(),
        "username": client.username(),
        "device": client.device_id(),
        "hub": context.config.hub,
    });
    if status {
        out["listening"] = json!(context.listening);
        out["contacts"] = json!(client.contacts().len());
        out["friends"] = json!(client.friends().len());
        out["groups"] = json!(client.groups().iter().filter(|g| !g.left).count());
        out["waiting"] = json!(client.waiting());
        out["dropped"] = json!(client.dropped());
    }
    Ok(out)
}

fn friends(client: &Client) -> Value {
    let contacts = client.contacts();
    let handles = |user: &str| contacts.iter().find(|c| c.user == user).map(|c| c.handles.clone()).unwrap_or_default();
    let list: Vec<Value> = client
        .friends()
        .into_iter()
        .map(|f| json!({ "user": f.user, "name": f.name, "handles": handles(&f.user), "blocked": f.blocked }))
        .collect();
    json!(list)
}

fn contacts(client: &Client) -> Value {
    let list: Vec<Value> = client
        .contacts()
        .into_iter()
        .map(|c| json!({ "user": c.user, "handles": c.handles, "name": c.name, "blocked": c.blocked, "gone": c.gone }))
        .collect();
    json!(list)
}

fn groups(client: &Client) -> Value {
    let list: Vec<Value> = client
        .groups()
        .into_iter()
        .map(|g| json!({ "id": g.id, "name": g.name, "members": g.members, "left": g.left, "accepted": g.accepted }))
        .collect();
    json!(list)
}

/// Who a name is: signed under the client, asked without it.
fn whois(context: &Context, name: &str) -> Result<(String, Option<String>), Failure> {
    let lookup = context.client().whois_begin(name)?;
    Ok(lookup.send()?)
}

/// Members by user id: as they are, usernames looked up.
fn users(context: &Context, names: &[&str]) -> Result<Vec<String>, Failure> {
    names.iter().map(|name| shared::user_for(context, name.trim_start_matches('@'))).collect()
}

fn add(context: &Context, name: &str) -> Outcome {
    let name = name.trim_start_matches('@');
    let (user, username) = whois(context, name)?;
    let mut client = context.client();
    client.whois_end(name, &user, username.clone())?;
    client.set_friend(&user, username.clone(), None)?;
    client.write_storage()?;
    Ok(json!({ "user": user, "username": username }))
}

fn sync(context: &Context, args: &[&str]) -> Outcome {
    let wait = match args {
        [] => 0,
        ["--wait", s] => s.parse().map_err(|_| Failure::usage("--wait: seconds"))?,
        _ => return Err(Failure::usage("sync [--wait S]")),
    };
    let mut client = context.client();
    let events = client.sync(wait)?;
    let mut out: Vec<Value> = events.iter().map(|e| event(e, true)).collect();
    out.extend(client.take_probe_requests().into_iter().map(|(user, token)| probe(&user, &token)));
    let _ = client.after_sync();
    client.acknowledge_only(|e| events.contains(e));
    Ok(json!(out))
}

/// A probe request for an echo service: answered with `echo-probe`.
pub fn probe(user: &str, token: &[u8; 32]) -> Value {
    json!({ "type": "probe", "from": user, "token": yo_wire::b64(token) })
}

/// An event as JSON. A photo's or video's `media.ref` (with `refs`) is what `send --media` takes: its blob's id and
/// key, so whoever holds it can open the photo.
pub fn event(event: &Event, refs: bool) -> Value {
    let message = |user: &str, group: Option<&str>, sent_at_ms: u64, content: &Content| {
        let reply_to = group.map_or_else(|| user.to_owned(), |g| format!("{GROUP}{g}"));
        let (kind, media) = match content {
            Content::Yo => ("yo", None),
            Content::Photo(m) => ("photo", Some(media_json(content, m, refs))),
            Content::Video { media: m, .. } => ("video", Some(media_json(content, m, refs))),
            _ => ("other", None),
        };
        json!({
            "type": "message", "from": user, "group": group, "reply_to": reply_to, "sent_at_ms": sent_at_ms,
            "content": kind, "media": media,
        })
    };
    match event {
        Event::Message { user, sent_at_ms, content } => message(user, None, *sent_at_ms, content),
        Event::GroupMessage { group, user, sent_at_ms, content } => message(user, Some(group), *sent_at_ms, content),
        Event::NewChat { user } => json!({ "type": "new_chat", "user": user }),
        Event::Profile { user, name } => json!({ "type": "profile", "user": user, "name": name }),
        Event::IdentityChanged { user } => json!({ "type": "identity_changed", "user": user }),
        Event::GroupChanged { group } => json!({ "type": "group_changed", "group": group }),
    }
}

fn media_json(content: &Content, m: &Media, refs: bool) -> Value {
    let mut out = json!({ "size": m.size, "width": m.width, "height": m.height });
    if let Content::Video { duration_ms, .. } = content {
        out["duration_ms"] = json!(duration_ms);
    }
    if refs && m.key != [0; 32] {
        out["ref"] = json!(media_ref(content));
    }
    out
}

/// A photo or video as a reference: kind, length, id, key, hash, size, width, height.
fn media_ref(content: &Content) -> String {
    let (kind, duration, m) = match content {
        Content::Photo(m) => (1u8, 0u32, m),
        Content::Video { media, duration_ms } => (2, *duration_ms, media),
        _ => unreachable!("media only"),
    };
    let mut bytes = vec![kind];
    bytes.extend_from_slice(&duration.to_le_bytes());
    bytes.extend_from_slice(&m.id);
    bytes.extend_from_slice(&m.key);
    bytes.extend_from_slice(&m.sha256);
    bytes.extend_from_slice(&m.size.to_le_bytes());
    bytes.extend_from_slice(&m.width.to_le_bytes());
    bytes.extend_from_slice(&m.height.to_le_bytes());
    yo_wire::b64(&bytes)
}

fn content_of_ref(text: &str) -> Option<Content> {
    let b = yo_wire::unb64(text).ok()?;
    if b.len() != 93 {
        return None;
    }
    let media = Media {
        id: b[5..21].try_into().ok()?,
        key: b[21..53].try_into().ok()?,
        sha256: b[53..85].try_into().ok()?,
        size: u32::from_le_bytes(b[85..89].try_into().ok()?),
        width: u16::from_le_bytes(b[89..91].try_into().ok()?),
        height: u16::from_le_bytes(b[91..93].try_into().ok()?),
    };
    let duration_ms = u32::from_le_bytes(b[1..5].try_into().ok()?);
    match b[0] {
        1 => Some(Content::Photo(media)),
        2 => Some(Content::Video { media, duration_ms }),
        _ => None,
    }
}

fn read(path: &str, max: usize) -> Result<Vec<u8>, Failure> {
    use std::io::Read;
    let mut bytes = Vec::new();
    let file = std::fs::File::open(path).map_err(|e| Failure::new(format!("{path}: {e}")))?;
    file.take(max as u64 + 1).read_to_end(&mut bytes).map_err(|e| Failure::new(format!("{path}: {e}")))?;
    if bytes.is_empty() || bytes.len() > max {
        return Err(Failure::new(format!("{path}: empty or larger than {max} bytes")));
    }
    Ok(bytes)
}

/// A Yo, with a photo from a file or a received photo or video sealed anew. No video from a file: the CLI could check
/// neither its length nor take out its metadata (spec 5).
fn send(context: &Context, to: &str, args: &[&str]) -> Outcome {
    let content = match args {
        [] => Content::Yo,
        ["--photo", path] => {
            let (bytes, width, height) = jpeg::clean(&read(path, MAX_PHOTO)?)
                .ok_or_else(|| Failure::new(format!("{path}: not a JPEG of at most 1600 px a side")))?;
            Content::Photo(upload(context, to, &bytes, width, height)?)
        }
        ["--media", reference] => {
            let content =
                content_of_ref(reference).ok_or_else(|| Failure::usage("--media: a media.ref of an event"))?;
            resend(context, to, content)?
        }
        _ => return Err(Failure::usage("send TO [--photo JPEG | --media -]")),
    };
    let events = shared::send(context, &Target::parse(to), content)?;
    (context.emit)(events);
    Ok(json!({ "sent": true }))
}

/// A photo or video received, fetched and opened in memory, sealed anew under a key of its own and uploaded for `to`:
/// the same bytes, never stored. Its fetch is acknowledged, so the hub may let it go.
fn resend(context: &Context, to: &str, content: Content) -> Result<Content, Failure> {
    let (Content::Photo(m) | Content::Video { media: m, .. }) = &content else { unreachable!("media only") };
    if m.size as usize > yo_wire::limits::MAX_BLOB_LEN {
        return Err(Failure::new("larger than a blob may be"));
    }
    let transport = context.client().transport();
    let plain = media::fetch_open(transport.as_ref(), m)?.ok_or_else(|| Failure::new("the blob is gone"))?;
    let _ = media::ack(transport.as_ref(), &m.id);
    let fresh = upload(context, to, &plain, m.width, m.height)?;
    Ok(match content {
        Content::Video { duration_ms, .. } => Content::Video { media: fresh, duration_ms },
        _ => Content::Photo(fresh),
    })
}

/// Seals and uploads for `to`, the client held only to open the chat and sign.
fn upload(context: &Context, to: &str, plain: &[u8], width: u16, height: u16) -> Result<Media, Failure> {
    let (media, sealed) = media::seal(plain, width, height)?;
    let received = shared::upload(context, &Target::parse(to), &media, &sealed)?;
    (context.emit)(received);
    Ok(media)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_media_reference_carries_the_photo_or_video_whole() {
        let media = Media { id: [1; 16], key: [2; 32], sha256: [3; 32], size: 70_000, width: 720, height: 1280 };
        for content in [Content::Photo(media.clone()), Content::Video { media, duration_ms: 4321 }] {
            assert_eq!(content_of_ref(&media_ref(&content)), Some(content));
        }
        assert_eq!(content_of_ref("AAAA"), None);
    }
}
