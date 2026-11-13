//! The plaintext of an application message (spec 5): a typed content, never free-form bytes.

use crate::Error;
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};

/// What a Yo carries (spec 5, `With`): kinds, sorted, each once.
pub const WITH_TEXT: u16 = 1;
pub const WITH_LINK: u16 = 2;
pub const WITH_LOCATION: u16 = 3;
pub const WITH_PHOTO: u16 = 4;
pub const WITH_VIDEO: u16 = 5;
/// The id of a blob at the hub (spec 6.8): 16 random bytes the uploader chooses.
pub const BLOB_ID_LEN: usize = 16;
pub type BlobId = [u8; BLOB_ID_LEN];

pub const TYPE_YO: u16 = 1;
pub const TYPE_PROFILE: u16 = 2;
pub const TYPE_TEXT: u16 = 3;
pub const TYPE_LINK: u16 = 4;
pub const TYPE_LOCATION: u16 = 5;

pub const MAX_NAME_LEN: usize = 32;
pub const MAX_TEXT_LEN: usize = 4096;
pub const MAX_URL_LEN: usize = 2048;
const CONTENT_VERSION: u16 = 1;

/// A photo or video a Yo carries: a blob at the hub (spec 6.8), encrypted under `key` (ChaCha20-Poly1305, nonce
/// zero: the key is used once); `sha256` and `size` are the ciphertext's. Only the members of the chat learn the key.
#[derive(Clone, PartialEq, Eq)]
pub struct Media {
    pub id: BlobId,
    pub key: [u8; 32],
    pub sha256: [u8; 32],
    pub size: u32,
    pub width: u16,
    pub height: u16,
}

/// No key in a log.
impl std::fmt::Debug for Media {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Media")
            .field("size", &self.size)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// A Yo with nothing.
    Yo,
    /// A Yo with a photo (JPEG or HEIC).
    Photo(Media),
    /// A Yo with a short video (MP4 or QuickTime), `duration_ms` long.
    Video {
        media: Media,
        duration_ms: u32,
    },
    Profile {
        name: String,
    },
    Text(String),
    Link(String),
    Location {
        lat_e7: i32,
        lon_e7: i32,
    },
    /// A type this version does not know: newer apps may send it.
    Unknown(u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub sent_at_ms: u64,
    pub content: Content,
}

#[derive(TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Wire {
    version: u16,
    sent_at: u64,
    kind: u16,
    body: VLBytes,
}

/// One thing a Yo carries.
#[derive(Debug, TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct With {
    kind: u16,
    data: VLBytes,
}

#[derive(TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct WithList {
    with: Vec<With>,
}

#[derive(TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Photo {
    id: BlobId,
    key: [u8; 32],
    sha256: [u8; 32],
    size: u32,
    width: u16,
    height: u16,
}

#[derive(TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Video {
    photo: Photo,
    duration_ms: u32,
}

impl From<&Media> for Photo {
    fn from(m: &Media) -> Self {
        Photo { id: m.id, key: m.key, sha256: m.sha256, size: m.size, width: m.width, height: m.height }
    }
}

impl From<Photo> for Media {
    fn from(p: Photo) -> Self {
        Media { id: p.id, key: p.key, sha256: p.sha256, size: p.size, width: p.width, height: p.height }
    }
}

/// A Yo's body with one thing.
fn with(kind: u16, data: Vec<u8>) -> Vec<u8> {
    crate::encode(&WithList { with: vec![With { kind, data: data.into() }] })
}

/// A Yo's body: empty, or what it carries, sorted by kind, each kind once, at least one. Kinds this version does not
/// know are skipped (the Yo still shows); of a photo and a video, the photo.
fn read_yo(body: &[u8]) -> Result<Content, Error> {
    if body.is_empty() {
        return Ok(Content::Yo);
    }
    let WithList { with } = crate::decode(body)?;
    if with.is_empty() || with.windows(2).any(|pair| pair[0].kind >= pair[1].kind) {
        return Err(Error::Format);
    }
    for item in with {
        let data = item.data.as_slice();
        match item.kind {
            WITH_PHOTO => return Ok(Content::Photo(crate::decode::<Photo>(data)?.into())),
            WITH_VIDEO => {
                let Video { photo, duration_ms } = crate::decode(data)?;
                return Ok(Content::Video { media: photo.into(), duration_ms });
            }
            _ => {}
        }
    }
    Ok(Content::Yo)
}

#[derive(TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Text {
    text: VLBytes,
}

#[derive(TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Location {
    lat_e7: u32,
    lon_e7: u32,
}

fn text(value: &str, max: usize) -> Result<Vec<u8>, Error> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(Error::Format);
    }
    Ok(crate::encode(&Text { text: value.as_bytes().to_vec().into() }))
}

fn read_text(body: &[u8], max: usize) -> Result<String, Error> {
    let Text { text } = crate::decode(body)?;
    let value = String::from_utf8(text.as_slice().to_vec()).map_err(|_| Error::Format)?;
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(Error::Format);
    }
    Ok(value)
}

