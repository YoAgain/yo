//! Photos and videos (spec 5, 6.8): each sealed under a key of its own, uploaded as a blob for the chat's other
//! devices to fetch, the key, hash and size carried in the Yo. The hub never sees the key, the picture, or who fetches.
//!
//! Nothing here holds the client while it talks to the hub: `Client::upload` only signs; the upload itself
//! (`Upload::send`) and fetches (`fetch`) need no client at all.

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

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).map_err(|_| Error::Internal("random"))?;
    Ok(bytes)
}

/// Seals a photo or video under a new key, with a new id: what the Yo carries, and the ciphertext to upload.
pub fn seal(plain: &[u8], width: u16, height: u16) -> Result<(Media, Vec<u8>)> {
    if plain.is_empty() || plain.len() > MAX_MEDIA_LEN {
        return Err(Error::Protocol("media size"));
    }
    let key: [u8; 32] = random()?;
    let ciphertext =
        ChaCha20Poly1305::new(&key.into()).encrypt(&NONCE.into(), plain).map_err(|_| Error::Internal("seal"))?;
    let media = Media { id: random()?, key, sha256: sha256(&ciphertext), size: ciphertext.len() as u32, width, height };
    Ok((media, ciphertext))
}

/// Opens a fetched blob: its size and hash must be the ones the Yo named, then it must decrypt.
pub fn open(media: &Media, ciphertext: &[u8]) -> Result<Vec<u8>> {
    if ciphertext.len() != media.size as usize || sha256(ciphertext) != media.sha256 {
        return Err(Error::Protocol("media hash"));
    }
    ChaCha20Poly1305::new(&media.key.into()).decrypt(&NONCE.into(), ciphertext).map_err(|_| Error::Protocol("media"))
}

/// Fetches a blob by its id, without the client and without identity (spec 6.8), and checks it against the Yo.
/// `None`: the hub no longer has it (fetched by all, or past its time). Each fetch counts at the hub: keep what came.
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
    /// Sends the blob. `Ok` when the hub has it; `Refused(429)` (the account's quota or a rate) and
    /// `Refused(507)` (no room at the hub) are "later": sign anew with `Client::upload` and send again.
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

/// Whether an upload's refusal means "later": the quota, the hub's room, no network.
pub fn retry_later(error: &Error) -> bool {
    match error {
        Error::Network(_) => true,
        Error::Refused(status) => {
            *status == status::RATE_LIMITED || *status == status::INSUFFICIENT_STORAGE || *status >= 500
        }
        _ => false,
    }
}

impl Client {
    /// Signs the upload of a sealed blob for `fetches` fetches: the number of the chat's devices other than this one
    /// (`media_fetches`). Each try needs its own signature (the hub takes each once).
    pub fn upload(&mut self, media: &Media, fetches: u32) -> Upload {
        let path = api::blob_put_path(&media.id, fetches.clamp(1, MAX_BLOB_FETCHES));
        let (mut headers, _) = self.sign_hash("PUT", &path, &media.sha256);
        headers.push((HEADER_BODY_HASH.to_owned(), yo_wire::b64(&media.sha256)));
        Upload { transport: self.transport.clone(), path, headers }
    }

    /// From now on, the keys of photos and videos received are dropped as they are decrypted, before any event is
    /// kept: for a client that never opens them (YoGPT). The blob's id stays, to fetch it into nothing.
    pub fn forget_media_keys(&mut self) {
        self.media_keys_forgotten = true;
    }

    /// The transport, for fetches that need no client (`fetch`).
    pub fn transport(&self) -> Arc<dyn Transport> {
        self.transport.clone()
    }

    /// Opens the chat with `to` if there is none yet, as the first send would (a lookup, the group, the Welcome), and
    /// answers how many fetches a photo or video to them is for (`media_fetches`). For the first photo to a new
    /// friend: the upload needs the chat's devices before the Yo can go.
    pub fn open_chat(&mut self, to: &str) -> Result<(u32, Vec<crate::Event>)> {
        let result = self.open_chat_inner(to);
        let fetches = result.as_ref().ok().copied();
        let events = self.finish(result)?;
        Ok((fetches.unwrap_or(1), events))
    }

    fn open_chat_inner(&mut self, to: &str) -> Result<u32> {
        if let Some(fetches) = self.media_fetches(to) {
            return Ok(fetches);
        }
        let handle = yo_wire::handle::Handle::parse(to)?;
        if self.is_me(&handle) {
            return Err(Error::Protocol("sending to oneself"));
        }
        // As a send: what waits is received first; offline, nothing can be opened.
        self.deliver().and_then(|()| self.receive_all(0))?;
        let mut user = self.known_user(&handle);
        if user.is_none() && handle.kind != yo_wire::handle::KIND_USER && !self.state.contacts.is_empty() {
            let (found, username) = self.lookup_named(&handle, false, Vec::new())?;
            let id = crate::state::hex(&found.user_id);
            if id == self.state.user_id {
                return Err(Error::Protocol("sending to oneself"));
            }
            if let Some(contact) = self.state.contacts.get_mut(&id) {
                crate::note_handle(contact, username.unwrap_or_else(|| handle.display()));
            }
            user = Some(id);
        }
        if let Some(contact) = user.as_ref().and_then(|u| self.state.contacts.get(u)) {
            if contact.blocked {
                return Err(Error::Blocked);
            }
            if contact.unconfirmed {
                return Err(Error::Unconfirmed);
            }
        }
        let user = match user.filter(|u| self.chat_with(u).is_some()) {
            Some(user) => user,
            None => self.start_chat(&handle)?.0,
        };
        let group = self.chat_with(&user).ok_or(Error::Internal("chat"))?;
        // The Welcome and the profile on their way; offline, they wait in the outbox.
        match self.deliver() {
            Ok(()) | Err(Error::Network(_)) => {}
            Err(error) => return Err(error),
        }
        let members = Self::members(&self.load(&group)?).len();
        Ok((members.saturating_sub(1) as u32).clamp(1, MAX_BLOB_FETCHES))
    }

    /// How many fetches a photo or video to `to` is for: the devices of the chat with them other than this one (their
    /// devices and the own other ones). `None` while there is no chat yet.
    pub fn media_fetches(&self, to: &str) -> Option<u32> {
        let handle = yo_wire::handle::Handle::parse(to).ok()?;
        let group = self.chat_with(&self.known_user(&handle)?)?;
        let members = Self::members(&self.load(&group).ok()?).len();
        Some((members.saturating_sub(1) as u32).clamp(1, MAX_BLOB_FETCHES))
    }
}
