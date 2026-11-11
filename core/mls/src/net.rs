//! The core talks to the hub through the platform's HTTP stack: URLSession on iOS, OkHttp on Android, reqwest in
//! tests. The core decides what to send; the platform only carries it.

/// An HTTP answer.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// One HTTP request to the hub, relative to its base URL. Blocking: call the core off the main thread.
/// An `Err` is a transport failure (no answer); any answer, whatever its status, is `Ok`.
pub trait Transport: Send + Sync {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String>;
}
