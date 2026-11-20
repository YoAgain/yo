//! Mail (spec 4.4, 6.3, 6.7): sealed deliveries stored by one writer in group commits, long polls, pushes.

use crate::db::{self, Db, Delivered, Incoming};
use crate::error::{self, refuse, Refusal, Res};
use crate::limiter::{self, Place};
use crate::request::{b64_array, bad, header, held, path_id, query, Auth, Hubs, Peer, MINUTE};
use crate::{now_ms, push, Hub};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use bytes::Bytes;
use std::sync::{Arc, PoisonError, Weak};
use std::time::Duration;
use tokio::sync::Notify;
use yo_wire::api::{self, HEADER_DEVICES};
use yo_wire::limits::{self, MAX_INBOX_FETCH};
use yo_wire::request::HEADER_DEVICE;
use yo_wire::{decode, encode, DeviceId, UserId};

/// Inbox requests of one device at once (app, extension, share): more is `429`, not the hub's memory and connections.
pub const INBOX_AT_ONCE: u32 = 4;

/// The most envelopes one transaction stores.
const BATCH: usize = 512;

/// The most deliveries waiting for the writer (each up to 64 KiB): beyond, `503` rather than the memory growing.
pub(crate) const QUEUED: usize = 2048;

/// An envelope on its way to the writer.
pub(crate) struct Delivery {
    pub user: UserId,
    pub devices_hash: [u8; 32],
    pub envelope: Bytes,
    pub urgent: bool,
    pub stored: tokio::sync::oneshot::Sender<Result<Delivered, ()>>,
    /// Where its pushes are sent from.
    pub runtime: tokio::runtime::Handle,
}

