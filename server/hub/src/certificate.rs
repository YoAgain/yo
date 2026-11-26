//! Sender certificates (spec 4.1). The file: server private key (32) || trust root public key (32) || certificate.

use libsignal_protocol::{
    Aci, DeviceId, KeyPair, PrivateKey, PublicKey, SenderCertificate, ServerCertificate, Timestamp,
};
use yoip::{UserId, KEY_LEN};

/// How long a sender certificate holds (Signal's: a day).
pub const LIFETIME_MS: u64 = 24 * 3600 * 1000;

pub struct Signer {
    key: PrivateKey,
    root: [u8; 32],
    certificate: ServerCertificate,
}

impl Signer {
    /// The signer in a file's bytes: its server certificate signed by the root it names, for its key.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() <= 64 {
            return Err("too short".into());
        }
        let key = PrivateKey::deserialize(&bytes[..32]).map_err(|e| format!("server key: {e}"))?;
        let root: [u8; 32] = bytes[32..64].try_into().expect("32 bytes");
        let certificate =
            ServerCertificate::deserialize(&bytes[64..]).map_err(|e| format!("server certificate: {e}"))?;
        let root_key = PublicKey::from_djb_public_key_bytes(&root).map_err(|e| format!("trust root: {e}"))?;
        if !certificate.validate(&root_key).map_err(|e| e.to_string())? {
            return Err("the server certificate is not signed by its trust root".into());
        }
        if certificate.public_key().map_err(|e| e.to_string())? != key.public_key().map_err(|e| e.to_string())? {
            return Err("the server certificate is for another key".into());
        }
        Ok(Signer { key, root, certificate })
    }

    /// The trust root's public key, 32 bytes: served at `/.well-known/yo-trust-root`.
    pub fn root(&self) -> [u8; 32] {
        self.root
    }

    /// A sender certificate for a user's device (spec 3.1), good until a day from `now_ms`.
    pub fn issue(
        &self,
        user: &UserId,
        device: u8,
        account_key: &[u8; KEY_LEN],
        now_ms: u64,
    ) -> Result<Vec<u8>, String> {
        let identity = PublicKey::from_djb_public_key_bytes(account_key).map_err(|e| e.to_string())?;
        let device = DeviceId::new(device).map_err(|_| "device number".to_string())?;
        let certificate = SenderCertificate::new(
            Aci::from_uuid_bytes(*user).service_id_string(),
            None,
            identity,
            device,
            Timestamp::from_epoch_millis(now_ms + LIFETIME_MS),
            self.certificate.clone(),
            &self.key,
            &mut rand::rng(),
        )
        .map_err(|e| e.to_string())?;
        Ok(certificate.serialized().map_err(|e| e.to_string())?.to_vec())
    }
}

/// A server key and its certificate by `root` (a trust root's private key), as a signer's file.
pub fn server_file(root: &[u8], key_id: u32) -> Result<Vec<u8>, String> {
    let root = PrivateKey::deserialize(root).map_err(|e| format!("trust root: {e}"))?;
    let server = KeyPair::generate(&mut rand::rng());
    let certificate =
        ServerCertificate::new(key_id, server.public_key, &root, &mut rand::rng()).map_err(|e| e.to_string())?;
    let root_public = root.public_key().map_err(|e| e.to_string())?;
    let mut file = server.private_key.serialize().to_vec();
    file.extend_from_slice(root_public.public_key_bytes());
    file.extend_from_slice(certificate.serialized().map_err(|e| e.to_string())?);
    Ok(file)
}

/// A new trust root: its private key (32 bytes) and its public key (32).
pub fn new_root() -> (Vec<u8>, [u8; 32]) {
    let pair = KeyPair::generate(&mut rand::rng());
    let public = pair.public_key.public_key_bytes().try_into().expect("32 bytes");
    (pair.private_key.serialize().to_vec(), public)
}

/// The signer file of the test hubs: a server key under the test trust root (`yoip::protocol::envelope::TEST_TRUST_ROOT`).
pub fn test_file() -> Vec<u8> {
    server_file(&yoip::protocol::envelope::TEST_TRUST_ROOT, 1).expect("the test trust root")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signer_issues_what_its_root_vouches_for() {
        let signer = Signer::from_bytes(&test_file()).unwrap();
        let root = PublicKey::from_djb_public_key_bytes(&signer.root()).unwrap();
        let account = KeyPair::generate(&mut rand::rng()).public_key.public_key_bytes().try_into().unwrap();
        let bytes = signer.issue(&[7; 16], 2, &account, 1_000).unwrap();
        let certificate = SenderCertificate::deserialize(&bytes).unwrap();
        assert!(certificate.validate(&root, Timestamp::from_epoch_millis(1_000 + LIFETIME_MS)).unwrap());
        assert!(!certificate.validate(&root, Timestamp::from_epoch_millis(1_001 + LIFETIME_MS)).unwrap());
        assert_eq!(u32::from(certificate.sender_device_id().unwrap()), 2);
        // Another root's file does not pass for this one's; a file cut short is refused.
        let (other, _) = new_root();
        let mut mixed = server_file(&other, 1).unwrap();
        mixed[32..64].copy_from_slice(&signer.root());
        assert!(Signer::from_bytes(&mixed).is_err());
        assert!(Signer::from_bytes(&test_file()[..64]).is_err());
    }
}
