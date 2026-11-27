//! Mail (spec 3.2, 4.1, 4.2): sealed deliveries stored by one writer in group commits, long polls, pushes.

use crate::db::{self, Copies, Db, Delivered, Incoming};
use crate::error::{self, refuse, Refusal, Res};
use crate::limiter::{self, Place};
use crate::request::{b64_array, bad, header, held, path_id, query, Auth, Hubs, Peer};
use crate::{now_ms, push, Hub};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::{Arc, PoisonError, Weak};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use yoip::net::api::{self, HEADER_DEVICES};
use yoip::net::limits::{self, MAX_INBOX_FETCH};
use yoip::net::request::HEADER_DEVICE;
use yoip::{decode, encode, DeviceId, UserId};

/// Inbox requests of one device at once (app, extension, share): more is `429`, not the hub's memory and connections.
pub const INBOX_AT_ONCE: u32 = 4;

/// The most envelopes one transaction stores.
const BATCH: usize = 512;

/// The most deliveries waiting for the writer (each up to 64 KiB): beyond, `503` rather than the memory growing.
pub(crate) const QUEUED: usize = 2048;
/// The most bytes waiting for the writer (a delivery of copies per device up to 257 KiB): beyond, `503` too.
pub(crate) const QUEUED_BYTES: usize = 128 << 20;

/// An envelope on its way to the writer.
pub(crate) struct Delivery {
    pub user: UserId,
    pub devices_hash: [u8; 32],
    /// The sending device, writing to its own user (`Incoming::from`).
    pub from: Option<DeviceId>,
    /// One envelope per device by number (`api::Copies`); none: `envelope` is one envelope for every device (else
    /// empty, the body not kept twice).
    pub copies: Option<Vec<(u8, Vec<u8>)>>,
    pub envelope: Bytes,
    /// Its bytes in `Hub::queued_bytes`, given back once stored.
    pub size: usize,
    pub urgent: bool,
    /// From the operator's bots (the server's own services, or a bot account's request): `bot-yo`, not `yo-in`.
    pub from_bot: bool,
    /// A request (signed, 3.3): its pushes are paced by the requests' window, a contact's by the contacts'.
    pub request: bool,
    pub stored: tokio::sync::oneshot::Sender<Result<Delivered, ()>>,
    /// Where its pushes are sent from.
    pub runtime: tokio::runtime::Handle,
}

/// A device's pushes (spec 4.2): when it was pushed last, and the one held for the end of its window: the device to
/// push (its latest token), whether any delivery it stands for was urgent, and when it is due.
pub(crate) struct Pushed {
    at: Instant,
    held: Option<(db::Wake, bool, Instant)>,
}

pub(crate) type Pushes = HashMap<DeviceId, Pushed>;

/// Group commit: what has queued up, in one transaction; then wakes the devices and answers each.
pub(crate) fn writer(
    db: Arc<Db>,
    queue: std::sync::mpsc::Receiver<Delivery>,
    most: u32,
    hub: Weak<Hub>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
) {
    while let Ok(first) = queue.recv() {
        let mut batch = vec![first];
        batch.extend(queue.try_iter().take(BATCH - 1));
        // Given back once written: what is being written still counts.
        let bytes: usize = batch.iter().map(|d| d.size).sum();
        let items: Vec<Incoming<'_>> = batch
            .iter()
            .map(|d| Incoming {
                user: d.user,
                devices_hash: d.devices_hash,
                from: d.from,
                copies: match &d.copies {
                    Some(each) => Copies::Each(each.iter().map(|(n, e)| (*n, e.as_slice())).collect()),
                    None => Copies::Legacy(&d.envelope),
                },
            })
            .collect();
        let hub = hub.upgrade();
        let now = hub.as_ref().map_or_else(now_ms, |hub| hub.clock_ms());
        let delivered = db.deliver(&items, now, most);
        drop(items);
        queued.fetch_sub(bytes, std::sync::atomic::Ordering::Relaxed);
        match delivered {
            Ok(outcomes) => {
                for (delivery, mut outcome) in batch.into_iter().zip(outcomes) {
                    // Here, not in the request: one cut off after its envelope was stored still wakes and pushes.
                    if let (Delivered::Stored(devices), Some(hub)) = (&mut outcome, &hub) {
                        // Counted here too: a bot's inbox apart (every device a bot's), nothing of whose.
                        let bot = !devices.is_empty() && devices.iter().all(|w| w.bot);
                        hub.count(
                            match (delivery.from_bot, bot) {
                                (true, _) => crate::Event::BotYo,
                                (false, true) => crate::Event::YoInBot,
                                (false, false) => crate::Event::YoIn,
                            },
                            1,
                        );
                        hub.woken(std::mem::take(devices), delivery.urgent, delivery.request, &delivery.runtime);
                    }
                    let _ = delivery.stored.send(Ok(outcome));
                }
            }
            Err(error) => {
                eprintln!("yo-hub: database: {error}");
                for delivery in batch {
                    let _ = delivery.stored.send(Err(()));
                }
            }
        }
    }
}

