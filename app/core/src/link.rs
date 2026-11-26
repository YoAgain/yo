//! Linking a device (spec 2.5). The new device opens a link (`start`), shows its `text` (a QR code, or text to paste)
//! and waits (`wait`); the old device reads it (`offer`), and only after the owner's tap (the app's) seals the account
//! to it (`Client::link_approve`). The new device then registers with the bundle's account secret as an added device
//! (`Client::register_username` with it, `legacy_storage_key` from `Linked::legacy_storage_key`) and loads the storage.
use crate::error::{Error, Result};
use crate::{Client, Transport};
use hpke::{Deserializable, OpModeR, OpModeS, Serializable};
use yoip::net::api::{self, status};
use yoip::{b64, decode, encode, unb64};

type Kem = hpke::kem::X25519HkdfSha256;
type Kdf = hpke::kdf::HkdfSha256;
type Aead = hpke::aead::AesGcm128;

/// What the text a new device shows starts with.
pub const PREFIX: &str = "yo-link:";
const INFO: &[u8] = b"yo link bundle";
const ENC_LEN: usize = 32;

/// A link this device opened: its id, the one-time key's secret (keep it until `wait` has the bundle), and the text
/// to show (`yo-link:<id>.<public key>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: String,
    pub secret: [u8; 32],
    pub text: String,
}

/// A link another device showed, read from its text: where and to whom the bundle goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub id: String,
    pub key: [u8; 32],
}

/// What the old device handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    pub username: String,
    pub account_secret: [u8; 32],
    /// The storage key if it is not the one the account secret gives (an account migrated from the old keys).
    pub legacy_storage_key: Option<[u8; 32]>,
}

fn keys(secret: &[u8; 32]) -> (<Kem as hpke::Kem>::PrivateKey, <Kem as hpke::Kem>::PublicKey) {
    <Kem as hpke::Kem>::derive_keypair(secret)
}

fn id_bytes(id: &str) -> Result<[u8; 16]> {
    unb64(id).ok().and_then(|id| id.try_into().ok()).ok_or(Error::Protocol("link id"))
}

/// Opens a link at the hub (anonymous) with a new one-time key. `Refused(404)`: the hub adds no devices.
pub fn start(transport: &dyn Transport) -> Result<Request> {
    let response = transport.request("POST", api::LINK, &[], &[]).map_err(Error::Network)?;
    if response.status != status::OK {
        return Err(Error::Refused(response.status));
    }
    let id = String::from_utf8(response.body).map_err(|_| Error::Protocol("link id"))?;
    id_bytes(&id)?;
    let secret = crate::identity::random::<32>();
    let public = keys(&secret).1.to_bytes();
    let text = format!("{PREFIX}{id}.{}", b64(&public));
    Ok(Request { id, secret, text })
}

/// Reads a link's text (`yo-link:<id>.<public key>`). Nothing is sent: the owner confirms first.
pub fn offer(text: &str) -> Result<Offer> {
    let (id, key) =
        text.trim().strip_prefix(PREFIX).and_then(|rest| rest.split_once('.')).ok_or(Error::Protocol("link"))?;
    id_bytes(id)?;
    let key: [u8; 32] = unb64(key).ok().and_then(|k| k.try_into().ok()).ok_or(Error::Protocol("link key"))?;
    Ok(Offer { id: id.to_owned(), key })
}

/// Waits up to `wait_s` for the bundle (the hub ends the link as it hands it out): `None` none yet, `Refused(404)` the
/// link is over (expired or taken).
pub fn wait(transport: &dyn Transport, request: &Request, wait_s: u64) -> Result<Option<Linked>> {
    let path = format!("{}?wait={wait_s}", api::link_path(&request.id));
    let response = transport.request("GET", &path, &[], &[]).map_err(Error::Network)?;
    match response.status {
        status::OK => open(request, &response.body).map(Some),
        status::NO_CONTENT => Ok(None),
        other => Err(Error::Refused(other)),
    }
}

