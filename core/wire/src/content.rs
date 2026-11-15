//! The plaintext of an application message (spec 5): a typed content, never free-form bytes.

use crate::Error;
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};

/// What a Yo carries (spec 5, `With`): kinds, sorted, each once.
pub const WITH_TEXT: u16 = 1;
pub const WITH_LINK: u16 = 2;
pub const WITH_LOCATION: u16 = 3;
pub const WITH_PHOTO: u16 = 4;
pub const WITH_VIDEO: u16 = 5;
/// The group a Yo goes to (spec 5): its id and the revision the sender has.
pub const WITH_GROUP: u16 = 6;
/// The id of a blob at the hub (spec 6.8): 16 random bytes the uploader chooses.
pub const BLOB_ID_LEN: usize = 16;
pub type BlobId = [u8; BLOB_ID_LEN];

pub const TYPE_YO: u16 = 1;
pub const TYPE_PROFILE: u16 = 2;
pub const TYPE_TEXT: u16 = 3;
pub const TYPE_LINK: u16 = 4;
pub const TYPE_LOCATION: u16 = 5;
/// Silent: a delivery receipt, the answer that moves the sender's ratchet on (spec 4.5).
pub const TYPE_RECEIPT: u16 = 6;
/// Silent: a message from the receiver did not open here; it sends again what it keeps (spec 4.6).
pub const TYPE_RETRY: u16 = 7;
/// A group as its sender has it now: id, revision, name, members (spec 4.9).
pub const TYPE_GROUP: u16 = 8;
/// Silent: the sender knows a newer revision of a group than this: it is asked for (spec 4.9).
pub const TYPE_GROUP_ASK: u16 = 9;
/// The sender's contact key (spec 4.10): with it, the receiver's deliveries to the sender are a contact's, not a
/// request. Never shown.
pub const TYPE_CONTACT_KEY: u16 = 10;

/// A group's id: 16 random bytes its creator chooses.
pub const GROUP_ID_LEN: usize = 16;
pub type GroupId = [u8; GROUP_ID_LEN];
/// The most accounts in a group, its creator among them.
pub const MAX_GROUP: usize = 20;

pub const MAX_NAME_LEN: usize = 32;
pub const MAX_TEXT_LEN: usize = 4096;
pub const MAX_URL_LEN: usize = 2048;
const CONTENT_VERSION: u16 = 1;

/// A photo or video a Yo carries: a blob at the hub (spec 6.8), encrypted under `key` (ChaCha20-Poly1305, nonce
/// zero: the key is used once); `sha256` and `size` are the ciphertext's. Only the receiver's devices learn the key.
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

/// What a group Yo names: the group, its revision as the sender has it, and `group_hash` of the sender's name and
/// members, so a receiver at the same revision sees when the two diverged (spec 4.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct GroupTag {
    pub id: GroupId,
    pub revision: u32,
    pub hash: [u8; GROUP_HASH_LEN],
}

pub const GROUP_HASH_LEN: usize = 8;

/// A group's name and members (sorted) as one value: the first 8 bytes of SHA-256("yo group v1" || id || name<V> ||
/// members<V>). The name too: what a member sees is both, and a rename lost is as much a divergence as a member.
pub fn group_hash(id: &GroupId, name: &str, members: &[crate::UserId]) -> [u8; GROUP_HASH_LEN] {
    use sha2::{Digest, Sha256};
    #[derive(TlsSerialize, TlsSize)]
    struct Hashed {
        id: GroupId,
        name: VLBytes,
        members: Vec<crate::UserId>,
    }
    let hashed = Hashed { id: *id, name: name.as_bytes().to_vec().into(), members: members.to_vec() };
    let digest = Sha256::new().chain_update(b"yo group v1").chain_update(crate::encode(&hashed)).finalize();
    digest[..GROUP_HASH_LEN].try_into().expect("8 bytes")
}

/// A group as one member has it: who is in it (user ids, sorted, each once, 1 to `MAX_GROUP`) and what it is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfo {
    pub id: GroupId,
    pub revision: u32,
    /// Who made this revision (not who passes it on).
    pub by: crate::UserId,
    pub name: String,
    pub members: Vec<crate::UserId>,
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
    /// Something from the receiver arrived (spec 4.5). Never shown.
    Receipt,
    /// Something from the receiver did not open: send again (spec 4.6). Never shown.
    Retry,
    /// A group made or changed: the whole group as the sender has it now.
    Group(GroupInfo),
    /// Send this group as you have it: the asker knows of a newer revision than its own. Never shown.
    GroupAsk {
        id: GroupId,
    },
    /// The sender's contact key: what the receiver shows the hub so that its deliveries to the sender are a contact's
    /// (spec 4.10). Never shown.
    ContactKey([u8; 32]),
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
struct Group {
    id: GroupId,
    revision: u32,
    /// Who made this revision: the tie-break of two at the same revision, also when another member passes it on.
    by: crate::UserId,
    name: VLBytes,
    members: Vec<crate::UserId>,
}

#[derive(TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct ContactKeyBody {
    key: [u8; 32],
}

#[derive(TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct GroupAsk {
    id: GroupId,
}

