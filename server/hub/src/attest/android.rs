//! Locally encrypted Play Integrity responses (spec 4.2): a classic token with nonce base64url(SHA-256(challenge)),
//! decrypted and verified with the app's own keys; it must show `PLAY_RECOGNIZED` and `MEETS_DEVICE_INTEGRITY`, the
//! app's package and a timestamp within 10 minutes.

use crate::config::PlayKeys;
use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use aws_lc_rs::key_wrap::{AesKek, KeyWrap, AES_256};
use aws_lc_rs::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
use serde_json::Value;
use yoip::{b64, sha256, unb64};

fn bytes(text: &str) -> Result<Vec<u8>, &'static str> {
    unb64(text).map_err(|_| "format")
}

fn header(text: &str, alg: &str) -> Result<Value, &'static str> {
    let value: Value = serde_json::from_slice(&bytes(text)?).map_err(|_| "format")?;
    if value["alg"] != alg || value.get("crit").is_some() {
        return Err("format");
    }
    Ok(value)
}

fn claims(payload: &[u8], keys: &PlayKeys) -> Result<Value, &'static str> {
    let token = std::str::from_utf8(payload).map_err(|_| "format")?;
    let mut parts = token.split('.');
    let [Some(protected), Some(wrapped), Some(iv), Some(cipher), Some(tag), None] =
        std::array::from_fn(|_| parts.next())
    else {
        return Err("format");
    };
    let jose = header(protected, "A256KW")?;
    if jose["enc"] != "A256GCM" || jose.get("zip").is_some() {
        return Err("format");
    }
    let wrapped = bytes(wrapped)?;
    if wrapped.len() != 40 {
        return Err("decrypt");
    }
    let mut cek = [0; 32];
    let kek = AesKek::new(&AES_256, &keys.decrypt).map_err(|_| "decrypt")?;
    let key = kek.unwrap(&wrapped, &mut cek).map_err(|_| "decrypt")?;
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| "decrypt")?);
    let nonce = Nonce::try_assume_unique_for_key(&bytes(iv)?).map_err(|_| "decrypt")?;
    let mut encrypted = bytes(cipher)?;
    let tag = bytes(tag)?;
    if tag.len() != 16 {
        return Err("decrypt");
    }
    encrypted.extend(tag);
    let clear = key.open_in_place(nonce, Aad::from(protected.as_bytes()), &mut encrypted).map_err(|_| "decrypt")?;
    let jws = std::str::from_utf8(clear).map_err(|_| "format")?;
    let (signed, signature) = jws.rsplit_once('.').ok_or("format")?;
    let (protected, body) = signed.split_once('.').ok_or("format")?;
    header(protected, "ES256")?;
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &keys.verify)
        .verify(signed.as_bytes(), &bytes(signature)?)
        .map_err(|_| "signature")?;
    serde_json::from_slice(&bytes(body)?).map_err(|_| "format")
}

