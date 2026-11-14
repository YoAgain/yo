//! Privacy Pass tokens for deliveries (spec 6.10), the client's side: a batch of token inputs blinded for the hub's
//! key, its answer checked against that key (the proof) and unblinded into tokens the hub has never seen.

use p384::NistP384;
use voprf::{EvaluationElement, Group as _, Proof, VoprfClient};
use yo_wire::token::{self, BatchTokenRequest, BatchTokenResponse, Token, NE};

/// The key a hub issues tokens under now, from its issuer directory (RFC 9578, section 4): the first of type 0x0001
/// whose `not-before` has passed. `None` when there is none, or the directory does not read.
pub fn directory_key(json: &[u8], now_s: u64) -> Option<[u8; NE]> {
    let directory: serde_json::Value = serde_json::from_slice(json).ok()?;
    directory["token-keys"].as_array()?.iter().find_map(|key| {
        let usable = key["token-type"].as_u64() == Some(token::TOKEN_TYPE as u64)
            && key["not-before"].as_u64().is_none_or(|not_before| not_before <= now_s);
        let public: [u8; NE] = token::unb64_padded(key["token-key"].as_str()?)?.try_into().ok()?;
        (usable && NistP384::deserialize_elem(&public).is_ok()).then_some(public)
    })
}

/// A batch on its way: the request, and what unblinds its answer.
pub struct Batch {
    pub request: BatchTokenRequest,
    public_key: [u8; NE],
    nonces: Vec<[u8; 32]>,
    clients: Vec<VoprfClient<NistP384>>,
}

/// `n` token inputs with fresh nonces, blinded for `public_key`.
pub fn request(public_key: &[u8; NE], n: usize) -> Option<Batch> {
    let key_id = token::token_key_id(public_key);
    let mut nonces = Vec::with_capacity(n);
    let mut clients = Vec::with_capacity(n);
    let mut blinded_elements = Vec::with_capacity(n);
    for _ in 0..n {
        let nonce: [u8; 32] = crate::identity::random();
        let blinded =
            VoprfClient::<NistP384>::blind(&token::token_input(&nonce, &key_id), &mut rand_core::OsRng).ok()?;
        blinded_elements.push(blinded.message.serialize().into());
        nonces.push(nonce);
        clients.push(blinded.state);
    }
    let request =
        BatchTokenRequest { token_type: token::TOKEN_TYPE, truncated_token_key_id: key_id[31], blinded_elements };
    Some(Batch { request, public_key: *public_key, nonces, clients })
}

/// The tokens of an answer, if its proof holds for the key and every element of the request.
pub fn finish(batch: &Batch, response: &BatchTokenResponse) -> Option<Vec<Token>> {
    if response.evaluated_elements.len() != batch.clients.len() {
        return None;
    }
    let messages = response
        .evaluated_elements
        .iter()
        .map(|element| EvaluationElement::<NistP384>::deserialize(element))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let proof = Proof::<NistP384>::deserialize(&response.evaluated_proof).ok()?;
    let public = NistP384::deserialize_elem(&batch.public_key).ok()?;
    let key_id = token::token_key_id(&batch.public_key);
    let inputs: Vec<Vec<u8>> = batch.nonces.iter().map(|nonce| token::token_input(nonce, &key_id)).collect();
    let outputs = VoprfClient::batch_finalize(&inputs, &batch.clients, &messages, &proof, public).ok()?;
    batch
        .nonces
        .iter()
        .zip(outputs)
        .map(|(nonce, output)| {
            Some(Token {
                token_type: token::TOKEN_TYPE,
                nonce: *nonce,
                challenge_digest: token::challenge_digest(),
                token_key_id: key_id,
                authenticator: output.ok()?.as_slice().try_into().ok()?,
            })
        })
        .collect()
}

/// A token as the state keeps it: its nonce and authenticator (hex); the rest is the same for all of one key.
pub fn kept(token: &Token) -> String {
    crate::state::hex(&[&token.nonce[..], &token.authenticator[..]].concat())
}