/// A long poll waiting for mail (`Hub::waiter`).
pub(crate) struct Waiter<'a> {
    hub: &'a Hub,
    device: DeviceId,
    pub notify: Arc<Notify>,
}

impl Drop for Waiter<'_> {
    fn drop(&mut self) {
        let mut waiting = self.hub.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        // Two references: the map's and ours. Nobody else waits.
        if Arc::strong_count(&self.notify) == 2 {
            waiting.remove(&self.device);
        }
    }
}

impl Hub {
    /// A place for one more inbox request of a device, if it has fewer than `INBOX_AT_ONCE` under way.
    pub(crate) fn fetching(&self, device: DeviceId) -> Option<Place> {
        limiter::take_place(&self.fetching, device, INBOX_AT_ONCE, None)
    }

    /// A long poll's wait for mail to `device`; the entry goes with its last waiter, also when cut off.
    pub(crate) fn waiter(&self, device: DeviceId) -> Waiter<'_> {
        let notify = self.waiting.lock().unwrap_or_else(PoisonError::into_inner).entry(device).or_default().clone();
        Waiter { hub: self, device, notify }
    }

    /// What follows a stored delivery: its devices' long polls answer, and each is pushed (spec 4.2), paced.
    fn woken(self: &Arc<Self>, devices: Vec<db::Wake>, urgent: bool, request: bool, runtime: &tokio::runtime::Handle) {
        for wake in devices {
            let window = self.push_window(request, wake.waiting);
            if let Some(notify) = self.waiting.lock().unwrap_or_else(PoisonError::into_inner).get(&wake.device_id) {
                notify.notify_waiters();
            }
            if !wake.push_token.is_empty() {
                let backlog = wake.waiting > self.config.limits.push_backlog;
                if self.push_paced(wake, urgent, window, runtime) {
                    self.count(if backlog { crate::Event::PushPaced } else { crate::Event::PushFolded }, 1);
                }
            }
        }
    }

    /// The window between a device's pushes: a contact's or a request's; past `push_backlog` waiting envelopes the
    /// device is evidently offline or flooded, and it gets one push per `push_backlog_interval_s` instead.
    pub(crate) fn push_window(&self, request: bool, waiting: u32) -> Duration {
        let limits = &self.config.limits;
        if waiting > limits.push_backlog {
            return Duration::from_secs(limits.push_backlog_interval_s);
        }
        Duration::from_millis(if request { limits.push_window_request_ms } else { limits.push_window_contact_ms })
    }

    /// One push per device per `window` at most (spec 4.2): a quiet device's goes at once; what comes within the
    /// window after it goes as one push at its end. The push says only "there is mail": the device fetches all.
    pub(crate) fn push_paced(
        self: &Arc<Self>,
        wake: db::Wake,
        urgent: bool,
        window: Duration,
        runtime: &tokio::runtime::Handle,
    ) -> bool {
        let now = Instant::now();
        let device = wake.device_id;
        let mut folded = false;
        let mut pushes = self.pushes.lock().unwrap_or_else(PoisonError::into_inner);
        let due = match pushes.get_mut(&device) {
            Some(pushed) => {
                let due = pushed.at + window;
                match &mut pushed.held {
                    // Joins the push held; due sooner if its own window ends sooner (a contact's after a request's).
                    Some((held, was_urgent, held_due)) => {
                        *held = wake;
                        *was_urgent |= urgent;
                        if due >= *held_due {
                            return true;
                        }
                        *held_due = due.max(now);
                        folded = true;
                        Some(*held_due)
                    }
                    None if now < due => {
                        pushed.held = Some((wake, urgent, due));
                        Some(due)
                    }
                    None => {
                        pushed.at = now;
                        drop(pushes);
                        runtime.spawn(self.clone().push(wake, urgent));
                        return false;
                    }
                }
            }
            None => {
                pushes.insert(device, Pushed { at: now, held: None });
                drop(pushes);
                runtime.spawn(self.clone().push(wake, urgent));
                return false;
            }
        };
        drop(pushes);
        let (hub, due) = (self.clone(), due.unwrap_or(now));
        runtime.spawn(async move {
            tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
            let held = {
                let mut pushes = hub.pushes.lock().unwrap_or_else(PoisonError::into_inner);
                pushes.get_mut(&device).and_then(|pushed| match &pushed.held {
                    // Only the timer of the push held now: one made due sooner has sent it already.
                    Some((_, _, held_due)) if *held_due == due => {
                        pushed.at = Instant::now();
                        pushed.held.take()
                    }
                    _ => None,
                })
            };
            if let Some((wake, urgent, _)) = held {
                hub.push(wake, urgent).await;
            }
        });
        folded
    }

    /// Devices pushed longer ago than the longest window (`push_window`) with nothing held: a push to them goes at
    /// once anyway. One forgotten sooner would lose a backlog device's pacing.
    pub(crate) fn forget_pushes(&self, now: Instant) {
        let limits = &self.config.limits;
        let longest = Duration::from_secs(limits.push_backlog_interval_s)
            .max(Duration::from_millis(limits.push_window_request_ms.max(limits.push_window_contact_ms)))
            .max(Duration::from_secs(60));
        self.pushes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, pushed| pushed.held.is_some() || now.saturating_duration_since(pushed.at) < longest);
    }

    pub(crate) async fn push(self: Arc<Self>, wake: db::Wake, urgent: bool) {
        match self.pusher.wake(wake.push_platform, &wake.push_token, urgent).await {
            push::Outcome::TokenInvalid => {
                let cleared = self.blocking(move |hub| hub.db.clear_push(&wake.device_id, &wake.push_token));
                if let Ok(Err(error)) = cleared.await {
                    eprintln!("yo-hub: clear push: {error}");
                }
            }
            push::Outcome::Failed(failure) => {
                eprintln!("yo-hub: push: {}", failure.detail);
                self.count(crate::Event::PushFail, 1);
                // An error as the apps report theirs (domain "push", the reason as its code, the service as its
                // place, app "hub"): grouped by those, nothing of the device; the operator's Errors.
                let payload = serde_json::json!({
                    "domain": "push", "code": failure.reason, "place": failure.service, "app": "hub",
                    "details": failure.detail,
                });
                let now = self.clock_ms();
                let noted = self.blocking(move |hub| hub.db.diagnostic("error", &payload, None, now));
                if let Ok(Err(error)) = noted.await {
                    eprintln!("yo-hub: push diagnostic: {error}");
                }
            }
            push::Outcome::Sent => self.count(crate::Event::PushOk, 1),
            push::Outcome::Skipped => {}
        }
    }
}