pub(super) fn verify(
    payload: &[u8],
    keys: &PlayKeys,
    challenge: &[u8],
    package: &str,
    now: u64,
    level: &mut Option<&'static str>,
) -> Result<(), &'static str> {
    let claims = claims(payload, keys)?;
    let request = &claims["requestDetails"];
    let app = &claims["appIntegrity"];
    let labels = claims["deviceIntegrity"]["deviceRecognitionVerdict"].as_array();
    let has = |label: &str| labels.is_some_and(|labels| labels.iter().any(|v| v == label));
    // A classic request (only those can be decrypted with the app's own keys): its nonce the challenge's hash.
    if request["nonce"].as_str().map(|nonce| nonce.trim_end_matches('=')) != Some(b64(&sha256(challenge)).as_str()) {
        return Err("nonce");
    }
    let packages = [request.get("requestPackageName"), app.get("packageName")];
    if packages.iter().all(Option::is_none) || packages.into_iter().flatten().any(|v| v != package) {
        return Err("package");
    }
    let stamp = &request["timestampMillis"];
    let time = stamp.as_u64().or_else(|| stamp.as_str()?.parse().ok()).ok_or("stale")?;
    if time.abs_diff(now) > 600_000 {
        return Err("stale");
    }
    *level = [
        ("MEETS_STRONG_INTEGRITY", "strong"),
        ("MEETS_DEVICE_INTEGRITY", "device"),
        ("MEETS_BASIC_INTEGRITY", "basic"),
    ]
    .into_iter()
    .find(|(label, _)| has(label))
    .map(|(_, level)| level);
    if app["appRecognitionVerdict"] != "PLAY_RECOGNIZED" {
        return Err("unrecognized_app");
    }
    if !has("MEETS_DEVICE_INTEGRITY") {
        return Err("device_integrity");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::encoding::{AsDer, PublicKeyX509Der};
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
    use serde_json::json;

    fn token(claims: &Value, signer: &EcdsaKeyPair, kek: &[u8]) -> Vec<u8> {
        let header = b64(br#"{"alg":"ES256"}"#);
        let body = format!("{header}.{}", b64(&serde_json::to_vec(claims).unwrap()));
        let sig = signer.sign(&SystemRandom::new(), body.as_bytes()).unwrap();
        let mut clear = format!("{body}.{}", b64(sig.as_ref())).into_bytes();
        let header = b64(br#"{"alg":"A256KW","enc":"A256GCM"}"#);
        let cek = rand::random::<[u8; 32]>();
        let iv = rand::random::<[u8; 12]>();
        let mut wrapped = [0; 40];
        AesKek::new(&AES_256, kek).unwrap().wrap(&cek, &mut wrapped).unwrap();
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &cek).unwrap());
        let tag = key
            .seal_in_place_separate_tag(Nonce::assume_unique_for_key(iv), Aad::from(header.as_bytes()), &mut clear)
            .unwrap();
        format!("{header}.{}.{}.{}.{}", b64(&wrapped), b64(&iv), b64(&clear), b64(tag.as_ref())).into_bytes()
    }

    #[test]
    fn encrypted_and_signed_verdicts() {
        let signer = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap();
        let der: PublicKeyX509Der = signer.public_key().as_der().unwrap();
        let keys = PlayKeys { decrypt: rand::random(), verify: der.as_ref().to_vec() };
        let challenge = b"abc";
        let package = "test.app";
        let base = json!({"requestDetails":{"nonce":b64(&sha256(challenge)),"requestPackageName":package,"timestampMillis":"1000"},
            "appIntegrity":{"appRecognitionVerdict":"PLAY_RECOGNIZED"},"deviceIntegrity":{"deviceRecognitionVerdict":["MEETS_DEVICE_INTEGRITY","MEETS_STRONG_INTEGRITY"]}});
        for (path, value, expected, want_level) in [
            ("/requestDetails/timestampMillis", json!(1000), Ok(()), Some("strong")),
            ("/requestDetails/nonce", json!("wrong"), Err("nonce"), None),
            (
                "/appIntegrity/appRecognitionVerdict",
                json!("UNRECOGNIZED_VERSION"),
                Err("unrecognized_app"),
                Some("strong"),
            ),
            (
                "/deviceIntegrity/deviceRecognitionVerdict",
                json!(["MEETS_BASIC_INTEGRITY"]),
                Err("device_integrity"),
                Some("basic"),
            ),
            ("/requestDetails/timestampMillis", json!(700000), Err("stale"), None),
        ] {
            let mut claims = base.clone();
            *claims.pointer_mut(path).unwrap() = value;
            let token = token(&claims, &signer, &keys.decrypt);
            let mut level = None;
            assert_eq!(verify(&token, &keys, challenge, package, 1000, &mut level), expected);
            assert_eq!(level, want_level);
        }
        let other = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap();
        assert_eq!(claims(&token(&base, &other, &keys.decrypt), &keys), Err("signature"));
        assert_eq!(claims(&token(&base, &signer, &rand::random::<[u8; 32]>()), &keys), Err("decrypt"));
    }
}
