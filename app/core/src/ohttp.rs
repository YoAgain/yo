//! Oblivious HTTP (spec 4.1, RFC 9458 with Binary HTTP, RFC 9292): a `Transport` that wraps every request to the
//! hub's pinned OHTTP key and posts it to `/v1/ohttp` over the platform's transport; blob data goes direct, by a
//! one-time ticket asked for inside OHTTP. Anonymous and signed requests keep their separate connections: each goes
//! over its own inner transport (`signed`, `anonymous`), which must never share a connection pool.
//! A failing OHTTP path never falls back to direct requests, and a wrapped request is never sent twice by itself.

use crate::net::{Response, Transport};
use std::sync::Arc;
use yoip::net::{api, limits};

/// The OHTTP key configs apps pin, per hub (`yo-hub ohttp public`): the one in use first, then the spare.
/// The hub holds the private key in use (key id 1); the spare (key id 2) stays offline until a rotation.
pub const PINNED: &[(&str, [&str; 2])] = &[
    (
        "hub.justyo.app",
        [
            "AQAg_zAjtMn6p4tcnHm8nuP7Uuf0KUxuWKxjOaEE-jiA8SYABAABAAE",
            "AgAglUWOVedJ-uCGXmHyh_FMTWJ4-z_KX37XsT_uOuM2CwcABAABAAE",
        ],
    ),
    (
        "dev.justyo.app",
        [
            "AQAgc4Nmtb9Xetv3JRWgyK84H-y3a_Laq66Y7BxAhcw3oXAABAABAAE",
            "AgAgGWGVbRyIX8vtfoUSd_8t9HhZpE0iOsZIUia7LfkL3VoABAABAAE",
        ],
    ),
];

/// The pinned configs of a hub's host, decoded; none for a hub not listed.
pub fn pinned(host: &str) -> Vec<Vec<u8>> {
    let host = host.trim_start_matches("https://").trim_start_matches("http://").split(['/', ':']).next().unwrap_or("");
    PINNED
        .iter()
        .filter(|(h, _)| *h == host)
        .flat_map(|(_, configs)| configs.iter().filter_map(|c| yoip::unb64(c).ok()))
        .collect()
}

pub struct Ohttp {
    signed: Arc<dyn Transport>,
    anonymous: Arc<dyn Transport>,
    /// Pinned key configs (`application/ohttp-keys` items), current first.
    configs: Vec<Vec<u8>>,
}

impl Ohttp {
    pub fn new(signed: Arc<dyn Transport>, anonymous: Arc<dyn Transport>, configs: Vec<Vec<u8>>) -> Ohttp {
        Ohttp { signed, anonymous, configs }
    }

    /// Which connection a request takes: signed, or (naming nobody) anonymous (spec 1.1), as the direct transports.
    fn line(&self, method: &str, path: &str, headers: &[(String, String)]) -> &dyn Transport {
        let signed = headers.iter().any(|(name, _)| name.eq_ignore_ascii_case(yoip::net::request::HEADER_DEVICE));
        match !signed && api::anonymous(method, path) {
            true => self.anonymous.as_ref(),
            false => self.signed.as_ref(),
        }
    }