const ACK_WINDOW: Duration = Duration::from_secs(120);
#[derive(Default)]
pub(crate) struct RecentAcks(HashMap<DeviceId, HashMap<u64, Instant>>);

impl RecentAcks {
    pub fn expire(&mut self) {
        self.0.retain(|_, ids| {
            ids.retain(|_, at| at.elapsed() < ACK_WINDOW);
            !ids.is_empty()
        });
    }

    pub fn remember(&mut self, device: DeviceId, ids: &[u64]) {
        if !ids.is_empty() {
            let now = Instant::now();
            self.0.entry(device).or_default().extend(ids.iter().map(|id| (*id, now)));
        }
    }

    pub fn contains(&self, device: &DeviceId, id: u64) -> bool {
        self.0.get(device).and_then(|ids| ids.get(&id)).is_some_and(|at| at.elapsed() < ACK_WINDOW)
    }
}

fn merge_items(mut items: Vec<api::InboxItem>, device: &DeviceId, recent: &RecentAcks) -> Vec<api::InboxItem> {
    items.retain(|item| !recent.contains(device, item.id));
    items.sort_by_key(|item| item.id);
    items.dedup_by_key(|item| item.id);
    items.truncate(MAX_INBOX_FETCH);
    items
}

fn inbox_items(rows: Vec<(u64, Vec<u8>, u64)>) -> Vec<api::InboxItem> {
    rows.into_iter()
        .map(|(id, envelope, received_ms)| api::InboxItem { id, received_ms, envelope: envelope.into() })
        .collect()
}

