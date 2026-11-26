//! The commands, on a client that is either this process's or a listener's (`Context::listening`).

use crate::{jpeg, Failure};
use serde_json::{json, Value};
use std::sync::{Mutex, MutexGuard};
use yo_core::connect::ConnectRequest;
use yo_core::media::{self, Media};
use yo_core::shared::{self, Locked, Target, GROUP_PREFIX as GROUP};
use yo_core::{Client, Content, Event};
use yoip::protocol::content::{CONNECT, CONNECTED, CONNECT_DECLINED, CONNECT_ENDED, CONNECT_REFUSED};

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
        ["whois", name] if own(context, name) => {
            let me = whoami(context, false)?;
            Ok(json!({ "user": me["user"], "username": me["username"], "bot": me["bot"] }))
        }
        ["whois", name] => {
            whois(context, name).map(|(user, username, bot)| json!({ "user": user, "username": username, "bot": bot }))
        }
        ["add", name] => add(context, name),
        ["send", to, rest @ ..] => send(context, to, rest),
        ["requests"] => Ok(json!({ "requests_open": context.client().requests_open() })),
        ["requests", switch @ ("on" | "off")] => {
            let open = *switch == "on";
            context.client().set_requests_open(open)?;
            Ok(json!({ "requests_open": open }))
        }
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
        ["sync", ..] => Err(Failure::new("yo listen is running: read its events instead")),
        ["ack"] => {
            context.client().acknowledge_only(|_| true);
            Ok(json!({ "acked": true }))
        }
        ["echo-probe", user, token] => {
            let token = yoip::unb64(token).ok().and_then(|t| t.try_into().ok());
            let token = token.ok_or_else(|| Failure::usage("usage: yo echo-probe USER TOKEN (32 bytes)"))?;
            done(context, |c| c.echo_probe(user, token))
        }
        [word @ ("connect" | "decline"), service, code] => {
            let code = yoip::unb64(code).ok().and_then(|c| c.try_into().ok());
            let code = code.ok_or_else(|| {
                Failure::usage("usage: yo services connect|decline SERVICE CODE (the code from its link)")
            })?;
            done(context, |c| match *word {
                "connect" => c.connect_service(service, code),
                _ => c.decline_service(service, code),
            })
        }
        ["connections"] => Ok(connections(&context.client())),
        ["disconnect", service] => {
            let user = context
                .client()
                .connections()
                .into_iter()
                .find(|c| c.user == *service || c.name.eq_ignore_ascii_case(service));
            let user = user.ok_or_else(|| Failure::new("not connected"))?.user;
            done(context, |c| c.disconnect_service(&user))
        }
        ["service-answer", user, kind, code] => {
            let kind = match *kind {
                "connected" => CONNECTED,
                "refused" => CONNECT_REFUSED,
                "ended" => CONNECT_ENDED,
                _ => return Err(Failure::usage("usage: yo services answer USER connected|refused|ended CODE")),
            };
            let code = yoip::unb64(code).ok().and_then(|c| c.try_into().ok());
            let code = code.ok_or_else(|| Failure::usage("services answer: CODE must be 32 bytes"))?;
            done(context, |c| c.answer_connection(user, kind, code))
        }
        ["bot", switch @ ("on" | "off")] => {
            context.client().set_bot(*switch == "on").map_err(|e| match e {
                yo_core::Error::Refused(409) => Failure::new("a bot can't become a person"),
                other => other.into(),
            })?;
            Ok(json!({ "bot": *switch == "on" }))
        }
        ["friends-view"] => Ok(view(&context.client())),
        ["remove", name] => {
            let user = friend_user(&context.client(), name).ok_or_else(|| Failure::new("no such friend"))?;
            let mut client = context.client();
            client.remove_friend(&user)?;
            client.write_storage()?;
            removed(context.config, &user, true);
            Ok(json!({ "removed": true, "user": user }))
        }
        ["requests", "list"] => Ok(requests(&context.client(), &removed_users(context.config))),
        ["requests", "accept", name] => {
            let events = context.client().accept(name.trim_start_matches('@'))?;
            (context.emit)(events);
            Ok(json!({ "accepted": true }))
        }
        ["requests", "decline", name] => {
            context.client().decline(name.trim_start_matches('@'))?;
            Ok(json!({ "declined": true }))
        }
        [word @ ("block" | "unblock"), name] | ["requests", word @ "block", name] => {
            let blocked = *word == "block";
            let mut client = context.client();
            client.set_blocked(name.trim_start_matches('@'), blocked)?;
            let _ = client.write_storage();
            Ok(json!({ "blocked": blocked }))
        }
        ["devices"] | ["devices", "list"] => {
            let mut client = context.client();
            let this = client.device_number();
            let list: Vec<Value> = client
                .own_devices()?
                .into_iter()
                .map(|d| {
                    json!({
                        "number": d.number, "this": d.number == this, "created_at": d.created_at, "last_seen_at": d.last_seen_at,
                        "app": String::from_utf8_lossy(d.app.as_slice()),
                    })
                })
                .collect();
            Ok(json!(list))
        }
        ["devices", "remove", number] => {
            let number: u8 = number.parse().map_err(|_| Failure::usage("usage: yo devices remove N"))?;
            context.client().remove_device(number).map_err(|e| match e {
                yo_core::Error::Refused(404) => Failure::new(format!("no device {number}; see yo devices")),
                other => other.into(),
            })?;
            Ok(json!({ "removed": number }))
        }
        ["recovery", "rotate"] => rotate_recovery(context),
        ["link-approve", text] => {
            let offer = yo_core::link::offer(text).map_err(|_| Failure::usage("not a Yo link"))?;
            context.client().link_approve(&offer).map_err(|e| match e {
                yo_core::Error::Refused(404) => {
                    Failure::new("the link is over (expired or taken): start again on the new device")
                }
                yo_core::Error::Refused(409) => Failure::new("this link was answered already"),
                other => other.into(),
            })?;
            Ok(json!({ "linked": true }))
        }
        ["probe", rest @ ..] => {
            let timeout_ms = match rest {
                [] => 10_000,
                ["--timeout-ms", ms] => ms.parse().map_err(|_| Failure::usage("--timeout-ms: not a number"))?,
                _ => return Err(Failure::usage("usage: yo probe [--timeout-ms N]")),
            };
            let ms = context.client().round_trip_probe("PING", timeout_ms)?;
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
        "device_id": yo_core::state::hex(&client.device_id()),
        "bot": client.is_bot(),
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

/// The friend for a name, handle or user id.
fn friend_user(client: &Client, name: &str) -> Option<String> {
    let name = name.trim_start_matches('@');
    let contacts = client.contacts();
    client.friends().into_iter().map(|f| f.user).find(|user| {
        user == name
            || contacts.iter().any(|c| {
                c.user == *user
                    && (c.handles.iter().any(|h| h.eq_ignore_ascii_case(name))
                        || c.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
            })
            || client
                .friends()
                .iter()
                .any(|f| f.user == *user && f.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
    })
}

/// What a person sees of a contact: its handle, else its name, else a short id.
fn shown(contact: Option<&yo_core::ContactInfo>, name: Option<&str>, user: &str) -> String {
    contact
        .and_then(|c| c.handles.first().cloned().or_else(|| c.name.clone()))
        .or_else(|| name.map(str::to_owned))
        .unwrap_or_else(|| user.chars().take(8).collect())
}

/// Friends and groups for a person (the TUI's bars, `yo friends` on a terminal): target, name, kind, flags.
fn view(client: &Client) -> Value {
    let contacts = client.contacts();
    let mut list: Vec<Value> = client
        .friends()
        .into_iter()
        .map(|f| {
            let contact = contacts.iter().find(|c| c.user == f.user);
            let username = contact
                .into_iter()
                .flat_map(|c| &c.handles)
                .chain(f.name.iter())
                .find(|name| yoip::net::handle::check_username_form(name).is_ok());
            json!({
                "kind": "friend", "target": f.user, "user": f.user,
                "color": f.color, "username": username,
                "name": shown(contact, f.name.as_deref(), &f.user),
                "bot": client.known_bot(&f.user), "blocked": f.blocked || contact.is_some_and(|c| c.blocked),
                "gone": contact.is_some_and(|c| c.gone),
            })
        })
        .collect();
    list.extend(client.groups().into_iter().filter(|g| !g.left).map(|g| {
        json!({
            "kind": "group", "target": format!("{GROUP}{}", g.id), "name": g.name, "members": g.members.len(),
            "accepted": g.accepted, "color": null, "bot": false, "blocked": false, "gone": false,
        })
    }));
    json!(list)
}

/// Friends removed here (`removed`, one user id a line), as the app keeps them: no request row for them.
fn removed_users(config: &crate::Config) -> Vec<String> {
    let text = crate::account::read_file(config, "removed").unwrap_or_default();
    String::from_utf8_lossy(&text).lines().map(str::to_owned).collect()
}

fn removed(config: &crate::Config, user: &str, on: bool) {
    let mut users = removed_users(config);
    users.retain(|u| u != user);
    if on {
        users.push(user.to_owned());
    }
    let _ = crate::account::write_file(config, "removed", users.join("\n").as_bytes());
}

/// Requests from strangers (spec 3.3): chats not said yes to, from someone not on the friend list.
fn requests(client: &Client, removed: &[String]) -> Value {
    let friends: Vec<String> = client.friends().into_iter().map(|f| f.user).collect();
    let list: Vec<Value> = client
        .contacts()
        .into_iter()
        .filter(|c| {
            client.has_direct_chat(&c.user)
                && !c.accepted
                && !c.blocked
                && !c.gone
                && !removed.contains(&c.user)
                && !friends.contains(&c.user)
        })
        .map(|c| json!({ "user": c.user, "name": shown(Some(&c), None, &c.user), "bot": c.bot }))
        .collect();
    json!({ "requests_open": client.requests_open(), "requests": list })
}

/// A new recovery key (spec 2.4): the backup made anew under it (the account's key in it, as the apps seal it), the
/// key kept in the directory (`recovery`, 0600) for `yo recovery show`. The key before stops working.
pub fn rotate_recovery(context: &Context) -> Outcome {
    use base64::prelude::{Engine, BASE64_STANDARD};
    let code = yo_core::backup::new_recovery_key();
    let mut client = context.client();
    let user_id = yo_core::state::unhex(client.user_id()).unwrap_or_default();
    let sealed = json!({
        "account": {
            "username": client.username(), "userID": BASE64_STANDARD.encode(user_id),
            "accountKey": BASE64_STANDARD.encode(client.account_secret()), "byPasskey": false, "byCode": true,
            "codeAuth": true, "passkeyAuth": false, "backupVersion": 3,
        },
        "code": code.to_string(),
    });
    let k = yo_core::backup::new_k();
    let record = yo_core::backup::make(&k, sealed.to_string().as_bytes(), &code, yo_core::backup::PasskeyDoor::None)?;
    client.put_backup(&record)?;
    drop(client);
    crate::account::write_file(context.config, "recovery", code.to_string().as_bytes())?;
    Ok(json!({ "recovery_key": code.to_string() }))
}

fn connections(client: &Client) -> Value {
    let list: Vec<Value> = client
        .connections()
        .into_iter()
        .map(|c| {
            json!({
                "user": c.user, "name": c.name, "connected": c.connected, "waiting": c.waiting, "refused": c.refused,
                "ending": c.ending,
            })
        })
        .collect();
    json!(list)
}

fn contacts(client: &Client) -> Value {
    let list: Vec<Value> = client
        .contacts()
        .into_iter()
        .map(|c| json!({ "user": c.user, "handles": c.handles, "name": c.name, "blocked": c.blocked, "gone": c.gone, "bot": c.bot }))
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
/// Whether a name is this account's own.
fn own(context: &Context, name: &str) -> bool {
    let name = name.trim_start_matches('@');
    context.client().username().is_some_and(|me| yoip::net::handle::same_name(me, name))
}

fn whois(context: &Context, name: &str) -> Result<(String, Option<String>, bool), Failure> {
    let lookup = context.client().whois_begin(name)?;
    let (user, username, bot) = lookup.send()?;
    context.client().note_bot(&user, bot)?;
    Ok((user, username, bot))
}

/// Members by user id: as they are, usernames looked up.
fn users(context: &Context, names: &[&str]) -> Result<Vec<String>, Failure> {
    names.iter().map(|name| shared::user_for(context, name.trim_start_matches('@'))).collect()
}

fn add(context: &Context, name: &str) -> Outcome {
    let name = name.trim_start_matches('@');
    if own(context, name) {
        return Err(Failure::usage("that's you"));
    }
    let (user, username, _) = whois(context, name)?;
    let mut client = context.client();
    client.whois_end(name, &user, username.clone())?;
    client.set_friend(&user, username.clone(), None)?;
    client.write_storage()?;
    removed(context.config, &user, false);
    Ok(json!({ "user": user, "username": username }))
}

fn sync(context: &Context, args: &[&str]) -> Outcome {
    let (mut wait, mut drain_media) = (0, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match *arg {
            "--wait" => {
                wait = args
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| Failure::usage("--wait: seconds, e.g. --wait 30"))?
            }
            "--drain-media" => drain_media = true,
            _ => return Err(Failure::usage("usage: yo sync [--wait S] [--drain-media]")),
        }
    }
    let mut client = context.client();
    // As `listen --drain-media`: no media key kept, every received blob fetched into nothing, never opened.
    if drain_media {
        client.forget_media_keys();
    }
    let events = client.sync(wait)?;
    let mut out: Vec<Value> = events.iter().map(|e| event(e, !drain_media)).collect();
    out.extend(client.take_probe_requests().into_iter().map(|(user, token)| probe(&user, &token)));
    out.extend(client.take_connect_requests().into_iter().map(|request| connect(&request)));
    let _ = client.after_sync();
    client.acknowledge_only(|e| events.contains(e));
    if drain_media {
        let transport = client.transport();
        drop(client);
        for event in &events {
            if let Event::Message { content, .. } | Event::GroupMessage { content, .. } = event {
                if let Content::Photo(blob) | Content::Video { media: blob, .. } = content {
                    crate::listen::drain(transport.as_ref(), blob);
                }
            }
        }
    }
    Ok(json!(out))
}

/// A user connects this service with another one, or ends that.
pub fn connect((user, kind, code): &ConnectRequest) -> Value {
    let kind = match *kind {
        CONNECT => "connect",
        CONNECT_DECLINED => "decline",
        _ => "disconnect",
    };
    json!({ "type": kind, "from": user, "code": yoip::b64(code) })
}

/// A probe request for an echo service: answered with `echo-probe`.
pub fn probe(user: &str, token: &[u8; 32]) -> Value {
    json!({ "type": "probe", "from": user, "token": yoip::b64(token) })
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
        Event::DeviceAdded { user } => json!({ "type": "device_added", "user": user }),
        Event::ConnectionChanged { user } => json!({ "type": "connection_changed", "user": user }),
        Event::Sent { user, sent_at_ms, content } => {
            let kind = match content {
                Content::Yo => "yo",
                Content::Photo(_) => "photo",
                Content::Video { .. } => "video",
                _ => "other",
            };
            json!({ "type": "sent", "user": user, "sent_at_ms": sent_at_ms, "content": kind })
        }
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
    yoip::b64(&bytes)
}

fn content_of_ref(text: &str) -> Option<Content> {
    let b = yoip::unb64(text).ok()?;
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
/// neither its length nor take out its metadata (spec 3.2).
fn send(context: &Context, to: &str, args: &[&str]) -> Outcome {
    let content = match args {
        [] => Content::Yo,
        ["--photo", path] => {
            let (bytes, width, height) = jpeg::clean(&read(path, MAX_PHOTO)?)
                .ok_or_else(|| Failure::new(format!("{path}: needs a JPEG, at most 1600 px a side")))?;
            Content::Photo(upload(context, to, &bytes, width, height)?)
        }
        ["--media", reference] => {
            let content = content_of_ref(reference)
                .ok_or_else(|| Failure::usage("--media: not a media reference from an event"))?;
            resend(context, to, content)?
        }
        _ => return Err(Failure::usage("usage: yo send TO [--photo JPEG | --media -]")),
    };
    let events = shared::send(context, &Target::parse(to), content)?;
    (context.emit)(events);
    Ok(json!({ "sent": true }))
}

/// A photo or video received, fetched and opened in memory, sealed anew under a key of its own and uploaded for `to`:
/// the same bytes, never stored. Its fetch is acknowledged, so the hub may let it go.
fn resend(context: &Context, to: &str, content: Content) -> Result<Content, Failure> {
    let (Content::Photo(m) | Content::Video { media: m, .. }) = &content else { unreachable!("media only") };
    if m.size as usize > yoip::net::limits::MAX_BLOB_LEN {
        return Err(Failure::new("too large to send"));
    }
    let transport = context.client().transport();
    let plain = media::fetch_open(transport.as_ref(), m)?.ok_or_else(|| Failure::new("the photo or video is gone"))?;
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
    fn declining_the_only_chat_empties_requests() {
        use yo_core::state::{Contact, State};
        struct Offline;
        impl yo_core::Transport for Offline {
            fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<yo_core::Response, String> {
                panic!("local operations only")
            }
        }

        let pending = "aa".repeat(16);
        let mut state = State::default();
        state.contacts.insert(pending.clone(), Contact { chat: true, ..Default::default() });
        state.contacts.insert("bb".repeat(16), Contact::default());
        let (mut client, _) =
            Client::from_snapshot(std::sync::Arc::new(Offline), state, Default::default(), false).unwrap();
        let list = requests(&client, &[]);
        assert_eq!(list["requests"].as_array().unwrap().len(), 1);
        assert_eq!(list["requests"][0]["user"], pending);
        client.decline(&pending).unwrap();
        assert_eq!(requests(&client, &[])["requests"], json!([]));
    }

    #[test]
    fn a_media_reference_carries_the_photo_or_video_whole() {
        let media = Media { id: [1; 16], key: [2; 32], sha256: [3; 32], size: 70_000, width: 720, height: 1280 };
        for content in [Content::Photo(media.clone()), Content::Video { media, duration_ms: 4321 }] {
            assert_eq!(content_of_ref(&media_ref(&content)), Some(content));
        }
        assert_eq!(content_of_ref("AAAA"), None);
    }
}
