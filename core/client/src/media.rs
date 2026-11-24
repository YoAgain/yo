//! Photos and videos (spec 5, 6.8): each sealed under its own key and uploaded as a blob; key, hash and size go in
//! the Yo. The hub never sees the key, the picture, or who fetches. Uploads and fetches never hold the client.

use crate::{Client, Error, Response, Result, Transport};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::ChaCha20Poly1305;
use std::sync::Arc;
use yo_wire::api::{self, status, HEADER_BODY_HASH};
use yo_wire::limits::{MAX_BLOB_FETCHES, MAX_BLOB_LEN};
use yo_wire::sha256;

pub use yo_wire::content::Media;

/// ChaCha20-Poly1305's tag.
const TAG: usize = 16;
/// The largest photo or video that fits in a blob.
pub const MAX_MEDIA_LEN: usize = MAX_BLOB_LEN - TAG;

/// Every key is used once: the nonce is zero.
const NONCE: [u8; 12] = [0; 12];

/// Seals a photo or video under a new key, with a new id: what the Yo carries, and the ciphertext to upload.
pub fn seal(plain: &[u8], width: u16, height: u16) -> Result<(Media, Vec<u8>)> {
    if plain.is_empty() || plain.len() > MAX_MEDIA_LEN {
        return Err(Error::Protocol("media size"));
    }
    let key: [u8; 32] = crate::identity::random();
    let ciphertext =
        ChaCha20Poly1305::new(&key.into()).encrypt(&NONCE.into(), plain).map_err(|_| Error::Internal("seal"))?;
    let media = Media {
        id: crate::identity::random(),
        key,
        sha256: sha256(&ciphertext),
        size: ciphertext.len() as u32,
        width,
        height,
    };
    Ok((media, ciphertext))
}

/// Opens a fetched blob: its size and hash must be the ones the Yo named, then it must decrypt.
pub fn open(media: &Media, ciphertext: &[u8]) -> Result<Vec<u8>> {
    if ciphertext.len() != media.size as usize || sha256(ciphertext) != media.sha256 {
        return Err(Error::Protocol("media hash"));
    }
    ChaCha20Poly1305::new(&media.key.into()).decrypt(&NONCE.into(), ciphertext).map_err(|_| Error::Protocol("media"))
}

/// Fetches a blob without the client or identity (spec 6.8) and checks it against the Yo. `None`: unavailable.
/// Call `ack` only after the opened file is durably stored.
pub fn fetch(transport: &dyn Transport, media: &Media) -> Result<Option<Vec<u8>>> {
    let response = transport.request("GET", &api::blob_path(&media.id), &[], &[]).map_err(Error::Network)?;
    match response.status {
        status::OK if response.body.len() == media.size as usize && sha256(&response.body) == media.sha256 => {
            Ok(Some(response.body))
        }
        status::OK => Err(Error::Protocol("media hash")),
        status::NOT_FOUND => Ok(None),
        other => Err(Error::Refused(other)),
    }
}

/// Acknowledges a durably stored blob anonymously. A lost answer can consume another fetch on retry.
pub fn ack(transport: &dyn Transport, id: &yo_wire::content::BlobId) -> Result<()> {
    let response = transport.request("DELETE", &api::blob_path(id), &[], &[]).map_err(Error::Network)?;
    match response.status {
        status::NO_CONTENT | status::NOT_FOUND => Ok(()),
        other => Err(Error::Refused(other)),
    }
}

/// The path of a blob, for a fetch by other means.
pub fn blob_path(id: &yo_wire::content::BlobId) -> String {
    api::blob_path(id)
}

/// The same blob without its key: what a client that forgets media keys keeps.
pub fn without_key(media: Media) -> Media {
    Media { key: [0; 32], ..media }
}

/// `fetch` and `open` in one.
pub fn fetch_open(transport: &dyn Transport, media: &Media) -> Result<Option<Vec<u8>>> {
    fetch(transport, media)?.map(|ciphertext| open(media, &ciphertext)).transpose()
}

/// A signed upload of a sealed blob, made under the client (`Client::upload`) and sent without it.
pub struct Upload {
    transport: Arc<dyn Transport>,
    path: String,
    headers: Vec<(String, String)>,
}

impl Upload {
    /// Sends the blob. `Refused(429)` and `Refused(507)` are "later": sign anew with `Client::upload` and send again.
    pub fn send(&self, ciphertext: &[u8]) -> Result<()> {
        let response: Response =
            self.transport.request("PUT", &self.path, &self.headers, ciphertext).map_err(Error::Network)?;
        match response.status {
            status::CREATED => Ok(()),
            // The id is random: a blob with it is this one, stored by a try whose answer was lost.
            status::CONFLICT => Ok(()),
            status::GONE => Err(Error::Replaced),
            other => Err(Error::Refused(other)),
        }
    }
}