pub(crate) async fn peer_inbox(State(hub): Hubs, headers: HeaderMap, Path(device): Path<String>) -> Res<Vec<u8>> {
    use subtle::ConstantTimeEq;
    let token = hub.config.corrosion.as_ref().and_then(|c| c.peer_token.as_deref()).filter(|t| !t.is_empty());
    if !token.zip(headers.get("yo-peer-token")).is_some_and(|(t, h)| bool::from(t.as_bytes().ct_eq(h.as_bytes()))) {
        return Err(refuse(StatusCode::NOT_FOUND));
    }
    let id = path_id(&device)?;
    let rows = hub.blocking(move |h| h.db.fetch(&id, MAX_INBOX_FETCH)).await??;
    Ok(encode(&api::Inbox { prekeys_left: 0, items: inbox_items(rows) }))
}

async fn ask_peers(hub: &Hub, client: &yo_mesh::Client, device: &DeviceId) -> Vec<api::InboxItem> {
    use futures_util::{stream::FuturesUnordered, StreamExt};
    let path = format!("/v1/peer/inbox/{}", yoip::b64(device));
    let token = hub.config.corrosion.as_ref().unwrap().peer_token.as_ref().unwrap();
    let mut requests = client
        .peers()
        .map(|name| {
            let req = axum::http::Request::get(&path).header("yo-peer-token", token).body(Bytes::new());
            async move {
                let reply = client.send(name, req.ok()?, Duration::from_millis(300)).await.ok()?;
                if reply.status() != StatusCode::OK {
                    return None;
                }
                decode::<api::Inbox>(reply.body()).ok()
            }
        })
        .collect::<FuturesUnordered<_>>();
    let mut items = Vec::new();
    while let Some(reply) = requests.next().await {
        if let Some(inbox) = reply {
            items.extend(inbox.items);
        }
    }
    items
}

