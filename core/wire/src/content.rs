//! The plaintext of an application message (spec 5): a typed content, never free-form bytes.

use crate::Error;
use serde::{Deserialize, Serialize};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};

/// What a Yo carries (spec 5, `With`): kinds, sorted, each once. 1-3 (text, link, location) are reserved.
pub const WITH_PHOTO: u16 = 4;
pub const WITH_VIDEO: u16 = 5;
/// The group a Yo goes to (spec 4.9): its master key and the revision the sender has.
pub const WITH_GROUP: u16 = 6;
/// The sender's profile key and its generation (spec 4.10: `opaque key[32]; uint32 generation`), on every content
/// message to an accepted contact or a joined group's members, as Signal's `DataMessage.profileKey`: a key missed or
/// forgotten comes back with the next one. Apps before it skip the kind.
pub const WITH_PROFILE_KEY: u16 = 7;

/// A profile key and its generation (the account's count of its keys, 1 the first): which of two is the later.
pub type KeyedBy = ([u8; 32], u32);
/// The id of a blob at the hub (spec 6.8): 16 random bytes the uploader chooses.
pub const BLOB_ID_LEN: usize = 16;
pub type BlobId = [u8; BLOB_ID_LEN];

pub const TYPE_YO: u16 = 1;
pub const TYPE_PROFILE: u16 = 2;
/// Silent: a delivery receipt, the answer that moves the sender's ratchet on (spec 4.5).
pub const TYPE_RECEIPT: u16 = 6;
/// Silent: a group was made or changed at the hub (spec 4.9): its master key and the revision now.
pub const TYPE_GROUP: u16 = 8;
/// The sender's profile key (spec 4.10): with it, the receiver's deliveries to the sender are a contact's, not a
/// request. Never shown.
pub const TYPE_PROFILE_KEY: u16 = 10;
/// Silent roundtrip request / response, carrying a random 32-byte token.
pub const TYPE_PROBE: u16 = 11;
pub const TYPE_PROBE_REPLY: u16 = 12;
pub const TYPE_SYNC: u16 = 13;
/// Silent: the sender allowed a service to be connected with another one; a random 32-byte code of that connection.
pub const TYPE_CONNECT: u16 = 14;

/// A read watermark (Signal's `SyncMessage.Read`): one direct or group thread, read through the message sent then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Read {
    pub user: Option<crate::UserId>,
    pub group: Option<[u8; 32]>,
    pub through_sent_at: u64,
}

/// The most threads one `SyncMessage::Read` names.
pub const MAX_READ: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Type 13 (spec 5): what one device of an account tells its other devices, Signal's `SyncMessage`. Own devices
/// only; unknown kinds are skipped, later own-device syncs come as new kinds.
pub enum SyncMessage {
    /// Kind 0: threads shown on the sending device, each naming exactly one thread.
    Read(Vec<Read>),
    /// Kind 1: the storage (spec 6.9) changed: read it.
    StorageChanged,
    /// A kind this version does not know, its body kept as it came.
    Unknown(u8, Vec<u8>),
}

impl SyncMessage {
    fn body(&self) -> Result<Vec<u8>, Error> {
        match self {
            Self::Read(reads)
                if !reads.is_empty()
                    && reads.len() <= MAX_READ
                    && reads.iter().all(|r| r.user.is_some() != r.group.is_some()) =>
            {
                let mut bytes = vec![0];
                bytes.extend(crate::encode(reads));
                Ok(bytes)
            }
            Self::StorageChanged => Ok(vec![1]),
            Self::Unknown(kind, body) if *kind > 1 => Ok([&[*kind][..], body].concat()),
            _ => Err(Error::Format),
        }
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        match bytes.split_first() {
            Some((0, body)) => {
                let reads: Vec<Read> = crate::decode(body)?;
                let message = Self::Read(reads);
                // Only what `body` makes: one thread each, at most `MAX_READ`.
                message.body()?;
                Ok(message)
            }
            Some((1, [])) => Ok(Self::StorageChanged),
            Some((1, _)) | None => Err(Error::Format),
            Some((&kind, body)) => Ok(Self::Unknown(kind, body.to_vec())),
        }
    }
}

/// A group's master key (Signal's `GroupMasterKey`, zkgroup): 32 random bytes its creator chooses; its members hold it,
/// the hub never does. The group's id at the hub (its `GroupIdentifier`) is derived from it.
pub const GROUP_KEY_LEN: usize = 32;
pub type GroupKey = [u8; GROUP_KEY_LEN];
/// The most accounts in a group, its creator among them.
pub const MAX_GROUP: usize = 20;