/// Whether an upload's refusal means "later": the core's one rule (`crate::retry_later`).
pub use crate::retry_later;

impl Client {
    /// Signs the upload of a sealed blob for `fetches` fetches; each try needs its own signature.
    pub fn upload(&mut self, media: &Media, fetches: u32) -> Result<Upload> {
        let path = api::blob_put_path(&media.id, fetches.clamp(1, MAX_BLOB_FETCHES));
        let (mut headers, _) = self.sign_hash("PUT", &path, &media.sha256);
        headers.push((HEADER_BODY_HASH.to_owned(), yo_wire::b64(&media.sha256)));
        self.persist()?;
        Ok(Upload { transport: self.transport.clone(), path, headers })
    }

    /// Drops the keys of received media before any event is kept, for a client that never opens them (YoGPT).
    pub fn forget_media_keys(&mut self) {
        self.media_keys_forgotten = true;
    }

    /// The transport, for fetches that need no client (`fetch`).
    pub fn transport(&self) -> Arc<dyn Transport> {
        self.transport.clone()
    }

    /// Starts writing to `to` as the first send would and answers how many fetches media to them is for: the upload
    /// needs their device before the Yo can go.
    pub fn open_chat(&mut self, to: &str) -> Result<(u32, Vec<crate::Event>)> {
        let result = self.open_chat_inner(to);
        self.finish_with(result)
    }

    fn open_chat_inner(&mut self, to: &str) -> Result<u32> {
        if let Some(fetches) = self.media_fetches(to) {
            return Ok(fetches);
        }
        let handle = yo_wire::handle::Handle::parse(to)?;
        if self.is_me(&handle) {
            return Err(Error::Protocol("sending to oneself"));
        }
        // As a send: what waits goes and is received first; offline, nothing can be opened.
        match self.deliver().and_then(|()| self.receive_all(0)) {
            Err(error) if !crate::retryable(&error) => return Err(error),
            _ => {}
        }
        let user = self.send_target(&handle)?;
        self.check_sendable(user.as_deref())?;
        let user = match user.filter(|u| self.has_chat(u)) {
            Some(user) => user,
            None => self.start_chat(&handle)?,
        };
        self.fetches_for(&user).ok_or(Error::Internal("devices"))
    }

    /// The fetch count once this device can send media to the recipient.
    pub fn media_fetches(&self, to: &str) -> Option<u32> {
        let handle = yo_wire::handle::Handle::parse(to).ok()?;
        self.fetches_for(&self.known_user(&handle)?)
    }

    fn fetches_for(&self, user: &str) -> Option<u32> {
        let devices = &self.state.contacts.get(user)?.devices;
        (!devices.is_empty()).then_some(devices.len() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type Call = (String, Vec<(String, String)>);

    struct Wire {
        response: Response,
        calls: Mutex<Vec<Call>>,
    }

    impl Transport for Wire {
        fn request(
            &self,
            method: &str,
            _: &str,
            headers: &[(String, String)],
            body: &[u8],
        ) -> std::result::Result<Response, String> {
            assert!(body.is_empty());
            self.calls.lock().unwrap().push((method.into(), headers.to_vec()));
            Ok(self.response.clone())
        }
    }

    #[test]
    fn fetching_and_opening_never_acknowledge_even_on_invalid_media() {
        let (m, ciphertext) = seal(b"photo", 1, 1).unwrap();
        let wire = Wire { response: Response { status: status::OK, body: ciphertext }, calls: Mutex::new(vec![]) };
        assert_eq!(fetch_open(&wire, &m).unwrap(), Some(b"photo".to_vec()));
        let wrong_key = Media { key: [0; 32], ..m.clone() };
        assert!(fetch_open(&wire, &wrong_key).is_err());
        let wrong_size = Media { size: m.size + 1, ..m.clone() };
        assert!(fetch_open(&wire, &wrong_size).is_err());
        let wrong_hash = Media { sha256: [0; 32], ..m };
        assert!(fetch_open(&wire, &wrong_hash).is_err());
        let calls = wire.calls.lock().unwrap();
        assert_eq!(calls.len(), 4);
        for (method, headers) in calls.iter() {
            assert_eq!(method, "GET");
            assert!(headers.is_empty());
        }
    }

    #[test]
    fn explicit_ack_is_anonymous_and_failures_are_returned_for_retry() {
        for (status, expected) in
            [(204, Ok(())), (404, Ok(())), (429, Err(Error::Refused(429))), (503, Err(Error::Refused(503)))]
        {
            let wire = Wire { response: Response { status, body: vec![] }, calls: Mutex::new(vec![]) };
            assert_eq!(ack(&wire, &[1; 16]), expected);
            assert_eq!(*wire.calls.lock().unwrap(), vec![("DELETE".into(), vec![])]);
        }
    }
}
