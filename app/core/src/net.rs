//! The core talks to the hub through the platform's HTTP stack: URLSession on iOS, OkHttp on Android, reqwest in
//! tests. The core decides what to send; the platform only carries it.

/// An HTTP answer.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// One HTTP request to the hub, relative to its base URL. Blocking: call the core off the main thread.
/// An `Err` is a transport failure (no answer); any answer, whatever its status, is `Ok`. A failure that starts with
/// `CLOCK_FAILURE` is a TLS handshake that refused the hub's certificate for its dates: the phone's clock is off.
pub trait Transport: Send + Sync {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String>;

    /// `request` with the answer's headers (lowercase names). Only the name check reads one (`Yo-Name-Min`); a
    /// transport that cannot see headers answers none.
    fn request_headers(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(Response, Vec<(String, String)>), String> {
        self.request(method, path, headers, body).map(|response| (response, Vec::new()))
    }

    /// Whether every answer is the hub's and fresh by itself: Oblivious HTTP to a pinned key (`ohttp::Ohttp`, spec
    /// 4.1). Answers handing out keys are then taken without the hub's signature (`Yo-Nonce`).
    fn authenticated(&self) -> bool {
        false
    }
}

/// How a transport's failure starts when TLS refused the hub's certificate for its dates (`Transport`).
pub const CLOCK_FAILURE: &str = "clock:";