/// `GET /v1/inbox?wait=s`: the device's envelopes, oldest first, waiting up to `wait` seconds for one.
pub(crate) async fn get_inbox(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap) -> Res<Response> {
    let wait_s = query::<u64>(&uri, "wait").unwrap_or(0).min(limits::MAX_WAIT_S);
    let split = query::<u8>(&uri, "split");
    let device = hub.blocking(move |hub| hub.authenticate(&method, &uri, &headers, &[])).await??;
    let fetching = hub.fetching(device.device_id).ok_or_else(|| refuse(StatusCode::TOO_MANY_REQUESTS))?;
    let id = device.device_id;
    let peers = hub.peers.as_ref().filter(|_| wait_s == 0);
    let only_missing = hub.db.agent.is_some() && wait_s == 0 && split == Some(2);
    let read = move |hub: &Hub| {
        let rows = if only_missing { Vec::new() } else { hub.db.fetch(&id, MAX_INBOX_FETCH)? };
        Ok::<_, rusqlite::Error>((rows, hub.db.prekeys_left(&id)?))
    };
    let mut status = StatusCode::OK;
    let mut remote = Vec::new();
    let (mut items, mut prekeys_left) = match peers {
        Some(_) if split == Some(1) => {
            status = StatusCode::PARTIAL_CONTENT;
            hub.blocking(read).await??
        }
        Some(client) => {
            let (local, answers) = tokio::join!(hub.blocking(read), ask_peers(&hub, client, &id));
            let local = local??;
            if !answers.is_empty() {
                remote = hub.blocking(move |h| h.db.peer_items(&id, answers, only_missing, h.clock_ms())).await??;
            }
            local
        }
        None => hub.blocking(read).await??,
    };
    if items.is_empty() && wait_s > 0 {
        let waiter = hub.waiter(id);
        let notified = waiter.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        // Mail that came between the first fetch and `enable` is found here.
        (items, prekeys_left) = hub.blocking(read).await??;
        if items.is_empty() {
            // A stopping hub answers at once: the client asks again and reaches the next process.
            let mut closing = hub.closing.subscribe();
            tokio::select! {
                _ = tokio::time::timeout(Duration::from_secs(wait_s), notified) => {}
                _ = closing.wait_for(|closing| *closing) => {}
            }
            (items, prekeys_left) = hub.blocking(read).await??;
        }
    }
    let mut items = inbox_items(items);
    if peers.is_some() {
        items.append(&mut remote);
        items = merge_items(items, &id, &hub.db.agent.as_ref().unwrap().recent_acks.lock().unwrap());
    }
    let mut response = held(encode(&api::Inbox { prekeys_left, items }), fetching);
    *response.status_mut() = status;
    Ok(response)
}