pub const MAX_NAME_LEN: usize = 32;
const CONTENT_VERSION: u16 = 1;

/// A photo or video a Yo carries: a blob at the hub (spec 6.8), encrypted under `key` (ChaCha20-Poly1305, nonce
/// zero: the key is used once); `sha256` and `size` are the ciphertext's. Only the receiver's devices learn the key.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// What a group Yo or a group's change names (spec 4.9): the group's master key, and its revision as the sender has
/// it. A receiver at an older revision fetches the group from the hub.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct GroupTag {
    pub key: GroupKey,
    pub revision: u32,
}

/// No key in a log.
impl std::fmt::Debug for GroupTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroupTag").field("revision", &self.revision).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Something from the receiver arrived (spec 4.5). Never shown.
    Receipt,
    /// A group made or changed at the hub: fetch it there. Never shown.
    Group(GroupTag),
    /// The sender's profile key: what the receiver shows the hub so that its deliveries to the sender are a contact's
    /// (spec 4.10). Never shown.
    ProfileKey([u8; 32]),
    Probe([u8; 32]),
    ProbeReply([u8; 32]),
    Sync(SyncMessage),
    /// To a service: the sender connects it, with this code. Never shown.
    Connect([u8; 32]),
    /// A type this version does not know: newer apps may send it.
    Unknown(u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub sent_at_ms: u64,
    /// A Yo (with a photo or video or not) to a group; never on other kinds.
    pub group: Option<GroupTag>,
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

/// A Yo's body: what it carries, by kind (ascending), empty for a plain Yo.
fn yo_body(items: Vec<(u16, Vec<u8>)>) -> Vec<u8> {
    if items.is_empty() {
        return Vec::new();
    }
    crate::encode(&WithList { with: items.into_iter().map(|(kind, data)| With { kind, data: data.into() }).collect() })
}

#[derive(TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct ProfileKeyBody {
    key: [u8; 32],
}

/// A Yo's body: empty, or what it carries, sorted by kind, each kind once, at least one. Kinds this version does not
/// know are skipped (the Yo still shows); of a photo and a video, the photo; and the group it goes to, if any.
fn read_yo(body: &[u8]) -> Result<(Content, Option<GroupTag>), Error> {
    if body.is_empty() {
        return Ok((Content::Yo, None));
    }
    let WithList { with } = crate::decode(body)?;
    if with.is_empty() || with.windows(2).any(|pair| pair[0].kind >= pair[1].kind) {
        return Err(Error::Format);
    }
    let (mut content, mut group) = (None, None);
    for item in with {
        let data = item.data.as_slice();
        match item.kind {
            WITH_PHOTO => content = content.or(Some(Content::Photo(crate::decode::<Photo>(data)?.into()))),
            // A video after a photo is not read (spec 5): the photo wins.
            WITH_VIDEO if content.is_none() => {
                let Video { photo, duration_ms } = crate::decode(data)?;
                content = Some(Content::Video { media: photo.into(), duration_ms });
            }
            WITH_GROUP => group = Some(crate::decode::<GroupTag>(data)?),
            _ => {}
        }
    }
    Ok((content.unwrap_or(Content::Yo), group))
}