/// Opens a sealed bundle: `enc` (32 bytes), then the ciphertext; the link id is the aad.
pub fn open(request: &Request, sealed: &[u8]) -> Result<Linked> {
    let bad = || Error::Protocol("link bundle");
    if sealed.len() <= ENC_LEN {
        return Err(bad());
    }
    let (enc, ciphertext) = sealed.split_at(ENC_LEN);
    let enc = <Kem as hpke::Kem>::EncappedKey::from_bytes(enc).map_err(|_| bad())?;
    let aad = id_bytes(&request.id)?;
    let plain = hpke::single_shot_open::<Aead, Kdf, Kem>(
        &OpModeR::Base,
        &keys(&request.secret).0,
        &enc,
        INFO,
        ciphertext,
        &aad,
    )
    .map_err(|_| bad())?;
    let bundle: api::LinkBundle = decode(&plain)?;
    let username = String::from_utf8(bundle.username.as_slice().to_vec()).map_err(|_| bad())?;
    let derived = yoip::protocol::storage::key(&bundle.account_secret);
    let legacy_storage_key = (bundle.storage_key != derived).then_some(bundle.storage_key);
    Ok(Linked { username, account_secret: bundle.account_secret, legacy_storage_key })
}

/// Seals a bundle to `offer`'s key, its id as aad.
pub fn seal(offer: &Offer, bundle: &api::LinkBundle) -> Result<Vec<u8>> {
    let key = <Kem as hpke::Kem>::PublicKey::from_bytes(&offer.key).map_err(|_| Error::Protocol("link key"))?;
    let aad = id_bytes(&offer.id)?;
    let (enc, ciphertext) = hpke::single_shot_seal::<Aead, Kdf, Kem>(&OpModeS::Base, &key, INFO, &encode(bundle), &aad)
        .map_err(|_| Error::Internal("hpke"))?;
    Ok([enc.to_bytes().as_slice(), &ciphertext].concat())
}

impl Client {
    /// The owner confirmed (spec 2.5: never without that tap): seals this account (username, account secret, storage
    /// key) to the new device and leaves it at the link. `Refused(404)`: the link is over; `Refused(409)`: answered.
    pub fn link_approve(&mut self, offer: &Offer) -> Result<()> {
        let username = self.state.username.clone().ok_or(Error::Protocol("no username"))?;
        let storage_key =
            self.state.legacy_storage_key.unwrap_or_else(|| yoip::protocol::storage::key(&self.state.account_secret));
        let bundle = api::LinkBundle {
            username: username.into_bytes().into(),
            account_secret: self.state.account_secret,
            storage_key,
        };
        let sealed = seal(offer, &bundle)?;
        let response = self.signed("PUT", &api::link_path(&offer.id), &sealed)?;
        crate::expect(response, status::NO_CONTENT).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_opens_only_with_its_key_and_link_id() {
        let secret = [7; 32];
        let id = b64(&[1; 16]);
        let request =
            Request { id: id.clone(), secret, text: format!("{PREFIX}{id}.{}", b64(&keys(&secret).1.to_bytes())) };
        let offer = offer(&request.text).unwrap();
        let bundle =
            api::LinkBundle { username: b"Max".to_vec().into(), account_secret: [3; 32], storage_key: [4; 32] };
        let sealed = seal(&offer, &bundle).unwrap();
        let linked = open(&request, &sealed).unwrap();
        assert_eq!((linked.username.as_str(), linked.account_secret), ("Max", [3; 32]));
        assert_eq!(linked.legacy_storage_key, Some([4; 32]));
        let other = Request { id: b64(&[2; 16]), ..request.clone() };
        assert!(open(&other, &sealed).is_err(), "another link id");
        let stranger = Request { secret: [8; 32], ..request.clone() };
        assert!(open(&stranger, &sealed).is_err(), "another key");
        assert!(super::offer("yo-link:nonsense").is_err());
    }
}
