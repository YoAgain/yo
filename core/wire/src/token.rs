//! Privacy Pass tokens (spec 6.10): a delivery may carry one instead of being counted against its address. The hub
//! issues them to its devices (signed) and takes them back from anyone (unsigned), without learning which issued
//! token comes back: privately verifiable tokens, type 0x0001, VOPRF(P-384, SHA-384) (RFC 9578, section 5), issued
//! in batches (draft-ietf-privacypass-batched-tokens, section 5), redeemed with `Authorization: PrivateToken`
//! (RFC 9577). Only the encodings are here; the VOPRF is in the hub and the core.

use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize};

/// VOPRF(P-384, SHA-384).
pub const TOKEN_TYPE: u16 = 0x0001;
/// A serialized P-384 element (compressed point): a key, a blinded or evaluated element.
pub const NE: usize = 49;
/// A serialized P-384 scalar.
pub const NS: usize = 48;
/// An authenticator: a SHA-384 output.
pub const NK: usize = 48;
/// The issuer's name in the challenge. Not the hub's host name: a hub is reached under several (a tunnel, a test
/// address), and its key alone tells hubs apart.
pub const ISSUER_NAME: &str = "yo";
/// Where the hub publishes its key (RFC 9578, section 4).
pub const DIRECTORY: &str = "/.well-known/private-token-issuer-directory";
/// The authentication scheme (RFC 9577).
pub const SCHEME: &str = "PrivateToken";

/// The one TokenChallenge (RFC 9577, section 2.1.1) every Yo token answers: this issuer, no redemption context, no
/// origin. Clients make it themselves; the hub sends none.
pub fn challenge() -> Vec<u8> {
    let mut out = TOKEN_TYPE.to_be_bytes().to_vec();
    out.extend_from_slice(&(ISSUER_NAME.len() as u16).to_be_bytes());
    out.extend_from_slice(ISSUER_NAME.as_bytes());
    out.push(0); // redemption_context<0..32>: empty
    out.extend_from_slice(&0u16.to_be_bytes()); // origin_info<0..2^16-1>: empty
    out
}

/// SHA-256 of `challenge()`.
pub fn challenge_digest() -> [u8; 32] {
    crate::sha256(&challenge())
}

/// The key id of an issuer key: SHA-256 of its serialized element (RFC 9578, section 5.5).
pub fn token_key_id(public_key: &[u8; NE]) -> [u8; 32] {
    crate::sha256(public_key)
}

/// A token as redeemed (RFC 9577, section 2.2.1).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Token {
    pub token_type: u16,
    /// Chosen by the client, random: what the hub remembers against a second spending.
    pub nonce: [u8; 32],
    pub challenge_digest: [u8; 32],
    pub token_key_id: [u8; 32],
    pub authenticator: [u8; NK],
}

/// What the VOPRF is evaluated on, at issuance (blinded) and at redemption: the token without its authenticator.
pub fn token_input(nonce: &[u8; 32], token_key_id: &[u8; 32]) -> Vec<u8> {
    let mut out = TOKEN_TYPE.to_be_bytes().to_vec();
    out.extend_from_slice(nonce);
    out.extend_from_slice(&challenge_digest());
    out.extend_from_slice(token_key_id);
    out
}

impl Token {
    pub fn input(&self) -> Vec<u8> {
        let mut out = self.token_type.to_be_bytes().to_vec();
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.challenge_digest);
        out.extend_from_slice(&self.token_key_id);
        out
    }

    /// The `Authorization` header's value: `PrivateToken token="..."`, base64url with padding (RFC 9577, 2.2.2).
    pub fn authorization(&self) -> String {
        use base64::Engine;
        let token = base64::engine::general_purpose::URL_SAFE.encode(crate::encode(self));
        format!("{SCHEME} token=\"{token}\"")
    }

    /// A token from an `Authorization` header's value; `None` for any other scheme or anything malformed. Unknown
    /// parameters are ignored, the value may be quoted or not, padded or not.
    pub fn from_authorization(value: &str) -> Option<Token> {
        let (scheme, params) = value.trim().split_once(' ')?;
        if !scheme.eq_ignore_ascii_case(SCHEME) {
            return None;
        }
        let token = params.split(',').find_map(|param| {
            let (name, value) = param.trim().split_once('=')?;
            name.trim().eq_ignore_ascii_case("token").then(|| value.trim().trim_matches('"'))
        })?;
        crate::decode(&unb64_padded(token)?).ok()
    }
}

/// base64url, padded or not (RFC 9577 and RFC 9578 pad; a client may not).
pub fn unb64_padded(text: &str) -> Option<Vec<u8>> {
    crate::unb64(text.trim_end_matches('=')).ok()
}

/// base64url with padding, as the issuer directory writes a key.
pub fn b64_padded(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE.encode(bytes)
}

/// `POST /v1/tokens` (signed): blinded token inputs, all for the issuer's current key (the batched-tokens draft's
/// AmortizedBatchTokenRequest).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct BatchTokenRequest {
    pub token_type: u16,
    /// The last byte of the key id.
    pub truncated_token_key_id: u8,
    pub blinded_elements: Vec<[u8; NE]>,
}

/// Its answer: the elements evaluated, in order, and one proof for all of them (AmortizedBatchTokenResponse).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct BatchTokenResponse {
    pub evaluated_elements: Vec<[u8; NE]>,
    pub evaluated_proof: [u8; 2 * NS],
}