/// Group commit: what has queued up, in one transaction; then wakes the devices and answers each.
pub(crate) fn writer(db: Arc<Db>, queue: std::sync::mpsc::Receiver<Delivery>, most: u32, hub: Weak<Hub>) {
    while let Ok(first) = queue.recv() {
        let mut batch = vec![first];
        batch.extend(queue.try_iter().take(BATCH - 1));
        let items: Vec<Incoming<'_>> = batch
            .iter()
            .map(|d| Incoming { user: d.user, devices_hash: d.devices_hash, envelope: &d.envelope })
            .collect();
        let hub = hub.upgrade();
        let now = hub.as_ref().map_or_else(now_ms, |hub| hub.clock_ms());
        match db.deliver(&items, now, most) {
            Ok(outcomes) => {
                for (delivery, mut outcome) in batch.into_iter().zip(outcomes) {
                    // Here, not in the request: one cut off after its envelope was stored still wakes and pushes.
                    if let (Delivered::Stored(devices), Some(hub)) = (&mut outcome, &hub) {
                        // Counted here too: a bot's inbox apart (every device a bot's), nothing of whose.
                        let bot = !devices.is_empty() && devices.iter().all(|w| w.bot);
                        hub.count(if bot { crate::Event::YoInBot } else { crate::Event::YoIn }, 1);
                        hub.woken(std::mem::take(devices), delivery.urgent, &delivery.runtime);
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

    /// What follows a stored delivery: its devices' long polls answer, and each is pushed (spec 6.7).
    fn woken(self: &Arc<Self>, devices: Vec<db::Wake>, urgent: bool, runtime: &tokio::runtime::Handle) {
        for wake in devices {
            if let Some(notify) = self.waiting.lock().unwrap_or_else(PoisonError::into_inner).get(&wake.device_id) {
                notify.notify_waiters();
            }
            if !wake.push_token.is_empty() {
                runtime.spawn(self.clone().push(wake, urgent));
            }
        }
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

/// `GET /v1/inbox?wait=s`: the device's envelopes, oldest first, waiting up to `wait` seconds for one.
pub(crate) async fn get_inbox(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap) -> Res<Response> {
    let wait_s = query::<u64>(&uri, "wait").unwrap_or(0).min(limits::MAX_WAIT_S);
    let device = hub.blocking(move |hub| hub.authenticate(&method, &uri, &headers, &[])).await??;
    let fetching = hub.fetching(device.device_id).ok_or_else(|| refuse(StatusCode::TOO_MANY_REQUESTS))?;
    let id = device.device_id;
    let read =
        move |hub: &Hub| Ok::<_, rusqlite::Error>((hub.db.fetch(&id, MAX_INBOX_FETCH)?, hub.db.prekeys_left(&id)?));
    let (mut items, mut prekeys_left) = hub.blocking(read).await??;
    if items.is_empty() && wait_s > 0 {
        let waiter = hub.waiter(id);
        let notified = waiter.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        // Mail that came between the first fetch and `enable` is found here.
        (items, prekeys_left) = hub.blocking(read).await??;
        if items.is_empty() {
            let _ = tokio::time::timeout(Duration::from_secs(wait_s), notified).await;
            (items, prekeys_left) = hub.blocking(read).await??;
        }
    }
    let items = items
        .into_iter()
        .map(|(id, envelope, received_ms)| api::InboxItem { id, received_ms, envelope: envelope.into() })
        .collect();
    Ok(held(encode(&api::Inbox { prekeys_left, items }), fetching))
}

/// `POST /v1/inbox/ack`: the envelopes the device has stored.
pub(crate) async fn post_ack(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let request: api::Ack = decode(&body)?;
    if request.ids.len() > limits::MAX_ACK {
        return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
    }
    let acked = hub.db.ack(&device.device_id, &request.ids)?;
    hub.count_acked(&acked, hub.clock_ms());
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/inbox/{user}`: an envelope for every device of a user, if the sender's view of them is the hub's (4.4).
/// Sealed: no identity, but for a request (spec 4.10).
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
    yo_wire::envelope::check(&body, headers.contains_key(HEADER_DEVICE))?;
    // Limited by its address only, and by the bytes it stores (6.6). An `Authorization` header is ignored.
    hub.limit_ip("deliver", peer, &headers, hub.config.limits.deliver_per_ip_minute, MINUTE)
        .map_err(|_| error::limited("ip", None))?;
    hub.limit_ip_bytes(peer, &headers, body.len())?;

    // Whose it is (spec 4.10): a contact's (the profile key shown, or a group send token, 4.9), else a request,
    // which its sending device signs; unsigned it is `403`.
    let sender = if headers.contains_key(HEADER_DEVICE) {
        let (uri, headers, body) = (uri.clone(), headers.clone(), body.clone());
        Some(hub.blocking(move |hub| hub.authenticate(&method, &uri, &headers, &body)).await??.user_id)
    } else {
        None
    };
    let shown = header(&headers, api::HEADER_ACCESS).and_then(b64_array).map(|key| api::access_check(&key));
    let endorsed = header(&headers, api::HEADER_GROUP_SEND).is_some_and(|token| hub.endorsed(&user, token));
    let request = if endorsed {
        None
    } else {
        match hub.blocking(move |hub| hub.db.admit(&user, shown, sender.as_ref())).await?? {
            db::Admission::Contact => None,
            db::Admission::Request => sender,
            db::Admission::Closed => return Err(refuse(StatusCode::FORBIDDEN)),
        }
    };
    // A request past its buckets waits, for this recipient only (`429` with `Retry-After`).
    if let Some(sender) = &request {
        hub.requests.check(sender, &user, hub.clock_ms()).map_err(|(scope, ms)| error::limited(scope, Some(ms)))?;
    }
    hub.db_room()?;

    // To the writer; full (it is behind) or gone: "later", the envelope stays with its sender. Never waits here.
    let (stored, outcome) = tokio::sync::oneshot::channel();
    let runtime = tokio::runtime::Handle::current();
    hub.deliveries
        .try_send(Delivery { user, devices_hash, envelope: body.clone(), urgent, stored, runtime })
        .map_err(|_| refuse(StatusCode::SERVICE_UNAVAILABLE))?;
    let outcome = outcome.await.ok().and_then(Result::ok).ok_or_else(|| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
    match outcome {
        Delivered::Stored(_) => {
            if let Some(sender) = request {
                hub.requests.take(&sender, &user, hub.clock_ms());
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