/// A group's members as they may be: 1 to `MAX_GROUP`, sorted, each once.
fn members_ok(members: &[crate::UserId]) -> bool {
    !members.is_empty() && members.len() <= MAX_GROUP && members.windows(2).all(|pair| pair[0] < pair[1])
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
        let group = self.group.map(|tag| (WITH_GROUP, crate::encode(&tag)));
        if group.is_some() && !matches!(self.content, Content::Yo | Content::Photo(_) | Content::Video { .. }) {
            return Err(Error::Format);
        }
        let (kind, body) = match &self.content {
            Content::Yo => (TYPE_YO, yo_body(group.into_iter().collect())),
            Content::Photo(media) => {
                let photo = (WITH_PHOTO, crate::encode(&Photo::from(media)));
                (TYPE_YO, yo_body([Some(photo), group].into_iter().flatten().collect()))
            }
            Content::Video { media, duration_ms } => {
                let video = (WITH_VIDEO, crate::encode(&Video { photo: media.into(), duration_ms: *duration_ms }));
                (TYPE_YO, yo_body([Some(video), group].into_iter().flatten().collect()))
            }
            Content::Profile { name } => (TYPE_PROFILE, text(name, MAX_NAME_LEN)?),
            Content::Text(value) => (TYPE_TEXT, text(value, MAX_TEXT_LEN)?),
            Content::Link(url) => (TYPE_LINK, text(url, MAX_URL_LEN)?),
            Content::Location { lat_e7, lon_e7 } => {
                (TYPE_LOCATION, crate::encode(&Location { lat_e7: *lat_e7 as u32, lon_e7: *lon_e7 as u32 }))
            }
            Content::Receipt => (TYPE_RECEIPT, Vec::new()),
            Content::Retry => (TYPE_RETRY, Vec::new()),
            Content::Group(info) => {
                if !members_ok(&info.members) {
                    return Err(Error::Format);
                }
                let name = text(&info.name, MAX_NAME_LEN)?;
                let Text { text: name } = crate::decode(&name)?;
                let group =
                    Group { id: info.id, revision: info.revision, by: info.by, name, members: info.members.clone() };
                (TYPE_GROUP, crate::encode(&group))
            }
            Content::GroupAsk { id } => (TYPE_GROUP_ASK, crate::encode(&GroupAsk { id: *id })),
            Content::ContactKey(key) => (TYPE_CONTACT_KEY, crate::encode(&ContactKeyBody { key: *key })),
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
        let mut group = None;
        let content = match wire.kind {
            TYPE_YO => {
                let (content, tag) = read_yo(body)?;
                group = tag;
                content
            }
            TYPE_PROFILE => Content::Profile { name: read_text(body, MAX_NAME_LEN)? },
            TYPE_TEXT => Content::Text(read_text(body, MAX_TEXT_LEN)?),
            TYPE_LINK => Content::Link(read_text(body, MAX_URL_LEN)?),
            TYPE_LOCATION => {
                let Location { lat_e7, lon_e7 } = crate::decode(body)?;
                Content::Location { lat_e7: lat_e7 as i32, lon_e7: lon_e7 as i32 }
            }
            TYPE_RECEIPT | TYPE_RETRY if !body.is_empty() => return Err(Error::Format),
            TYPE_RECEIPT => Content::Receipt,
            TYPE_RETRY => Content::Retry,
            TYPE_GROUP => {
                let Group { id, revision, by, name, members } = crate::decode(body)?;
                let name = read_text(&crate::encode(&Text { text: name }), MAX_NAME_LEN)?;
                if !members_ok(&members) {
                    return Err(Error::Format);
                }
                Content::Group(GroupInfo { id, revision, by, name, members })
            }
            TYPE_GROUP_ASK => Content::GroupAsk { id: crate::decode::<GroupAsk>(body)?.id },
            TYPE_CONTACT_KEY => Content::ContactKey(crate::decode::<ContactKeyBody>(body)?.key),
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

    #[test]
    fn round_trips() {
        for content in [
            Content::Yo,
            Content::Profile { name: "CHRIS".into() },
            Content::Text("hi there".into()),
            Content::Link("https://justyo.co".into()),
            Content::Location { lat_e7: -338_688_000, lon_e7: 1_512_093_000 },
            Content::Receipt,
            Content::Retry,
        ] {
            let message = Message { sent_at_ms: 1_762_000_000_000, group: None, content };
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
        // An empty list, a kind twice, out of order, a short photo: refused.
        assert!(Message::decode(&yo(vec![])).is_err());
        assert!(Message::decode(&yo(vec![photo(), photo()])).is_err());
        assert!(Message::decode(&yo(vec![other(99), photo()])).is_err());
        assert!(Message::decode(&yo(vec![With { kind: WITH_PHOTO, data: vec![0; 10].into() }])).is_err());
    }

    #[test]
    fn a_group_yo_and_a_group_round_trip() {
        let tag = GroupTag { id: [9; 16], revision: 3, hash: [4; 8] };
        for content in [Content::Yo, Content::Photo(media(1)), Content::Video { media: media(2), duration_ms: 1 }] {
            let message = Message { sent_at_ms: 9, group: Some(tag), content };
            assert_eq!(Message::decode(&message.encode().unwrap()).unwrap(), message);
        }
        let group =
            GroupInfo { id: [7; 16], revision: 1, by: [1; 16], name: "FAMILY".into(), members: vec![[1; 16], [2; 16]] };
        let message = Message { sent_at_ms: 9, group: None, content: Content::Group(group.clone()) };
        assert_eq!(Message::decode(&message.encode().unwrap()).unwrap(), message);
        let ask = Message { sent_at_ms: 9, group: None, content: Content::GroupAsk { id: [7; 16] } };
        assert_eq!(Message::decode(&ask.encode().unwrap()).unwrap(), ask);
        // Members unsorted, twice, none or too many: refused; a group on a profile: refused.
        for members in [vec![[2; 16], [1; 16]], vec![[1; 16], [1; 16]], vec![], (0..21u8).map(|i| [i; 16]).collect()] {
            let bad = Content::Group(GroupInfo { members, ..group.clone() });
            assert_eq!(Message { sent_at_ms: 9, group: None, content: bad }.encode(), Err(Error::Format));
        }
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
