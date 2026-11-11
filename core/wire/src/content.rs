//! The plaintext of an application message (spec 5): a typed content, never free-form bytes.

use crate::Error;
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};

pub const TYPE_YO: u16 = 1;
pub const TYPE_PROFILE: u16 = 2;
pub const TYPE_TEXT: u16 = 3;
pub const TYPE_LINK: u16 = 4;
pub const TYPE_LOCATION: u16 = 5;

pub const MAX_NAME_LEN: usize = 32;
pub const MAX_TEXT_LEN: usize = 4096;
pub const MAX_URL_LEN: usize = 2048;
const CONTENT_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Yo,
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
            TYPE_YO if body.is_empty() => Content::Yo,
            TYPE_YO => return Err(Error::Format),
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