#[derive(TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Text {
    text: VLBytes,
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
        self.encode_with_key(None)
    }

    /// As `encode`, a content message carrying the sender's profile key too (`WITH_PROFILE_KEY`): a Yo in its `With`
    /// list (spec 5). Receipts, names, and group and key messages carry none.
    pub fn encode_with_key(&self, profile_key: Option<KeyedBy>) -> Result<Vec<u8>, Error> {
        let key =
            profile_key.map(|(key, generation)| (WITH_PROFILE_KEY, [&key[..], &generation.to_be_bytes()].concat()));
        let group = self.group.map(|tag| (WITH_GROUP, crate::encode(&tag)));
        if group.is_some() && !matches!(self.content, Content::Yo | Content::Photo(_) | Content::Video { .. }) {
            return Err(Error::Format);
        }
        let (kind, body) = match &self.content {
            Content::Yo => (TYPE_YO, yo_body([group, key].into_iter().flatten().collect())),
            Content::Photo(media) => {
                let photo = (WITH_PHOTO, crate::encode(&Photo::from(media)));
                (TYPE_YO, yo_body([Some(photo), group, key].into_iter().flatten().collect()))
            }
            Content::Video { media, duration_ms } => {
                let video = (WITH_VIDEO, crate::encode(&Video { photo: media.into(), duration_ms: *duration_ms }));
                (TYPE_YO, yo_body([Some(video), group, key].into_iter().flatten().collect()))
            }
            Content::Profile { name } => (TYPE_PROFILE, text(name, MAX_NAME_LEN)?),
            Content::Probe(token) => (TYPE_PROBE, token.to_vec()),
            Content::ProbeReply(token) => (TYPE_PROBE_REPLY, token.to_vec()),
            Content::Connect(code) => (TYPE_CONNECT, code.to_vec()),
            Content::Sync(seen) => (TYPE_SYNC, seen.body()?),
            Content::Receipt => (TYPE_RECEIPT, Vec::new()),
            Content::Group(tag) => (TYPE_GROUP, crate::encode(tag)),
            Content::ProfileKey(key) => (TYPE_PROFILE_KEY, crate::encode(&ProfileKeyBody { key: *key })),
            Content::Unknown(_) => return Err(Error::Unsupported),
        };
        Ok(crate::encode(&Wire { version: CONTENT_VERSION, sent_at: self.sent_at_ms, kind, body: body.into() }))
    }

    /// The sender's profile key a message carries (`encode_with_key`), with its generation, if it does.
    pub fn profile_key_of(bytes: &[u8]) -> Option<KeyedBy> {
        let wire: Wire = crate::decode(bytes).ok()?;
        let body = wire.body.as_slice();
        if wire.version != CONTENT_VERSION {
            return None;
        }
        let list = match wire.kind {
            TYPE_YO if !body.is_empty() => body,
            _ => return None,
        };
        let WithList { with } = crate::decode(list).ok()?;
        let data = with.into_iter().find(|w| w.kind == WITH_PROFILE_KEY)?.data;
        let data = data.as_slice();
        (data.len() == 36)
            .then(|| (data[..32].try_into().expect("32"), u32::from_be_bytes(data[32..].try_into().expect("4"))))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let wire: Wire = crate::decode(bytes)?;
        if wire.version != CONTENT_VERSION {
            return Err(Error::Unsupported);
        }
        let body = wire.body.as_slice();
        let mut group = None;
        let content = match wire.kind {
            TYPE_YO => {
                let (content, tag) = read_yo(body)?;
                group = tag;
                content
            }
            TYPE_PROFILE => Content::Profile { name: read_text(body, MAX_NAME_LEN)? },
            TYPE_SYNC => Content::Sync(SyncMessage::decode(body)?),
            TYPE_RECEIPT if !body.is_empty() => return Err(Error::Format),
            TYPE_RECEIPT => Content::Receipt,
            TYPE_PROBE => Content::Probe(body.try_into().map_err(|_| Error::Format)?),
            TYPE_PROBE_REPLY => Content::ProbeReply(body.try_into().map_err(|_| Error::Format)?),
            TYPE_CONNECT => Content::Connect(body.try_into().map_err(|_| Error::Format)?),
            TYPE_GROUP => Content::Group(crate::decode::<GroupTag>(body)?),
            TYPE_PROFILE_KEY => Content::ProfileKey(crate::decode::<ProfileKeyBody>(body)?.key),
            other => Content::Unknown(other),
        };
        Ok(Message { sent_at_ms: wire.sent_at, group, content })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_video_after_a_photo_is_not_read() {
        let photo = Photo { id: [1; 16], key: [2; 32], sha256: [3; 32], size: 9, width: 1, height: 1 };
        let with = WithList {
            with: vec![
                With { kind: WITH_PHOTO, data: crate::encode(&photo).into() },
                With { kind: WITH_VIDEO, data: vec![0xff; 3].into() },
            ],
        };
        let (content, _) = read_yo(&crate::encode(&with)).unwrap();
        assert!(matches!(content, Content::Photo(_)));
    }
    use proptest::prelude::*;

    fn media(n: u8) -> Media {
        Media { id: [n; 16], key: [n + 1; 32], sha256: [n + 2; 32], size: 300_000, width: 1200, height: 1600 }
    }

    #[test]
    fn a_yo_carries_a_photo_or_a_video() {
        for (content, len) in
            [(Content::Photo(media(1)), 108), (Content::Video { media: media(2), duration_ms: 9_500 }, 112)]
        {
            let message = Message { sent_at_ms: 7, group: None, content };
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
        // The sender's profile key on a Yo: read beside it, skipped by `decode` (and so by apps before it).
        let group = GroupTag { key: [3; 32], revision: 2 };
        let keyed = Message { sent_at_ms: 5, group: Some(group), content: Content::Photo(media(3)) };
        let bytes = keyed.encode_with_key(Some(([7; 32], 3))).unwrap();
        assert_eq!(Message::decode(&bytes).unwrap(), keyed);
        assert_eq!(Message::profile_key_of(&bytes), Some(([7; 32], 3)));
        assert_eq!(Message::profile_key_of(&keyed.encode().unwrap()), None);
        let plain = Message { sent_at_ms: 5, group: None, content: Content::Yo };
        let bytes = plain.encode_with_key(Some(([8; 32], 1))).unwrap();
        assert_eq!(Message::decode(&bytes).unwrap(), plain, "a plain Yo with only the key is a plain Yo");
        assert_eq!(Message::profile_key_of(&bytes), Some(([8; 32], 1)));
        assert_eq!(bytes.len(), 53, "spec 5: 13 bytes, 53 with the key");
        let receipt = Message { sent_at_ms: 5, group: None, content: Content::Receipt };
        assert_eq!(
            receipt.encode_with_key(Some(([8; 32], 1))).unwrap(),
            receipt.encode().unwrap(),
            "no key on a receipt"
        );
        // An empty list, a kind twice, out of order, a short photo: refused.
        assert!(Message::decode(&yo(vec![])).is_err());
        assert!(Message::decode(&yo(vec![photo(), photo()])).is_err());
        assert!(Message::decode(&yo(vec![other(99), photo()])).is_err());
        assert!(Message::decode(&yo(vec![With { kind: WITH_PHOTO, data: vec![0; 10].into() }])).is_err());
    }

    #[test]
    fn a_group_yo_and_a_group_round_trip() {
        let tag = GroupTag { key: [9; 32], revision: 3 };
        for content in [Content::Yo, Content::Photo(media(1)), Content::Video { media: media(2), duration_ms: 1 }] {
            let message = Message { sent_at_ms: 9, group: Some(tag), content };
            assert_eq!(Message::decode(&message.encode().unwrap()).unwrap(), message);
        }
        let message = Message { sent_at_ms: 9, group: None, content: Content::Group(tag) };
        assert_eq!(Message::decode(&message.encode().unwrap()).unwrap(), message);
        // The key never shows in a log; a group on a profile: refused.
        assert!(!format!("{tag:?}").contains("[9"));
        let profile = Content::Profile { name: "A".into() };
        assert_eq!(Message { sent_at_ms: 9, group: Some(tag), content: profile }.encode(), Err(Error::Format));
    }

    #[test]
    fn a_yo_is_tiny() {
        let bytes = Message { sent_at_ms: 1, group: None, content: Content::Yo }.encode().unwrap();
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
            let message = Message { sent_at_ms: 0, group: None, content: Content::Profile { name: name.into() } };
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

#[cfg(test)]
mod seen_tests {
    use super::*;

    #[test]
    fn seen_bodies_are_exact_and_unknown_kinds_are_skipped() {
        for seen in [
            SyncMessage::Read(vec![
                Read { user: Some([3; 16]), group: None, through_sent_at: 10 },
                Read { user: None, group: Some([4; 32]), through_sent_at: 11 },
            ]),
            SyncMessage::StorageChanged,
            SyncMessage::Unknown(42, vec![1, 2]),
        ] {
            let message = Message { sent_at_ms: 1, group: None, content: Content::Sync(seen) };
            assert_eq!(Message::decode(&message.encode().unwrap()).unwrap(), message);
        }
        assert_eq!(SyncMessage::decode(&[1, 0]), Err(Error::Format));
        assert_eq!(SyncMessage::decode(&[42, 1, 2]), Ok(SyncMessage::Unknown(42, vec![1, 2])));
        let neither = crate::encode(&vec![Read { user: None, group: None, through_sent_at: 1 }]);
        assert_eq!(SyncMessage::decode(&[&[0][..], &neither].concat()), Err(Error::Format));
        assert_eq!(SyncMessage::decode(&[&[0][..], &crate::encode(&Vec::<Read>::new())].concat()), Err(Error::Format));
        // The fuzzer's find: a kind this version does not know encodes back as it came.
        let mut found = vec![0, 1, 0x10, 0, 0, 0x10, 0, 0, 0, 0, 0, 0x0d];
        found.extend([0x1d; 29]);
        found.push(0);
        let message = Message::decode(&found).unwrap();
        assert!(matches!(message.content, Content::Sync(SyncMessage::Unknown(0x1d, _))));
        assert_eq!(Message::decode(&message.encode().unwrap()).unwrap(), message);
        assert_eq!(SyncMessage::decode(&[0; 11]), Err(Error::Format));
    }
}
