//! Waking a device (spec 6.7). A push says only "there is mail": APNs with a constant alert and
//! `mutable-content`, so the notification extension fetches, decrypts and writes "YO from CHRIS"; FCM as a
//! high-priority data message. The requests go through `Transport`, so tests record them.

use crate::config::{ApnsConfig, FcmConfig};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::json;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use yo_wire::api::{PUSH_APNS, PUSH_APNS_SANDBOX, PUSH_FCM};

pub type Fut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Sends a request: the status and the body of the answer.
pub trait Transport: Send + Sync {
    fn send(&self, request: Request) -> Fut<Result<(u16, Vec<u8>), String>>;
}

/// Sends over HTTPS (HTTP/2 where the server speaks it, as APNs requires).
pub struct Https(reqwest::Client);

impl Default for Https {
    fn default() -> Self {
        Https(reqwest::Client::builder().timeout(Duration::from_secs(15)).build().expect("HTTP client"))
    }
}

impl Transport for Https {
    fn send(&self, request: Request) -> Fut<Result<(u16, Vec<u8>), String>> {
        let mut builder = self.0.post(&request.url).body(request.body);
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        Box::pin(async move {
            let response = builder.send().await.map_err(|e| e.to_string())?;
            let status = response.status().as_u16();
            let body = response.bytes().await.map_err(|e| e.to_string())?.to_vec();
            Ok((status, body))
        })
    }
}

/// Records every request and answers like the services would: tests, and a hub without push services.
#[derive(Default)]
pub struct Recorder {
    pub sent: Mutex<Vec<Request>>,
    /// The status pushes to devices are answered with; 200 when unset.
    pub device_status: Mutex<Option<u16>>,
}

