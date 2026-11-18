use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// A refusal: a status and, for some, a TLS-encoded body (a delivery's `409` names the devices).
#[derive(Debug)]
pub struct Refusal(pub StatusCode, pub Vec<u8>);

pub type Res<T> = Result<T, Refusal>;

pub fn refuse(status: StatusCode) -> Refusal {
    Refusal(status, Vec::new())
}

/// `429` naming its limit (spec 6.6) in the body, and the seconds until a token as `Retry-After` where known.
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

/// A write the database refused: `410` a device gone, `429` a support thread full, else `409`.
impl From<crate::db::Conflict> for Refusal {
    fn from(conflict: crate::db::Conflict) -> Self {
        use crate::db::Conflict;
        refuse(match conflict {
            Conflict::DeviceGone => StatusCode::GONE,
            Conflict::SupportFull => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::CONFLICT,
        })
    }
}

impl From<yo_wire::Error> for Refusal {
    fn from(_: yo_wire::Error) -> Self {
        refuse(StatusCode::BAD_REQUEST)
    }
}