    /// One request wrapped, sent and its answer unwrapped. A `422` (key id unknown: not processed) tries the spare.
    fn wrapped(
        &self,
        line: &dyn Transport,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(Response, Vec<(String, String)>), String> {
        if self.configs.is_empty() {
            return Err("no OHTTP key pinned for this hub".into());
        }
        let plain = encode_request(method, path, headers, body)?;
        let outer = [("content-type".to_owned(), api::OHTTP_REQUEST_TYPE.to_owned())];
        for config in &self.configs {
            let client = ::ohttp::ClientRequest::from_encoded_config(config).map_err(|e| format!("ohttp: {e}"))?;
            let (request, opener) = client.encapsulate(&plain).map_err(|e| format!("ohttp: {e}"))?;
            let response = line.request("POST", api::OHTTP, &outer, &request)?;
            match response.status {
                200 => {
                    let plain = opener.decapsulate(&response.body).map_err(|_| "ohttp: the answer does not open")?;
                    return decode_response(&plain);
                }
                422 => continue,
                // Refused before it was unwrapped: the proxy's or the hub's limits, a broken request.
                _ => return Ok((response, Vec::new())),
            }
        }
        Err("ohttp: the hub knows none of the pinned keys".into())
    }
}

/// A request as Binary HTTP with its `Date` (spec 4.1: replay), zero padded so that wrapped it fills a bucket.
pub fn encode_request(method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Vec<u8>, String> {
    let mut message = ::bhttp::Message::request(method.into(), b"https".to_vec(), Vec::new(), path.into());
    for (name, value) in headers {
        message.put_header(name.to_ascii_lowercase(), value.as_bytes());
    }
    message.put_header("date", ::httpdate::fmt_http_date(std::time::SystemTime::now()));
    message.write_content(body);
    let mut out = Vec::new();
    message.write_bhttp(::bhttp::Mode::KnownLength, &mut out).map_err(|e| format!("bhttp: {e}"))?;
    let padded = limits::ohttp_padded(out.len(), limits::OHTTP_REQUEST_OVERHEAD).ok_or("request too large")?;
    out.resize(padded, 0);
    Ok(out)
}

fn decode_response(plain: &[u8]) -> Result<(Response, Vec<(String, String)>), String> {
    let mut cursor = std::io::Cursor::new(plain);
    let message = ::bhttp::Message::read_bhttp(&mut cursor).map_err(|_| "bhttp: the answer does not parse")?;
    let status = message.control().status().map(|s| s.code()).ok_or("bhttp: no status")?;
    let headers = message
        .header()
        .iter()
        .map(|f| {
            (String::from_utf8_lossy(f.name()).to_ascii_lowercase(), String::from_utf8_lossy(f.value()).into_owned())
        })
        .collect();
    Ok((Response { status, body: message.content().to_vec() }, headers))
}

impl Transport for Ohttp {
    fn authenticated(&self) -> bool {
        true
    }

    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        self.request_headers(method, path, headers, body).map(|(response, _)| response)
    }

    fn request_headers(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(Response, Vec<(String, String)>), String> {
        let line = self.line(method, path, headers);
        let blob = path.starts_with(&format!("{}/", api::BLOBS));
        if !(blob && matches!(method, "PUT" | "GET")) {
            return self.wrapped(line, method, path, headers, body);
        }
        // Media (spec 4.1): a ticket inside OHTTP for this blob and direction, then the transfer direct.
        let mut asking = headers.to_vec();
        asking.push((api::HEADER_TICKET.to_owned(), "1".to_owned()));
        if method == "PUT" {
            asking.push((api::HEADER_BLOB_LENGTH.to_owned(), body.len().to_string()));
        }
        let (answer, answer_headers) = self.wrapped(line, method, path, &asking, &[])?;
        if answer.status != 200 {
            return Ok((answer, answer_headers));
        }
        let ticket = std::str::from_utf8(&answer.body).map_err(|_| "ohttp: no ticket")?;
        let response = line.request(method, &api::ticket_path(ticket), &[], body)?;
        Ok((response, Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_pinned_configs_decode_with_the_suite() {
        for host in ["hub.justyo.app", "https://dev.justyo.app/"] {
            let configs = super::pinned(host);
            assert_eq!(configs.len(), 2);
            for config in configs {
                assert!(::ohttp::ClientRequest::from_encoded_config(&config).is_ok());
                // Key id, X25519 (0x0020), its 32-byte key, one suite: HKDF-SHA256 (1), AES-128-GCM (1).
                assert_eq!((config.len(), &config[1..3], &config[35..]), (41, &[0, 0x20][..], &[0, 4, 0, 1, 0, 1][..]));
            }
        }
        assert!(super::pinned("example.org").is_empty());
    }
}
