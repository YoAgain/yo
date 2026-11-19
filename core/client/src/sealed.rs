//! Sealed sender (spec 6.1), libsignal's: every envelope is a sealed sender v1 message to the recipient's account
//! key, with the sender's certificate from the hub and the payload, padded to exactly one bucket. The recipient checks
//! the certificate against its pinned trust roots at the time the hub took the envelope.
//! Without a certificate an envelope goes identified: the padded payload, signed by the sending device, as Signal's.

use crate::error::{Error, Result};
use crate::identity::Key;
use crate::signal;
use libsignal_protocol::{
    sealed_sender_decrypt_to_usmc, sealed_sender_encrypt_from_usmc, Aci, CiphertextMessageType, ContentHint, DeviceId,
    Direction, IdentityChange, IdentityKey, IdentityKeyPair, IdentityKeyStore, ProtocolAddress, PublicKey,
    SenderCertificate, SignalProtocolError, Timestamp, UnidentifiedSenderMessageContent,
};
use yo_wire::envelope::{self, BUCKETS};
use yo_wire::UserId;

/// A sender certificate is fetched anew once it has less than this left (the hub's hold a day).
pub const REFRESH_MS: u64 = 2 * 3600 * 1000;

/// The two identities a sealed sender message is between, and nothing else: libsignal asks its store for them.
struct Between {
    own: IdentityKeyPair,
    them: IdentityKey,
}

type R<T> = std::result::Result<T, SignalProtocolError>;

#[async_trait::async_trait(?Send)]
impl IdentityKeyStore for Between {
    async fn get_identity_key_pair(&self) -> R<IdentityKeyPair> {
        Ok(self.own)
    }

    async fn get_local_registration_id(&self) -> R<u32> {
        Ok(0)
    }

    async fn save_identity(&mut self, _: &ProtocolAddress, _: &IdentityKey) -> R<IdentityChange> {
        Ok(IdentityChange::NewOrUnchanged)
    }

    async fn is_trusted_identity(&self, _: &ProtocolAddress, identity: &IdentityKey, _: Direction) -> R<bool> {
        Ok(*identity == self.them)
    }

    async fn get_identity(&self, _: &ProtocolAddress) -> R<Option<IdentityKey>> {
        Ok(Some(self.them))
    }
}

/// The pinned trust roots as libsignal's keys (32 bytes each, as the apps write them).
pub fn roots(keys: &[Key]) -> Vec<PublicKey> {
    keys.iter().filter_map(|k| PublicKey::from_djb_public_key_bytes(k).ok()).collect()
}

/// Seals `payload` from the holder of `own` to the account `recipient`: an envelope of exactly one bucket.
pub fn seal(
    certificate: &SenderCertificate,
    own: &IdentityKeyPair,
    recipient: &IdentityKey,
    payload: &[u8],
) -> Result<Vec<u8>> {
    let store = Between { own: *own, them: *recipient };
    // libsignal names an address; the store answers the one identity for any.
    let address = ProtocolAddress::new(String::new(), DeviceId::new(1).expect("1"));
    let make = |size: usize| -> Result<Vec<u8>> {
        let contents = envelope::pad(payload, &[size]).map_err(|_| Error::Protocol("too large"))?;
        // Plaintext: the contents are Yo's payload (a session message per device), not one of libsignal's messages.
        let usmc = UnidentifiedSenderMessageContent::new(
            CiphertextMessageType::Plaintext,
            certificate.clone(),
            contents,
            ContentHint::Default,
            None,
        )
        .map_err(signal::signal)?;
        signal::now(sealed_sender_encrypt_from_usmc(&address, &usmc, &store, &mut rand::rng())).map_err(signal::signal)
    };
    let needed = envelope::HEADER + payload.len();
    // What sealing adds (the certificate, keys, MACs, protobuf lengths), measured: the padding makes up the rest.
    let overhead = make(needed)?.len() - needed;
    for bucket in BUCKETS {
        let Some(mut size) = bucket.checked_sub(overhead).filter(|s| *s >= needed) else { continue };
        // A length's varint may grow by a byte on the way up: a few rounds settle it.
        for _ in 0..3 {
            let sealed = make(size)?;
            if sealed.len() == bucket {
                return Ok(sealed);
            }
            match (size + bucket).checked_sub(sealed.len()).filter(|s| *s >= needed) {
                Some(next) => size = next,
                None => break,
            }
        }
    }
    Err(Error::Protocol("too large"))
}

/// An opened envelope: the payload and who the certificate names (`None`: identified, the session says who).
#[derive(Debug)]
pub struct Opened {
    pub sender: Option<(UserId, u8, IdentityKey)>,
    pub payload: Vec<u8>,
}

/// Opens an envelope sealed to `own`, the certificate checked against `roots` at `received_ms`, when the hub took
/// it, as Signal checks at the server's timestamp.
pub fn open(own: &IdentityKeyPair, sealed: &[u8], roots: &[PublicKey], received_ms: u64) -> Result<Opened> {
    if !BUCKETS.contains(&sealed.len()) {
        return Err(Error::Protocol("not a bucket"));
    }
    if sealed[0] == yo_wire::VERSION {
        let payload = envelope::unpad(sealed, &BUCKETS)?.to_vec();
        return Ok(Opened { sender: None, payload });
    }
    let store = Between { own: *own, them: *own.identity_key() };
    let usmc = signal::now(sealed_sender_decrypt_to_usmc(sealed, &store))
        .map_err(|_| Error::Protocol("not sealed to this account"))?;
    let certificate = usmc.sender().map_err(signal::signal)?;
    let (user, device) = check(certificate, roots, received_ms)?;
    if usmc.msg_type().map_err(signal::signal)? != CiphertextMessageType::Plaintext {
        return Err(Error::Protocol("sealed message type"));
    }
    let contents = usmc.contents().map_err(signal::signal)?;
    let payload = envelope::unpad(contents, &[contents.len()])?.to_vec();
    let identity = IdentityKey::new(certificate.key().map_err(signal::signal)?);
    Ok(Opened { sender: Some((user, device, identity)), payload })
}

