//! The contract between the apps and the hub (spec/v1.md): what both must read the same way, written once.
//!
//! Formats are TLS-encoded like MLS itself. This crate verifies signatures but never makes one: it holds no secret
//! key and does no I/O.

pub mod api;
pub mod cert;
pub mod content;
pub mod envelope;
pub mod limits;
pub mod phone;
pub mod request;

pub use tls_codec::{self, VLBytes};

/// The protocol version of every versioned structure here.
pub const VERSION: u8 = 1;
/// A mailbox id: 16 random bytes chosen by the device.
pub const MAILBOX_LEN: usize = 16;
/// A group id: 32 random bytes chosen by the creator.
pub const GROUP_ID_LEN: usize = 32;
/// An Ed25519 public key.
pub const KEY_LEN: usize = 32;
/// An Ed25519 signature.
pub const SIGNATURE_LEN: usize = 64;
/// An epoch token and its hash.
pub const TOKEN_LEN: usize = 32;
/// The MLS exporter label of the epoch token (spec 4.1); the context is the group id.
pub const EPOCH_TOKEN_LABEL: &str = "yo epoch token";

pub type Mailbox = [u8; MAILBOX_LEN];
pub type GroupId = [u8; GROUP_ID_LEN];

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("malformed")]
    Format,
    #[error("too large")]
    Size,
    #[error("bad signature")]
    Signature,
    #[error("bad phone number")]
    Phone,
    #[error("unsupported version or type")]
    Unsupported,
}

impl From<tls_codec::Error> for Error {
    fn from(_: tls_codec::Error) -> Self {
        Error::Format
    }
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

/// Decodes a TLS structure strictly: every byte must be used.
pub fn decode<T: tls_codec::DeserializeBytes>(bytes: &[u8]) -> Result<T, Error> {
    Ok(T::tls_deserialize_exact_bytes(bytes)?)
}

/// base64url without padding, as the request headers carry bytes.
pub fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(text: &str) -> Result<Vec<u8>, Error> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| Error::Format)
}
