//! The contract between the apps and the hub (spec/v1.md): what both must read the same way, written once.
//!
//! Formats are TLS-encoded (RFC 8446 section 3). This crate verifies signatures but never makes one: it holds no secret
//! key and does no I/O.

pub mod api;
pub mod cert;
pub mod content;
pub mod devices;
pub mod envelope;
pub mod handle;
pub mod hubsig;
pub mod legacy;
pub mod limits;
pub mod request;

pub use tls_codec::{self, VLBytes};

/// The protocol version of every versioned structure here.
pub const VERSION: u8 = 1;
/// A user id: 16 random bytes the hub gives an account when it registers. It never changes.
pub const USER_ID_LEN: usize = 16;
/// A device id: 16 random bytes the device chooses; the hub's name for its inbox, never given to other users.
pub const DEVICE_ID_LEN: usize = 16;
/// A public key: Curve25519 (the account key, auth keys, the hub's key), its 32 bytes.
pub const KEY_LEN: usize = 32;
/// A signature: XEdDSA by a Curve25519 key (libsignal).
pub const SIGNATURE_LEN: usize = 64;

pub type DeviceId = [u8; DEVICE_ID_LEN];
pub type UserId = [u8; USER_ID_LEN];

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("malformed")]
    Format,
    #[error("too large")]
    Size,
    #[error("bad signature")]
    Signature,
    #[error("unsupported version or type")]
    Unsupported,
}

impl From<tls_codec::Error> for Error {
    fn from(_: tls_codec::Error) -> Self {
        Error::Format
    }
}

/// The order of Curve25519's prime-order group (`l`), little-endian: an XEdDSA signature's scalar is below it.
pub const GROUP_ORDER: [u8; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
];

/// Whether `signature` is the XEdDSA signature of the Curve25519 key `key` (its 32 bytes) over `message`, as libsignal
/// makes and checks it: the account key's (spec 3.1, 6.4), an auth key's (6.2, 7), the hub's (6.10).
/// Its scalar must be reduced (below `GROUP_ORDER`), as every signer makes it: libsignal takes it modulo the order,
/// so without this one signature would have a second encoding, which a replay cache keyed by signatures (6.2) took
/// as another.
pub fn xeddsa_verify(key: &[u8; KEY_LEN], message: &[u8], signature: &[u8]) -> bool {
    signature.len() == SIGNATURE_LEN
        && scalar_reduced(&signature[32..])
        && libsignal_core::curve::PublicKey::from_djb_public_key_bytes(key)
            .is_ok_and(|key| key.verify_signature(message, signature))
}

/// A signature's scalar (its last 32 bytes, the top bit the key's sign) below `GROUP_ORDER`.
fn scalar_reduced(s: &[u8]) -> bool {
    let top = s[31] & 0x7f;
    let rest = s[..31].iter().rev();
    // Compared from the most significant byte down.
    std::iter::once(&top).chain(rest).zip(GROUP_ORDER.iter().rev()).find(|(a, b)| a != b).is_some_and(|(a, b)| a < b)
}

/// Whether 32 bytes are a Curve25519 public key a signature can be checked against: canonical, in the prime-order
/// subgroup.
pub fn curve_key_usable(key: &[u8; KEY_LEN]) -> bool {
    libsignal_core::curve::PublicKey::from_djb_public_key_bytes(key).is_ok_and(|key| key.is_canonical())
}

/// SHA-256.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(data).into()
}

/// Encodes a TLS structure. Every structure here has bounded fields, so this cannot fail for a value that was
/// built by this crate or parsed by it.
pub fn encode<T: tls_codec::Serialize>(value: &T) -> Vec<u8> {
    value.tls_serialize_detached().expect("TLS encoding of a bounded structure")
}

/// Decodes a TLS structure strictly: every byte must be used, and the bytes must be the one encoding of what they
/// say (a length in more bytes than it needs is refused): one message, one form.
pub fn decode<T: tls_codec::DeserializeBytes + tls_codec::Serialize>(bytes: &[u8]) -> Result<T, Error> {
    let value = T::tls_deserialize_exact_bytes(bytes)?;
    if value.tls_serialize_detached().ok().as_deref() != Some(bytes) {
        return Err(Error::Format);
    }
    Ok(value)
}

/// base64url without padding, as the request headers carry bytes.
pub fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(text: &str) -> Result<Vec<u8>, Error> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(text).map_err(|_| Error::Format)
}
