use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// A refusal: a status and, for some, a TLS-encoded body (a `409` names the epoch).
#[derive(Debug)]
pub struct Refusal(pub StatusCode, pub Vec<u8>);

pub type Res<T> = Result<T, Refusal>;

pub fn refuse(status: StatusCode) -> Refusal {
    Refusal(status, Vec::new())
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
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
