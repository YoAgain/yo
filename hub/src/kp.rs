//! The hub's check of every KeyPackage it stores (spec 6.5): it hands out only what names the right person.

use openmls::prelude::{BasicCredential, Ciphersuite, CredentialType, KeyPackageIn, ProtocolVersion};
use openmls_rust_crypto::RustCrypto;
use tls_codec::Deserialize as _;
use yo_wire::cert::DeviceCertificate;
use yo_wire::limits::MAX_KEY_PACKAGE_LEN;
use yo_wire::Mailbox;

pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519;

pub struct Checked {
    pub hash: [u8; 32],
    pub last_resort: bool,
    pub certificate: DeviceCertificate,
}

/// Checks a KeyPackage: encoding, signature, lifetime, suite, credential, certificate, and that the certificate
/// is the uploader's: its account key, its mailbox.
pub fn check(crypto: &RustCrypto, bytes: &[u8], account_key: &[u8; 32], mailbox: &Mailbox) -> Option<Checked> {
    if bytes.len() > MAX_KEY_PACKAGE_LEN {
        return None;
    }
    let key_package = KeyPackageIn::tls_deserialize_exact(bytes).ok()?.validate(crypto, ProtocolVersion::Mls10).ok()?;
    if key_package.ciphersuite() != CIPHERSUITE {
        return None;
    }
    let leaf = key_package.leaf_node();
    if leaf.credential().credential_type() != CredentialType::Basic {
        return None;
    }
    let credential = BasicCredential::try_from(leaf.credential().clone()).ok()?;
    let certificate = DeviceCertificate::verify(credential.identity()).ok()?;
    let body = &certificate.body;
    let fits = &body.account_key == account_key
        && &body.mailbox == mailbox
        && body.device_key.as_slice() == leaf.signature_key().as_slice();
    fits.then(|| Checked { hash: yo_wire::sha256(bytes), last_resort: key_package.last_resort(), certificate })
}