impl Transport for Recorder {
    fn send(&self, request: Request) -> Fut<Result<(u16, Vec<u8>), String>> {
        let answer = if request.url.contains("oauth2") {
            (200, br#"{"access_token":"test","expires_in":3600}"#.to_vec())
        } else {
            (self.device_status.lock().unwrap_or_else(PoisonError::into_inner).unwrap_or(200), Vec::new())
        };
        self.sent.lock().unwrap_or_else(PoisonError::into_inner).push(request);
        Box::pin(async move { Ok(answer) })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    /// The service says the token is dead: forget it.
    TokenInvalid,
    Failed(String),
    /// No service configured for the platform, or no token.
    Skipped,
}

pub struct Pusher {
    transport: Arc<dyn Transport>,
    apns: Option<Apns>,
    fcm: Option<Fcm>,
}

struct Cached {
    value: String,
    until: Instant,
}

struct Apns {
    config: ApnsConfig,
    key: EncodingKey,
    jwt: Mutex<Option<Cached>>,
}

struct Fcm {
    project_id: String,
    client_email: String,
    token_uri: String,
    key: EncodingKey,
    /// Async: concurrent pushes wait for one token exchange instead of each running their own.
    access: tokio::sync::Mutex<Option<Cached>>,
}

fn unix_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

impl Pusher {
    pub fn new(
        transport: Arc<dyn Transport>,
        apns: Option<&ApnsConfig>,
        fcm: Option<&FcmConfig>,
    ) -> Result<Self, String> {
        let apns = apns
            .map(|config| {
                Ok::<_, String>(Apns {
                    key: EncodingKey::from_ec_pem(config.key_pem.as_bytes()).map_err(|e| format!("APNs key: {e}"))?,
                    config: config.clone(),
                    jwt: Mutex::new(None),
                })
            })
            .transpose()?;
        let fcm = fcm
            .map(|config| {
                let account: serde_json::Value = serde_json::from_str(&config.service_account_json)
                    .map_err(|e| format!("FCM service account: {e}"))?;
                let field = |name: &str| {
                    account[name].as_str().map(str::to_owned).ok_or(format!("FCM service account: no {name}"))
                };
                Ok::<_, String>(Fcm {
                    project_id: field("project_id")?,
                    client_email: field("client_email")?,
                    token_uri: field("token_uri")?,
                    key: EncodingKey::from_rsa_pem(field("private_key")?.as_bytes())
                        .map_err(|e| format!("FCM key: {e}"))?,
                    access: tokio::sync::Mutex::new(None),
                })
            })
            .transpose()?;
        Ok(Pusher { transport, apns, fcm })
    }

    pub async fn wake(&self, platform: u8, token: &[u8]) -> Outcome {
        let Ok(token) = std::str::from_utf8(token) else { return Outcome::TokenInvalid };
        if token.is_empty() {
            return Outcome::Skipped;
        }
        match platform {
            // An APNs device token is hex; anything else would go into the request's path (`apns`).
            PUSH_APNS | PUSH_APNS_SANDBOX if !token.bytes().all(|b| b.is_ascii_hexdigit()) => Outcome::TokenInvalid,
            PUSH_APNS | PUSH_APNS_SANDBOX => match &self.apns {
                Some(apns) => self.apns(apns, platform == PUSH_APNS_SANDBOX, token).await,
                None => Outcome::Skipped,
            },
            PUSH_FCM => match &self.fcm {
                Some(fcm) => self.fcm(fcm, token).await,
                None => Outcome::Skipped,
            },
            _ => Outcome::Skipped,
        }
    }

    async fn apns(&self, apns: &Apns, sandbox: bool, token: &str) -> Outcome {
        let jwt = {
            let mut cached = apns.jwt.lock().unwrap_or_else(PoisonError::into_inner);
            match cached.as_ref().filter(|c| c.until > Instant::now()) {
                Some(c) => c.value.clone(),
                None => {
                    let mut header = Header::new(Algorithm::ES256);
                    header.kid = Some(apns.config.key_id.clone());
                    let claims = json!({ "iss": apns.config.team_id, "iat": unix_s() });
                    let Ok(value) = jsonwebtoken::encode(&header, &claims, &apns.key) else {
                        return Outcome::Failed("APNs token".into());
                    };
                    // Apple accepts a token for an hour and refuses one renewed more than every 20 minutes.
                    *cached =
                        Some(Cached { value: value.clone(), until: Instant::now() + Duration::from_secs(40 * 60) });
                    value
                }
            }
        };
        let host = if sandbox { "api.sandbox.push.apple.com" } else { "api.push.apple.com" };
        let body =
            json!({ "aps": { "alert": { "title": "Yo", "body": "Yo" }, "sound": "yo.caf", "mutable-content": 1 } });
        let request = Request {
            url: format!("https://{host}/3/device/{token}"),
            headers: vec![
                ("authorization".into(), format!("bearer {jwt}")),
                ("apns-topic".into(), apns.config.topic.clone()),
                ("apns-push-type".into(), "alert".into()),
                ("apns-priority".into(), "10".into()),
                ("content-type".into(), "application/json".into()),
            ],
            body: body.to_string().into_bytes(),
        };
        match self.transport.send(request).await {
            Ok((200, _)) => Outcome::Sent,
            Ok((410, _)) => Outcome::TokenInvalid,
            Ok((400, body)) if String::from_utf8_lossy(&body).contains("BadDeviceToken") => Outcome::TokenInvalid,
            Ok((status, body)) => Outcome::Failed(format!("APNs {status} {}", String::from_utf8_lossy(&body))),
            Err(error) => Outcome::Failed(error),
        }
    }

    async fn fcm_access(&self, fcm: &Fcm) -> Result<String, String> {
        let mut access = fcm.access.lock().await;
        if let Some(c) = access.as_ref().filter(|c| c.until > Instant::now()) {
            return Ok(c.value.clone());
        }
        let now = unix_s();
        let claims = json!({
            "iss": fcm.client_email,
            "scope": "https://www.googleapis.com/auth/firebase.messaging",
            "aud": fcm.token_uri,
            "iat": now,
            "exp": now + 3600,
        });
        let assertion = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &fcm.key)
            .map_err(|e| format!("FCM assertion: {e}"))?;
        let request = Request {
            url: fcm.token_uri.clone(),
            headers: vec![("content-type".into(), "application/x-www-form-urlencoded".into())],
            body: format!("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={assertion}")
                .into_bytes(),
        };
        let (status, body) = self.transport.send(request).await?;
        let answer: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        let token = answer["access_token"].as_str().filter(|_| status == 200).ok_or(format!("FCM OAuth {status}"))?;
        let lifetime = answer["expires_in"].as_u64().unwrap_or(3600).saturating_sub(300);
        *access = Some(Cached { value: token.to_owned(), until: Instant::now() + Duration::from_secs(lifetime) });
        Ok(token.to_owned())
    }

    async fn fcm(&self, fcm: &Fcm, token: &str) -> Outcome {
        let access = match self.fcm_access(fcm).await {
            Ok(access) => access,
            Err(error) => return Outcome::Failed(error),
        };
        let body = json!({ "message": { "token": token, "data": { "yo": "1" }, "android": { "priority": "HIGH" } } });
        let request = Request {
            url: format!("https://fcm.googleapis.com/v1/projects/{}/messages:send", fcm.project_id),
            headers: vec![
                ("authorization".into(), format!("Bearer {access}")),
                ("content-type".into(), "application/json".into()),
            ],
            body: body.to_string().into_bytes(),
        };
        match self.transport.send(request).await {
            Ok((200, _)) => Outcome::Sent,
            Ok((404, _)) => Outcome::TokenInvalid,
            Ok((400, body)) if String::from_utf8_lossy(&body).contains("INVALID_ARGUMENT") => Outcome::TokenInvalid,
            Ok((status, body)) => Outcome::Failed(format!("FCM {status} {}", String::from_utf8_lossy(&body))),
            Err(error) => Outcome::Failed(error),
        }
    }
}