impl Message {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let (kind, body) = match &self.content {
            Content::Yo => (TYPE_YO, Vec::new()),
            Content::Photo(media) => (TYPE_YO, with(WITH_PHOTO, crate::encode(&Photo::from(media)))),
            Content::Video { media, duration_ms } => {
                (TYPE_YO, with(WITH_VIDEO, crate::encode(&Video { photo: media.into(), duration_ms: *duration_ms })))
            }
            Content::Profile { name } => (TYPE_PROFILE, text(name, MAX_NAME_LEN)?),
            Content::Text(value) => (TYPE_TEXT, text(value, MAX_TEXT_LEN)?),
            Content::Link(url) => (TYPE_LINK, text(url, MAX_URL_LEN)?),
            Content::Location { lat_e7, lon_e7 } => {
                (TYPE_LOCATION, crate::encode(&Location { lat_e7: *lat_e7 as u32, lon_e7: *lon_e7 as u32 }))
            }
            Content::Unknown(_) => return Err(Error::Unsupported),
        };
        Ok(crate::encode(&Wire { version: CONTENT_VERSION, sent_at: self.sent_at_ms, kind, body: body.into() }))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let wire: Wire = crate::decode(bytes)?;
        if wire.version != CONTENT_VERSION {
            return Err(Error::Unsupported);
        }
        let body = wire.body.as_slice();
        let content = match wire.kind {
            TYPE_YO => read_yo(body)?,
            TYPE_PROFILE => Content::Profile { name: read_text(body, MAX_NAME_LEN)? },
            TYPE_TEXT => Content::Text(read_text(body, MAX_TEXT_LEN)?),
            TYPE_LINK => Content::Link(read_text(body, MAX_URL_LEN)?),
            TYPE_LOCATION => {
                let Location { lat_e7, lon_e7 } = crate::decode(body)?;
                Content::Location { lat_e7: lat_e7 as i32, lon_e7: lon_e7 as i32 }
            }
            other => Content::Unknown(other),
        };
        Ok(Message { sent_at_ms: wire.sent_at, content })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn round_trips() {
        for content in [
            Content::Yo,
            Content::Profile { name: "CHRIS".into() },
            Content::Text("hi there".into()),
            Content::Link("https://justyo.co".into()),
            Content::Location { lat_e7: -338_688_000, lon_e7: 1_512_093_000 },
        ] {
            let message = Message { sent_at_ms: 1_762_000_000_000, content };
            assert_eq!(Message::decode(&message.encode().unwrap()).unwrap(), message);
        }
    }

    fn media(n: u8) -> Media {
        Media { id: [n; 16], key: [n + 1; 32], sha256: [n + 2; 32], size: 300_000, width: 1200, height: 1600 }
    }

    #[test]
    fn a_yo_carries_a_photo_or_a_video() {
        for (content, len) in
            [(Content::Photo(media(1)), 108), (Content::Video { media: media(2), duration_ms: 9_500 }, 112)]
        {
            let message = Message { sent_at_ms: 7, content };
            let bytes = message.encode().unwrap();
            assert_eq!(Message::decode(&bytes).unwrap(), message);
            assert_eq!(bytes.len(), len, "spec 5; in the smallest envelope");
        }
        // The key never shows in a log.
        assert!(!format!("{:?}", Content::Photo(media(1))).contains("key"));
    }

    #[test]
    fn what_a_yo_carries_is_strict_and_unknown_kinds_are_skipped() {
        let yo = |with: Vec<With>| {
            let body = crate::encode(&WithList { with });
            crate::encode(&Wire { version: 1, sent_at: 5, kind: TYPE_YO, body: body.into() })
        };
        let photo = || With { kind: WITH_PHOTO, data: crate::encode(&Photo::from(&media(3))).into() };
        let other = |kind| With { kind, data: vec![1, 2, 3].into() };
        // Unknown kinds around a photo: the photo.
        let decoded = Message::decode(&yo(vec![other(1), photo(), other(99)])).unwrap();
        assert_eq!(decoded.content, Content::Photo(media(3)));
        // Nothing known: a plain Yo.
        assert_eq!(Message::decode(&yo(vec![other(99)])).unwrap().content, Content::Yo);
        // An empty list, a kind twice, out of order, a short photo: refused.
        assert!(Message::decode(&yo(vec![])).is_err());
        assert!(Message::decode(&yo(vec![photo(), photo()])).is_err());
        assert!(Message::decode(&yo(vec![other(99), photo()])).is_err());
        assert!(Message::decode(&yo(vec![With { kind: WITH_PHOTO, data: vec![0; 10].into() }])).is_err());
    }

    #[test]
    fn a_yo_is_tiny() {
        let bytes = Message { sent_at_ms: 1, content: Content::Yo }.encode().unwrap();
        assert_eq!(bytes.len(), 2 + 8 + 2 + 1);
    }

    #[test]
    fn unknown_types_are_reported_not_refused() {
        let bytes = crate::encode(&Wire { version: 1, sent_at: 5, kind: 99, body: vec![1, 2].into() });
        assert_eq!(Message::decode(&bytes).unwrap().content, Content::Unknown(99));
    }

    #[test]
    fn refuses_bad_names() {
        for name in ["", "THIS NAME IS FAR TOO LONG FOR A YO NAME", "a\nb"] {
            let message = Message { sent_at_ms: 0, content: Content::Profile { name: name.into() } };
            assert_eq!(message.encode(), Err(Error::Format));
        }
    }

    proptest! {
        #[test]
        fn never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
            let _ = Message::decode(&bytes);
        }
    }
}
