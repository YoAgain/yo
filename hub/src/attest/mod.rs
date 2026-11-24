//! Device attestation (spec 6.11). Only the operator sees the measurements.

mod android;
mod ios;

use crate::error::{refuse, Res};
use crate::request::{bad, Auth, Hubs, HOUR};
use axum::extract::State;
use axum::http::{header::CACHE_CONTROL, StatusCode};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::sync::PoisonError;
use yo_wire::{api, decode, encode, DeviceId};

type Challenge = [u8; api::ATTEST_CHALLENGE_LEN];
#[derive(Default)]
pub(crate) struct Challenges(HashMap<DeviceId, (Challenge, u64)>);

impl Challenges {
    fn issue(&mut self, id: DeviceId, now: u64) -> Res<Challenge> {
        self.0.retain(|_, (_, expires)| *expires > now);
        if self.0.len() >= 8192 && !self.0.contains_key(&id) {
            if let Some(oldest) = self.0.iter().min_by_key(|(_, (_, expiry))| expiry).map(|(id, _)| *id) {
                self.0.remove(&oldest);
            }
        }
        let mut challenge = [0; api::ATTEST_CHALLENGE_LEN];
        getrandom::fill(&mut challenge).map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
        self.0.insert(id, (challenge, now + api::ATTEST_CHALLENGE_TTL_MS));
        Ok(challenge)
    }

    fn take(&mut self, id: &DeviceId, now: u64) -> Option<Challenge> {
        self.0.remove(id).filter(|(_, expiry)| *expiry > now).map(|(c, _)| c)
    }
}

pub(crate) async fn challenge(State(hub): Hubs, Auth(device, _): Auth) -> Res<Response> {
    if !hub.config.attest {
        return Ok((StatusCode::NO_CONTENT, [(CACHE_CONTROL, "no-store")]).into_response());
    }
    hub.limit_signed(&device, "attest", &device.device_id, 20, HOUR)?;
    let challenge =
        hub.challenges.lock().unwrap_or_else(PoisonError::into_inner).issue(device.device_id, hub.clock_ms())?;
    Ok(([(CACHE_CONTROL, "no-store")], encode(&api::AttestChallenge { challenge: challenge.to_vec().into() }))
        .into_response())
}

