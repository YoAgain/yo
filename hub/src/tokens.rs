//! The hub as Privacy Pass issuer and verifier (spec 6.10): it evaluates blinded token inputs for its devices and
//! checks the tokens deliveries bring, privately verifiable (VOPRF(P-384, SHA-384), RFC 9578). It never sees an
//! issued token before it comes back, so it cannot tell whose it is. Its key is derived from a seed in the database
//! and replaced every `token_key_days`; the one before still verifies what it issued, then it goes with the record of
//! its spent tokens.

use crate::db::Db;
use p384::NistP384;
use std::sync::{PoisonError, RwLock};
use voprf::{BlindedElement, Group as _, VoprfServer};
use yo_wire::token::{self, BatchTokenRequest, BatchTokenResponse, Token, NE, NS};

/// The keys kept: the one issuing and the one before.
const KEPT: usize = 2;

struct Key {
    /// Its row in `token_keys`.
    row: i64,
    server: VoprfServer<NistP384>,
    public: [u8; NE],
    id: [u8; 32],
    created_at: u64,
}

impl Key {
    fn derive(row: i64, seed: &[u8], created_at: u64) -> Option<Key> {
        // RFC 9578, 5.5: DeriveKeyPair(seed, "PrivacyPass").
        let server = VoprfServer::<NistP384>::new_from_seed(seed, b"PrivacyPass").ok()?;
        let public: [u8; NE] = NistP384::serialize_elem(server.get_public_key()).into();
        Some(Key { row, server, id: token::token_key_id(&public), public, created_at })
    }
}

/// Why an issuance is refused: `422` in the batched-tokens draft.
#[derive(Debug, PartialEq, Eq)]
pub struct Unprocessable;

#[derive(Default)]
pub struct Issuer {
    /// Newest first.
    keys: RwLock<Vec<Key>>,
}

impl Issuer {
    /// The keys in the database, a first one made if there is none or the newest is due (`rotate`).
    pub fn open(db: &Db, now: u64, every_ms: u64) -> rusqlite::Result<Issuer> {
        let keys = db
            .token_keys()?
            .into_iter()
            .take(KEPT)
            .filter_map(|(row, seed, created_at)| Key::derive(row, &seed, created_at))
            .collect();
        let issuer = Issuer { keys: RwLock::new(keys) };
        issuer.rotate(db, now, every_ms)?;
        Ok(issuer)
    }

    /// A new key when the newest is `every_ms` old (or there is none); the one before stays to verify, older ones go
    /// with their spent tokens. A new key's truncated id differs from the one before's, so an issuance request names
    /// one key only.
    pub fn rotate(&self, db: &Db, now: u64, every_ms: u64) -> rusqlite::Result<()> {
        let due = self.keys().first().is_none_or(|key| now.saturating_sub(key.created_at) >= every_ms);
        if !due {
            return Ok(());
        }
        let truncated = self.keys().first().map(|key| key.id[31]);
        let (seed, key) = loop {
            let seed: [u8; NS] = random();
            let key = Key::derive(0, &seed, now).expect("a random seed derives a key");
            if Some(key.id[31]) != truncated {
                break (seed, key);
            }
        };
        let row = db.add_token_key(&seed, now, KEPT)?;
        let mut keys = self.keys.write().unwrap_or_else(PoisonError::into_inner);
        keys.insert(0, Key { row, ..key });
        keys.truncate(KEPT);
        Ok(())
    }

    fn keys(&self) -> std::sync::RwLockReadGuard<'_, Vec<Key>> {
        self.keys.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// The issuer directory (RFC 9578, section 4): where to ask, and the key issuing now. Only that one: a client
    /// makes tokens for the first key listed.
    pub fn directory(&self) -> String {
        let keys = self.keys();
        let listed: Vec<_> = keys
            .first()
            .map(|key| serde_json::json!({ "token-type": token::TOKEN_TYPE, "token-key": token::b64_padded(&key.public) }))
            .into_iter()
            .collect();
        serde_json::json!({ "issuer-request-uri": yo_wire::api::TOKENS, "token-keys": listed }).to_string()
    }