/// `POST /v1/inbox/ack`: the envelopes the device has stored.
pub(crate) async fn post_ack(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let request: api::Ack = decode(&body)?;
    if request.ids.len() > limits::MAX_ACK {
        return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
    }
    // An envelope id is a database row id (SQLite's i64): one past it is no envelope, and a malformed request.
    if request.ids.iter().any(|&id| i64::try_from(id).is_err()) {
        return Err(refuse(StatusCode::BAD_REQUEST));
    }
    let (acked, remote, bot) = hub.db.ack_inbox(&device.device_id, &request.ids)?;
    hub.count_acked(&acked, hub.clock_ms(), bot);
    if remote > 0 {
        hub.count(if bot { crate::Event::YoOutBot } else { crate::Event::YoOut }, remote as u64);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/inbox/{user}`: an envelope for every device of a user, if the sender's view of them is the hub's (3.2).
/// Sealed: no identity, but for a request (spec 3.3).
pub(crate) async fn post_envelope(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Path(user): Path<String>,
    body: Bytes,
) -> Res<(StatusCode, Vec<u8>)> {
    let user: UserId = path_id(&user)?;
    let urgent = match headers.get(api::HEADER_URGENT).map(|v| v.as_bytes()) {
        None | Some(b"true") => true,
        Some(b"false") => false,
        _ => return Err(bad()),
    };
    let devices_hash: [u8; 32] = header(&headers, HEADER_DEVICES).and_then(b64_array).ok_or_else(bad)?;
    let signed = headers.contains_key(HEADER_DEVICE);
    // One envelope per device (spec 3.2), each a bucket's size; an app before multi-device sends one for all.
    let copies: Option<Vec<(u8, Vec<u8>)>> = if headers.contains_key(api::HEADER_COPIES) {
        let copies: api::Copies = decode(&body).map_err(|_| bad())?;
        let mut numbers = std::collections::BTreeSet::new();
        if copies.copies.is_empty()
            || copies.copies.len() > limits::MAX_DEVICES
            || !copies.copies.iter().all(|c| numbers.insert(c.device))
        {
            return Err(bad());
        }
        for copy in &copies.copies {
            yoip::protocol::envelope::check(copy.envelope.as_slice(), signed)?;
        }
        Some(copies.copies.into_iter().map(|c| (c.device, c.envelope.as_slice().to_vec())).collect())
    } else {
        if body.len() > limits::MAX_BODY {
            return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
        }
        yoip::protocol::envelope::check(&body, signed)?;
        None
    };
    // Limited by its address only (4.1), and by its recipient's inbox below. An `Authorization` header is ignored.
    hub.limit_anonymous(peer, &headers)?;

    // Whose it is (spec 3.3): a contact's (the profile key shown, or a group send token, 3.4), else a request,
    // which its sending device signs; unsigned it is `403`.
    let signer = if signed {
        let (uri, headers, body) = (uri.clone(), headers.clone(), body.clone());
        Some(hub.blocking(move |hub| hub.authenticate(&method, &uri, &headers, &body)).await??)
    } else {
        None
    };
    let sender = signer.as_ref().map(|s| s.user_id);
    // To its own user (Signal's sync): never a request, never back to the sending device.
    let from = signer.filter(|s| s.user_id == user).map(|s| s.device_id);
    if from.is_some() {
        let sealed = |bytes: &[u8]| bytes.first() == Some(&yoip::protocol::envelope::SEALED);
        if copies.as_ref().map_or_else(|| sealed(&body), |copies| copies.iter().any(|(_, bytes)| sealed(bytes))) {
            return Err(bad());
        }
    }
    let shown = header(&headers, api::HEADER_ACCESS).and_then(b64_array).map(|key| api::access_check(&key));
    let endorsed = header(&headers, api::HEADER_GROUP_SEND).is_some_and(|token| hub.endorsed(&user, token));
    let request = if endorsed || from.is_some() {
        None
    } else {
        match hub.blocking(move |hub| hub.db.admit(&user, shown, sender.as_ref())).await?? {
            db::Admission::Contact => None,
            db::Admission::Request => sender,
            db::Admission::Closed => return Err(refuse(StatusCode::FORBIDDEN)),
            db::Admission::Unknown => return Err(refuse(StatusCode::NOT_FOUND)),
        }
    };
    // A request past its buckets waits, for this recipient only (`429` with `Retry-After`); a bot account's never.
    let request = request.filter(|sender| hub.bot(sender).is_none());
    let from_bot = hub.own_service(peer, &headers) || sender.is_some_and(|sender| hub.bot(&sender).is_some());
    // A contact's delivery: the envelope bytes into the recipient's inbox from everyone (4.1, Signal's inbound
    // message bytes), not what waits; a bot's inbox and the server's own services pass. `429` naming the recipient,
    // `Retry-After` in whole minutes: the answer of every limit, telling nothing of the inbox.
    if request.is_none() && !from_bot && hub.bot(&user).is_none() {
        let limits = &hub.config.limits;
        let key = format!("inbound:{}", yoip::b64(&user));
        let (size, per) = (limits.recipient_bytes_burst, limits.recipient_bytes_per_hour);
        hub.limiter
            .take_shaped(key, body.len() as u64, size, per, crate::request::HOUR)
            .map_err(|ms| error::limited("recipient", Some(ms.div_ceil(60_000).clamp(1, 60) * 60_000)))?;
    }
    // Taken now, before the writer stores it (requests sent together must not pass one token), given back unless stored.
    let reserved = match &request {
        Some(sender) => Some(
            hub.requests
                .reserve(sender, &user, hub.clock_ms())
                .map_err(|(scope, ms)| error::limited(scope, Some(ms)))?,
        ),
        None => None,
    };
    let give_back = |hub: &Hub, reserved: Option<crate::limiter::Reserved>| {
        if let Some(reserved) = reserved {
            hub.requests.give_back(reserved, hub.clock_ms());
        }
    };
    if let Err(full) = hub.db_room() {
        give_back(&hub, reserved);
        return Err(full);
    }

    // To the writer; full (it is behind) or gone: "later", the envelope stays with its sender. Never waits here.
    let size = body.len();
    let queued = &hub.queued_bytes;
    if queued.fetch_add(size, std::sync::atomic::Ordering::Relaxed) + size > QUEUED_BYTES {
        queued.fetch_sub(size, std::sync::atomic::Ordering::Relaxed);
        give_back(&hub, reserved);
        return Err(refuse(StatusCode::SERVICE_UNAVAILABLE));
    }
    let (stored, outcome) = tokio::sync::oneshot::channel();
    let runtime = tokio::runtime::Handle::current();
    let envelope = if copies.is_some() { Bytes::new() } else { body.clone() };
    let sent = hub
        .deliveries
        .try_send(Delivery {
            user,
            devices_hash,
            from,
            copies,
            size,
            envelope,
            urgent,
            from_bot,
            request: request.is_some(),
            stored,
            runtime,
        })
        .map_err(|_| {
            queued.fetch_sub(size, std::sync::atomic::Ordering::Relaxed);
            refuse(StatusCode::SERVICE_UNAVAILABLE)
        });
    let outcome = match &sent {
        Ok(()) => outcome.await.ok().and_then(Result::ok),
        Err(_) => None,
    };
    let Some(outcome) = outcome else {
        give_back(&hub, reserved);
        return Err(sent.err().unwrap_or_else(|| refuse(StatusCode::INTERNAL_SERVER_ERROR)));
    };
    if !matches!(outcome, Delivered::Stored(_)) {
        give_back(&hub, reserved);
    }
    match outcome {
        Delivered::Stored(_) => {
            if let Some(sender) = request {
                let now = hub.clock_ms();
                // Counted by its sender (the operator's "Requests 24h"); a contact's delivery never is.
                if let Ok(Err(error)) = hub.blocking(move |hub| hub.db.count_request(&sender, now)).await {
                    eprintln!("yo-hub: count request: {error}");
                }
            }
            Ok((StatusCode::ACCEPTED, Vec::new()))
        }
        Delivered::Mismatch(roster) => {
            Err(Refusal(StatusCode::CONFLICT, hub.signed_answer(&headers, uri.path(), &body, encode(&roster))))
        }
        // This recipient has no room now: `507` holds back its deliveries only (a `429` stops the sender's round).
        Delivered::Full => Err(refuse(StatusCode::INSUFFICIENT_STORAGE)),
        Delivered::Unknown => Err(refuse(StatusCode::NOT_FOUND)),
    }
}

#[cfg(test)]
mod peer_tests {
    use super::*;

    #[test]
    fn merge_filters_device_acks_before_sorting_deduplicating_and_limiting() {
        let device = [1; 16];
        let other = [2; 16];
        let mut recent = RecentAcks::default();
        recent.remember(device, &[1, 2]);
        recent.remember(other, &[3]);
        recent.0.get_mut(&device).unwrap().insert(2, Instant::now() - ACK_WINDOW);
        let item = |id| api::InboxItem { id, received_ms: 0, envelope: vec![0].into() };
        let mut rows = vec![api::InboxItem { received_ms: 10, ..item(3) }];
        rows.extend((1..=MAX_INBOX_FETCH as u64 + 4).rev().map(item));
        let rows = merge_items(rows, &device, &recent);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), (2..2 + MAX_INBOX_FETCH as u64).collect::<Vec<_>>());
        assert_eq!(rows[1].received_ms, 10);
        recent.expire();
        assert!(!recent.0[&device].contains_key(&2));
    }
}