pub(crate) async fn submit(State(hub): Hubs, Auth(device, body): Auth) -> Res<Vec<u8>> {
    let answer = |verdict| encode(&api::AttestResult { verdict });
    if !hub.config.attest {
        return Ok(answer(api::ATTEST_UNJUDGED));
    }
    let now = hub.clock_ms();
    let challenge = hub.challenges.lock().unwrap_or_else(PoisonError::into_inner).take(&device.device_id, now);
    let att: api::Attest = decode(&body)?;
    if att.payload.as_slice().len() > api::MAX_ATTEST_PAYLOAD {
        return Err(bad());
    }
    let platform = match att.platform {
        api::ATTEST_IOS => "ios",
        api::ATTEST_ANDROID => "android",
        _ => return Err(bad()),
    };
    let Some(challenge) = challenge else { return Ok(answer(api::ATTEST_UNJUDGED)) };
    let mut level = None;
    let result = if att.platform == api::ATTEST_IOS {
        ios::verify(att.payload.as_slice(), &yo_wire::sha256(&challenge), &hub.config.apple_app_ids, ios::ROOT, now)
    } else {
        let Some(keys) = &hub.config.play_keys else { return Ok(answer(api::ATTEST_UNJUDGED)) };
        android::verify(att.payload.as_slice(), keys, &challenge, &hub.config.play_package, now, &mut level)
    };
    if let Err(reason) = result {
        let app = hub.db.device(&device.device_id)?.and_then(|d| d.app);
        let payload = serde_json::json!({"severity":"expected","domain":"attest","code":reason,"place":platform,"app":app.as_deref().unwrap_or("hub")});
        hub.db.diagnostic("error", &payload, None, now)?;
    }
    hub.db.record_attestation(&device.device_id, platform, result.err(), level, now)?;
    Ok(answer(if result.is_ok() { api::ATTEST_ATTESTED } else { api::ATTEST_FAILED }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, push, request::Signed, Hub};
    use bytes::Bytes;
    use std::sync::Arc;

    #[test]
    fn replaces_consumes_and_expires_challenges() {
        let mut challenges = Challenges::default();
        let id = [0; 16];
        let first = challenges.issue(id, 0).unwrap();
        let second = challenges.issue(id, 1).unwrap();
        assert_ne!(first, second);
        assert!(challenges.take(&[1; 16], 2).is_none());
        assert_eq!(challenges.take(&id, 2), Some(second));
        assert!(challenges.take(&id, 3).is_none());
        challenges.issue(id, 10).unwrap();
        assert!(challenges.take(&id, 10 + api::ATTEST_CHALLENGE_TTL_MS).is_none());
    }

    #[tokio::test]
    async fn switch_and_missing_keys_leave_no_measurement() {
        let auth = |body: Vec<u8>| Auth(Signed { device_id: [0; 16], user_id: [0; 16] }, Bytes::from(body));
        for enabled in [false, true] {
            let hub = Hub::new(Config { attest: enabled, ..Config::for_tests() }, Arc::new(push::Recorder::default()))
                .unwrap();
            let response = challenge(State(hub.clone()), auth(vec![])).await.unwrap();
            let status = response.status();
            let challenge = axum::body::to_bytes(response.into_body(), 128).await.unwrap();
            let body = if enabled {
                assert_eq!(status, StatusCode::OK);
                assert_eq!(decode::<api::AttestChallenge>(&challenge).unwrap().challenge.as_slice().len(), 32);
                encode(&api::Attest { platform: api::ATTEST_ANDROID, payload: vec![1].into() })
            } else {
                assert_eq!(status, StatusCode::NO_CONTENT);
                assert!(challenge.is_empty());
                vec![255; api::MAX_ATTEST_PAYLOAD + 1]
            };
            let response = submit(State(hub.clone()), auth(body)).await.unwrap();
            assert_eq!(decode::<api::AttestResult>(&response).unwrap().verdict, api::ATTEST_UNJUDGED);
            assert!(!hub.db.conn().prepare("SELECT 1 FROM attestation").unwrap().exists([]).unwrap());
        }
    }
    #[tokio::test]
    async fn signed_routes_bind_one_submission_and_report_failures() {
        use axum::body::{to_bytes, Body};
        use axum::http::Request;
        use tower::ServiceExt;
        use yo_wire::{b64, request};

        let hub =
            Hub::new(Config { attest: true, ..Config::for_tests() }, Arc::new(push::Recorder::default())).unwrap();
        let pair = libsignal_protocol::KeyPair::generate(&mut rand::rng());
        let device = crate::db::NewDevice {
            device_id: rand::random(),
            device_key: rand::random(),
            auth_key: pair.public_key.public_key_bytes().try_into().unwrap(),
            registration_id: 1,
            push_platform: 0,
            push_token: vec![],
            app: None,
            version: None,
        };
        let prekeys = api::Prekeys { signed: None, last_resort: None, one_time: vec![], kyber: vec![] };
        hub.db
            .register(b"test", "test", &rand::random(), &rand::random(), &device, &prekeys, 10, 0)
            .unwrap()
            .done()
            .unwrap();
        let send = |method: &str, path: &str, body: Vec<u8>| {
            let time = crate::now_ms();
            let sig = pair
                .private_key
                .calculate_signature(&request::to_be_signed(method, path, time, &body), &mut rand::rng())
                .unwrap();
            let req = Request::builder()
                .method(method)
                .uri(path)
                .header(request::HEADER_DEVICE, b64(&device.device_id))
                .header(request::HEADER_TIME, time.to_string())
                .header(request::HEADER_SIGNATURE, b64(&sig))
                .header("Yo-App", "ios/2")
                .body(Body::from(body))
                .unwrap();
            hub.router().oneshot(req)
        };
        let issued = send("GET", api::ATTEST_CHALLENGE, vec![]).await.unwrap();
        assert_eq!(issued.status(), StatusCode::OK);
        assert_eq!(issued.headers()[CACHE_CONTROL], "no-store");
        let body = |platform, payload: Vec<u8>| encode(&api::Attest { platform, payload: payload.into() });
        for verdict in [api::ATTEST_FAILED, api::ATTEST_UNJUDGED] {
            let response = send("POST", api::ATTEST, body(api::ATTEST_IOS, vec![0])).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                decode::<api::AttestResult>(&to_bytes(response.into_body(), 64).await.unwrap()).unwrap().verdict,
                verdict
            );
        }
        for malformed in [body(99, vec![]), body(api::ATTEST_IOS, vec![0; api::MAX_ATTEST_PAYLOAD + 1])] {
            assert_eq!(send("POST", api::ATTEST, malformed).await.unwrap().status(), StatusCode::BAD_REQUEST);
        }
        let payload: String = hub
            .db
            .conn()
            .query_row("SELECT payload FROM diagnostics WHERE kind='error' AND count=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&payload).unwrap(),
            serde_json::json!(
                {"domain":"attest","code":"format","severity":"expected","place":"ios","app":"ios/2"}
            )
        );
    }
}
