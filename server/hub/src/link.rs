//! Linking a device (spec 2.5): the new device opens a link, the old one leaves a sealed bundle there, the new one
//! takes it once. Held in memory only (a restart ends the links open), never logged; the hub sees a link id, a time and
//! ciphertext. Only on a hub that adds devices (`YO_ADD_DEVICES`); elsewhere every route is `404`.

use crate::error::{refuse, Res};
use crate::request::{path_id, query, Auth, Hubs, Peer};
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use yoip::net::{api, limits};

/// The most links open at once: beyond, `503`.
const MOST_OPEN: usize = 10_000;

struct Link {
    until: u64,
    bundle: Option<Vec<u8>>,
    arrived: Arc<Notify>,
}

/// The links open, by id.
#[derive(Default)]
pub(crate) struct Links(Mutex<HashMap<[u8; 16], Link>>);

impl Links {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<[u8; 16], Link>> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Ends the links past their time, with what they held.
    pub(crate) fn forget_expired(&self, now: u64) {
        self.lock().retain(|_, link| link.until > now);
    }
}

fn enabled(hub: &crate::Hub) -> Res<()> {
    match hub.config.add_devices {
        true => Ok(()),
        false => Err(refuse(StatusCode::NOT_FOUND)),
    }
}

/// `POST /v1/link`: a new link id, open `LINK_MS`. Limited per address as a registration is.
pub(crate) async fn post_link(
    axum::extract::State(hub): Hubs,
    axum::extract::ConnectInfo(peer): Peer,
    headers: HeaderMap,
) -> Res<Response> {
    enabled(&hub)?;
    hub.limit_expensive(peer, &headers)?;
    let mut id = [0u8; 16];
    getrandom::fill(&mut id).map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
    let now = hub.clock_ms();
    let mut links = hub.links.lock();
    links.retain(|_, link| link.until > now);
    if links.len() >= MOST_OPEN {
        return Err(refuse(StatusCode::SERVICE_UNAVAILABLE));
    }
    links.insert(id, Link { until: now + api::LINK_MS, bundle: None, arrived: Arc::new(Notify::new()) });
    Ok(yoip::b64(&id).into_response())
}

/// `PUT /v1/link/{id}`, signed: the sealed bundle, once.
pub(crate) async fn put_link(
    axum::extract::State(hub): Hubs,
    Path(id): Path<String>,
    Auth(_, body): Auth,
) -> Res<StatusCode> {
    enabled(&hub)?;
    let id: [u8; 16] = path_id(&id)?;
    if body.is_empty() || body.len() > api::MAX_LINK_BUNDLE {
        return Err(refuse(StatusCode::BAD_REQUEST));
    }
    let now = hub.clock_ms();
    let mut links = hub.links.lock();
    let link = links.get_mut(&id).filter(|link| link.until > now).ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    if link.bundle.is_some() {
        return Err(refuse(StatusCode::CONFLICT));
    }
    link.bundle = Some(body.to_vec());
    link.arrived.notify_waiters();
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/link/{id}?wait=S`: the bundle, taken once (the link ends with it); `204` none within the wait.
pub(crate) async fn get_link(
    axum::extract::State(hub): Hubs,
    axum::extract::ConnectInfo(peer): Peer,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Res<Response> {
    enabled(&hub)?;
    hub.limit_anonymous(peer, &headers)?;
    let id: [u8; 16] = path_id(&id)?;
    let wait_s = query::<u64>(&uri, "wait").unwrap_or(0).min(limits::MAX_WAIT_S);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_s);
    let arrived = {
        let links = hub.links.lock();
        links.get(&id).map(|link| link.arrived.clone()).ok_or_else(|| refuse(StatusCode::NOT_FOUND))?
    };
    loop {
        // Waiting before looking: a bundle put in between still wakes it.
        let notified = arrived.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
            let now = hub.clock_ms();
            let mut links = hub.links.lock();
            let Some(link) = links.get(&id).filter(|link| link.until > now) else {
                links.remove(&id);
                return Err(refuse(StatusCode::NOT_FOUND));
            };
            if link.bundle.is_some() {
                let bundle = links.remove(&id).and_then(|link| link.bundle).unwrap_or_default();
                return Ok(crate::request::octets(bundle.len() as u64, bundle.into()));
            }
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
    }
}