/// A certificate signed by a server certificate of one of `roots`, good at `at_ms`: the user and device it names.
fn check(certificate: &SenderCertificate, roots: &[PublicKey], at_ms: u64) -> Result<(UserId, u8)> {
    if roots.is_empty() {
        return Err(Error::Protocol("no trust root pinned"));
    }
    let good = certificate
        .validate_with_trust_roots(roots, Timestamp::from_epoch_millis(at_ms))
        .map_err(|_| Error::Protocol("sender certificate"))?;
    if !good {
        return Err(Error::Protocol("sender certificate not valid"));
    }
    let uuid = certificate.sender_uuid().map_err(signal::signal)?;
    let user = Aci::parse_from_service_id_string(uuid).ok_or(Error::Protocol("sender certificate uuid"))?;
    let device = u32::from(certificate.sender_device_id().map_err(signal::signal)?);
    let user: UserId = user.service_id_binary().try_into().map_err(|_| Error::Protocol("sender certificate uuid"))?;
    Ok((user, u8::try_from(device).map_err(|_| Error::Protocol("device"))?))
}

/// Checks a certificate the hub gave this device: from `roots`, naming this user, device and identity; its expiry.
pub fn check_own(bytes: &[u8], roots: &[PublicKey], user: &UserId, device: u8, identity: &IdentityKey) -> Result<u64> {
    let certificate = SenderCertificate::deserialize(bytes).map_err(|_| Error::Protocol("sender certificate"))?;
    let expires = certificate.expiration().map_err(signal::signal)?.epoch_millis();
    let named = check(&certificate, roots, expires)?;
    let key = certificate.key().map_err(signal::signal)?;
    if named != (*user, device) || key != *identity.public_key() {
        return Err(Error::Protocol("a sender certificate for another"));
    }
    Ok(expires)
}

/// For tests: an envelope opened and sealed again by `account_secret` with `certificate` (an insider's forgery).
#[cfg(feature = "test-support")]
pub mod testing {
    use super::*;

    /// The payload and the sender certificate of an envelope sealed to the account with `account_secret`.
    pub fn open(account_secret: &[u8; 32], sealed: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        // Identified (a request, or none in date): the payload padded, no certificate.
        if sealed.first() == Some(&yo_wire::VERSION) {
            return Ok((envelope::unpad(sealed, &BUCKETS)?.to_vec(), Vec::new()));
        }
        let own = crate::identity::account_pair(account_secret)?;
        let store = Between { own, them: *own.identity_key() };
        let usmc = signal::now(sealed_sender_decrypt_to_usmc(sealed, &store)).map_err(signal::signal)?;
        let contents = usmc.contents().map_err(signal::signal)?;
        let payload = envelope::unpad(contents, &[contents.len()])?.to_vec();
        Ok((payload, usmc.sender().map_err(signal::signal)?.serialized().map_err(signal::signal)?.to_vec()))
    }

    /// `payload` sealed by the account with `account_secret` with `certificate`, to the account `recipient`.
    pub fn seal(account_secret: &[u8; 32], certificate: &[u8], recipient: &Key, payload: &[u8]) -> Result<Vec<u8>> {
        let certificate = SenderCertificate::deserialize(certificate).map_err(signal::signal)?;
        super::seal(
            &certificate,
            &crate::identity::account_pair(account_secret)?,
            &signal::identity_of(recipient)?,
            payload,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsignal_protocol::{KeyPair, ServerCertificate};

    #[test]
    fn every_size_is_one_bucket_and_opens_only_for_its_account() {
        let root = KeyPair::generate(&mut rand::rng());
        let server = KeyPair::generate(&mut rand::rng());
        let signer = ServerCertificate::new(1, server.public_key, &root.private_key, &mut rand::rng()).unwrap();
        let (me, them) =
            (crate::identity::account_pair(&[1; 32]).unwrap(), crate::identity::account_pair(&[2; 32]).unwrap());
        let user = Aci::from_uuid_bytes([7; 16]).service_id_string();
        let device = DeviceId::new(3).unwrap();
        let expires = Timestamp::from_epoch_millis(10_000);
        let certificate = SenderCertificate::new(
            user,
            None,
            *me.public_key(),
            device,
            expires,
            signer,
            &server.private_key,
            &mut rand::rng(),
        )
        .unwrap();
        for (len, bucket) in [(1, 1024), (500, 1024), (2000, 4096), (60_000, 65536)] {
            let payload = vec![9; len];
            let sealed = seal(&certificate, &me, them.identity_key(), &payload).unwrap();
            assert_eq!(sealed.len(), bucket, "{len}");
            let opened = open(&them, &sealed, &[root.public_key], 10_000).unwrap();
            assert_eq!(opened.payload, payload);
            assert_eq!(opened.sender.map(|(u, d, _)| (u, d)), Some(([7; 16], 3)));
            assert!(open(&me, &sealed, &[root.public_key], 10_000).is_err(), "not sealed to it");
            assert!(open(&them, &sealed, &[root.public_key], 10_001).is_err(), "expired");
            assert!(open(&them, &sealed, &[server.public_key], 1).is_err(), "another root");
            assert!(open(&them, &sealed, &[], 1).is_err(), "no root pinned");
        }
        // What the largest bucket cannot hold sealed is refused, not sent another way.
        assert!(seal(&certificate, &me, them.identity_key(), &vec![9; 65_400]).is_err());
    }
}
