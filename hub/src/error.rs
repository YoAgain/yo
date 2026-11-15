use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// A refusal: a status and, for some, a TLS-encoded body (a delivery's `409` names the devices).
#[derive(Debug)]
pub struct Refusal(pub StatusCode, pub Vec<u8>);

pub type Res<T> = Result<T, Refusal>;

pub fn refuse(status: StatusCode) -> Refusal {
    Refusal(status, Vec::new())
}

/// `429` naming the limit it is (spec 6.6), so a sender holds back only what shares it: `pair`, `new` or
/// `recipient` (a request's buckets, 4.10: this recipient only), `ip` or `bytes` (deliveries without a token),
/// `device` (this device's signed requests); with the seconds until a token where known, as `Retry-After` too. The
/// body says it (the apps' transport hands over the body only): "pair 3600".
pub fn limited(scope: &str, wait_ms: Option<u64>) -> Refusal {
    let body = match wait_ms {
        Some(ms) => format!("{scope} {}", ms.div_ceil(1000).max(1)),
        None => scope.to_owned(),
    };
    Refusal(StatusCode::TOO_MANY_REQUESTS, body.into_bytes())
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let body = String::from_utf8_lossy(&self.1).into_owned();
        if let Some(seconds) =
            body.split_once(' ').map(|(_, s)| s.to_owned()).filter(|_| self.0 == StatusCode::TOO_MANY_REQUESTS)
        {
            return (self.0, [(axum::http::header::RETRY_AFTER, seconds)], self.1).into_response();
        }
        (self.0, self.1).into_response()
    }
}

impl From<rusqlite::Error> for Refusal {
    fn from(error: rusqlite::Error) -> Self {
        eprintln!("yo-hub: database: {error}");
        refuse(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

impl From<yo_wire::Error> for Refusal {
    fn from(_: yo_wire::Error) -> Self {
        refuse(StatusCode::BAD_REQUEST)
    }
}