/// RFC 9578, A.1, test vector 1 (hex): for the tests of both sides.
pub mod vector {
    pub const SK: &str =
        "39b0d04d3732459288fc5edb89bb02c2aa42e06709f201d6c518871d518114910bee3c919bed1bbffe3fc1b87d53240a";
    pub const PK: &str =
        "02d45bf522425cdd2227d3f27d245d9d563008829252172d34e48469290c21da1a46d42ca38f7beabdf05c074aee1455bf";
    pub const CHALLENGE: &str = concat!(
        "0001000e6973737565722e6578616d706c65205de58a52fcdaef25ca3f65448d04e040fb1924e8264acfccfc6c5ad451d582b3000e",
        "6f726967696e2e6578616d706c65"
    );
    pub const NONCE: &str = "6aa422c41b59d3e44a136dd439df2454e3587ee5f3697798cdc05fafe73073b8";
    pub const BLIND: &str =
        "8e7fd80970b8a00b0931b801a2e22d9903d83bd5597c6a4dc1496ed2b17ef820445ef3bd223f3ab2c4f54c5d1c956909";
    pub const REQUEST: &str = concat!(
        "0001f4030ab3e23181d1e213f24315f5775983c678ce22eff9427610832ab3900f2cd12d6829a07ec8a6813cf0b5b886f4cc497",
        "9"
    );
    pub const RESPONSE: &str = concat!(
        "036bb3c5c397d88c3527cf9f08f1fe63687b867e85c930c49ee2c222408d4903722a19ff272ac97e3725b947c972784ebfe86e",
        "b9ea54336e4334ea9660212c0c85fbadfbf491a1ce2446fc3379337fccd45c1059b2bc760110ee1ec227d8e01c9f482c00c47ff",
        "a0dbe2fb58c32dde2b1dbe69fff920a528e68dd9b3c2483848e57c30542b8984fa6bfecd6d71d54d53eda"
    );
    pub const TOKEN: &str = concat!(
        "00016aa422c41b59d3e44a136dd439df2454e3587ee5f3697798cdc05fafe73073b8501370b494089dc462802af545e6380958",
        "1ee6ef57890a12105c28368169514bf260d0792bf7f46c9866a6d37c3032d8714415f87f5f6903d7fb071e253be2f4e0a835d7",
        "6528b8444f73789ee7dc90715b01c17902fd87375c00a7a9d3d92540437f470773be20f71e721da3af40edeb"
    );

    pub fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::vector::bytes as unhex;
    use super::*;

    #[test]
    fn the_challenge_is_rfc_9577s_structure() {
        // RFC 9578, A.1, test vector 4: issuer "issuer.example", no context, no origin; ours names "yo".
        let vector = unhex("0001000e6973737565722e6578616d706c65000000");
        assert_eq!(&vector[..2], &challenge()[..2]);
        assert_eq!(&vector[vector.len() - 3..], &challenge()[challenge().len() - 3..]);
        assert_eq!(challenge(), [0, 1, 0, 2, b'y', b'o', 0, 0, 0]);
    }

    #[test]
    fn a_token_reads_as_rfc_9578_writes_it() {
        // RFC 9578, A.1, test vector 1.
        let bytes = unhex(vector::TOKEN);
        let token: Token = crate::decode(&bytes).unwrap();
        assert_eq!(token.token_type, TOKEN_TYPE);
        assert_eq!(crate::encode(&token), bytes);
        let public_key: [u8; NE] = unhex(vector::PK).try_into().unwrap();
        assert_eq!(token.token_key_id, token_key_id(&public_key));
        let challenge = unhex(vector::CHALLENGE);
        assert_eq!(token.challenge_digest, crate::sha256(&challenge));
        assert_eq!(token.input(), bytes[..2 + 32 + 32 + 32]);
    }

    #[test]
    fn authorization_round_trips_and_tolerates_its_forms() {
        let token = Token {
            token_type: TOKEN_TYPE,
            nonce: [1; 32],
            challenge_digest: challenge_digest(),
            token_key_id: [3; 32],
            authenticator: [4; NK],
        };
        let header = token.authorization();
        assert!(header.starts_with("PrivateToken token=\""));
        assert_eq!(Token::from_authorization(&header), Some(token.clone()));
        let bare = crate::b64(&crate::encode(&token));
        for form in [format!("privatetoken token={bare}"), format!("PrivateToken realm=\"x\", token=\"{bare}\"")] {
            assert_eq!(Token::from_authorization(&form), Some(token.clone()), "{form}");
        }
        for bad in ["", "PrivateToken", "Bearer token=\"AAAA\"", "PrivateToken token=\"AAAA\"", "PrivateToken x=1"] {
            assert_eq!(Token::from_authorization(bad), None, "{bad}");
        }
        assert_eq!(token_input(&token.nonce, &token.token_key_id), token.input());
    }

    #[test]
    fn a_batch_encodes_with_variable_length_vectors() {
        let request =
            BatchTokenRequest { token_type: TOKEN_TYPE, truncated_token_key_id: 7, blinded_elements: vec![[9; NE]; 2] };
        let bytes = crate::encode(&request);
        // token_type, the key id byte, the length 98 as a two-byte QUIC varint (0x4062), the elements.
        assert_eq!(&bytes[..5], &[0, 1, 7, 0x40, 98]);
        assert_eq!(bytes.len(), 5 + 2 * NE);
        assert_eq!(crate::decode::<BatchTokenRequest>(&bytes).unwrap(), request);
    }
}