/// A kept token, whole again, for the key it was made for.
pub fn unkept(kept: &str, public_key: &[u8; NE]) -> Option<Token> {
    let bytes = crate::state::unhex(kept)?;
    if bytes.len() != 32 + token::NK {
        return None;
    }
    Some(Token {
        token_type: token::TOKEN_TYPE,
        nonce: bytes[..32].try_into().ok()?,
        challenge_digest: token::challenge_digest(),
        token_key_id: token::token_key_id(public_key),
        authenticator: bytes[32..].try_into().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use yo_wire::token::vector;

    #[test]
    fn finishes_rfc_9578s_token_from_its_blind_and_answer() {
        // Test vector 1: the client's state is its blind and blinded element; the answer one element and its proof.
        let request = vector::bytes(vector::REQUEST);
        let client =
            VoprfClient::<NistP384>::deserialize(&[vector::bytes(vector::BLIND), request[3..].to_vec()].concat())
                .unwrap();
        let public_key: [u8; NE] = vector::bytes(vector::PK).try_into().unwrap();
        let response = vector::bytes(vector::RESPONSE);
        let batch = Batch {
            request: BatchTokenRequest {
                token_type: token::TOKEN_TYPE,
                truncated_token_key_id: request[2],
                blinded_elements: vec![request[3..].try_into().unwrap()],
            },
            public_key,
            nonces: vec![vector::bytes(vector::NONCE).try_into().unwrap()],
            clients: vec![client],
        };
        let answer = BatchTokenResponse {
            evaluated_elements: vec![response[..NE].try_into().unwrap()],
            evaluated_proof: response[NE..].try_into().unwrap(),
        };
        // `finish` makes Yo's challenge; the vector's has another, so compare the authenticator by hand.
        let messages = [EvaluationElement::<NistP384>::deserialize(&answer.evaluated_elements[0]).unwrap()];
        let proof = Proof::<NistP384>::deserialize(&answer.evaluated_proof).unwrap();
        let expected: Token = yo_wire::decode(&vector::bytes(vector::TOKEN)).unwrap();
        let output = VoprfClient::batch_finalize(
            &[expected.input()],
            &batch.clients,
            &messages,
            &proof,
            NistP384::deserialize_elem(&public_key).unwrap(),
        )
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
        assert_eq!(output.to_vec(), expected.authenticator.to_vec());
        assert_eq!(request[2], token::token_key_id(&public_key)[31]);
        // A proof for another key fails.
        let mut other = answer.clone();
        other.evaluated_proof[5] ^= 1;
        assert!(finish(&batch, &other).is_none());
    }

    #[test]
    fn the_directory_names_the_first_usable_key() {
        let key = token::b64_padded(&vector::bytes(vector::PK));
        let json = format!(
            r#"{{"issuer-request-uri":"/v1/tokens","token-keys":[{{"token-type":2,"token-key":"{key}"}},
            {{"token-type":1,"token-key":"{key}","not-before":2000}},{{"token-type":1,"token-key":"{key}"}}]}}"#
        );
        assert_eq!(directory_key(json.as_bytes(), 1000).map(|k| k.to_vec()), Some(vector::bytes(vector::PK)));
        assert!(directory_key(br#"{"token-keys":[{"token-type":1,"token-key":"AAAA"}]}"#, 0).is_none());
        assert!(directory_key(b"<html>", 0).is_none());
        let token: Token = yo_wire::decode(&vector::bytes(vector::TOKEN)).unwrap();
        let public_key: [u8; NE] = vector::bytes(vector::PK).try_into().unwrap();
        let again = unkept(&kept(&token), &public_key).unwrap();
        assert_eq!(
            (again.nonce, again.authenticator, again.token_key_id),
            (token.nonce, token.authenticator, token.token_key_id)
        );
    }
}