    /// Evaluates a batch of blinded elements with the key issuing now, with one proof for all. Refused: another token
    /// type or key, more than `limit` elements or none, an element that is no point.
    pub fn issue(&self, request: &BatchTokenRequest, limit: usize) -> Result<BatchTokenResponse, Unprocessable> {
        let n = request.blinded_elements.len();
        if request.token_type != token::TOKEN_TYPE || n == 0 || n > limit {
            return Err(Unprocessable);
        }
        let keys = self.keys();
        let key = keys.first().filter(|key| key.id[31] == request.truncated_token_key_id).ok_or(Unprocessable)?;
        let elements = request
            .blinded_elements
            .iter()
            .map(|element| BlindedElement::<NistP384>::deserialize(element))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| Unprocessable)?;
        let evaluated = key.server.batch_blind_evaluate(&mut rand_core::OsRng, &elements).map_err(|_| Unprocessable)?;
        Ok(BatchTokenResponse {
            evaluated_elements: evaluated.messages.iter().map(|m| m.serialize().into()).collect(),
            evaluated_proof: evaluated.proof.serialize().as_slice().try_into().expect("two scalars"),
        })
    }

    /// The row of the key a token was issued under, if it is a token of this hub's for Yo's challenge and its
    /// authenticator holds. Whether it was spent is the database's to say.
    pub fn verify(&self, token: &Token) -> Option<i64> {
        if token.token_type != token::TOKEN_TYPE || token.challenge_digest != token::challenge_digest() {
            return None;
        }
        let keys = self.keys();
        let key = keys.iter().find(|key| key.id == token.token_key_id)?;
        let expected = key.server.evaluate(&token.input()).ok()?;
        let same = expected.iter().zip(token.authenticator).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
        same.then_some(key.row)
    }
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).expect("random");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use yo_wire::token::vector;

    fn issuer_of(secret: &[u8]) -> Issuer {
        let server = VoprfServer::<NistP384>::new_with_key(secret).unwrap();
        let public: [u8; NE] = NistP384::serialize_elem(server.get_public_key()).into();
        let key = Key { row: 7, server, id: token::token_key_id(&public), public, created_at: 0 };
        Issuer { keys: RwLock::new(vec![key]) }
    }

    #[test]
    fn verifies_rfc_9578s_token_as_its_own_issuer_would() {
        // Test vector 1's key, with its challenge in place of Yo's: the authenticator is over that challenge.
        let issuer = issuer_of(&vector::bytes(vector::SK));
        let keys = issuer.keys();
        assert_eq!(keys[0].public.to_vec(), vector::bytes(vector::PK));
        let token: Token = yo_wire::decode(&vector::bytes(vector::TOKEN)).unwrap();
        let expected = keys[0].server.evaluate(&token.input()).unwrap();
        assert_eq!(expected.to_vec(), token.authenticator.to_vec());
        drop(keys);
        assert_eq!(issuer.verify(&token), None, "not Yo's challenge");
    }

    #[test]
    fn issues_only_for_its_current_key_and_within_the_limit() {
        let issuer = issuer_of(&vector::bytes(vector::SK));
        let request: [u8; 52] = vector::bytes(vector::REQUEST).try_into().unwrap();
        let element: [u8; NE] = request[3..].try_into().unwrap();
        let batch = |key: u8, n: usize| BatchTokenRequest {
            token_type: token::TOKEN_TYPE,
            truncated_token_key_id: key,
            blinded_elements: vec![element; n],
        };
        let response = issuer.issue(&batch(request[2], 1), 100).unwrap();
        // The evaluation is deterministic, the proof not: the element is the vector's.
        assert_eq!(response.evaluated_elements[0].to_vec(), vector::bytes(vector::RESPONSE)[..NE].to_vec());
        assert_eq!(issuer.issue(&batch(request[2].wrapping_add(1), 1), 100), Err(Unprocessable));
        assert_eq!(issuer.issue(&batch(request[2], 0), 100), Err(Unprocessable));
        assert_eq!(issuer.issue(&batch(request[2], 101), 100), Err(Unprocessable));
        let mut bad = batch(request[2], 1);
        bad.blinded_elements[0][0] = 9;
        assert_eq!(issuer.issue(&bad, 100), Err(Unprocessable));
    }

    #[test]
    fn keys_are_kept_in_the_database_and_replaced_when_due() {
        let db = Db::open(":memory:").unwrap();
        let day = 24 * 3600 * 1000;
        let issuer = Issuer::open(&db, day, 30 * day).unwrap();
        let first = issuer.keys()[0].public;
        assert_eq!(Issuer::open(&db, 2 * day, 30 * day).unwrap().keys()[0].public, first, "the same after a restart");
        issuer.rotate(&db, 31 * day, 30 * day).unwrap();
        let second = issuer.keys()[0].public;
        assert_ne!(second, first);
        assert_eq!(issuer.keys()[1].public, first, "the one before still verifies");
        issuer.rotate(&db, 61 * day, 30 * day).unwrap();
        assert_eq!(db.token_keys().unwrap().len(), KEPT, "the oldest is gone");
        assert_eq!(issuer.keys()[1].public, second);
        let directory: serde_json::Value = serde_json::from_str(&issuer.directory()).unwrap();
        assert_eq!(directory["token-keys"].as_array().unwrap().len(), 1);
        assert_eq!(directory["token-keys"][0]["token-type"], 1);
    }
}
