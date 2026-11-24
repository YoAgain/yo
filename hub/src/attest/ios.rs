//! App Attest (spec 6.11). The root and the clock are inputs so tests can use a private test CA.

use ciborium::Value;
use rustls_pki_types::{CertificateDer, UnixTime};
use std::time::Duration;
use webpki::{anchor_from_trusted_cert, EndEntityCert, KeyUsage};
use x509_parser::prelude::*;
use yo_wire::sha256;

// Apple's App Attestation Root CA, SHA-256 of DER:
// 1CB9823BA28BA6AD2D33A006941DE2AE4F513EF1D4E831B9F7E0FA7B6242C932.
pub(super) const ROOT: &[u8] = include_bytes!("apple-root.der");

fn field<'a>(object: &'a Value, name: &str) -> Result<&'a Value, &'static str> {
    let mut matches = object.as_map().ok_or("format")?.iter().filter(|(k, _)| k.as_text() == Some(name));
    let (_, value) = matches.next().ok_or("format")?;
    matches.next().is_none().then_some(value).ok_or("format")
}

fn cert(bytes: &[u8]) -> Result<X509Certificate<'_>, &'static str> {
    let (rest, cert) = X509Certificate::from_der(bytes).map_err(|_| "chain")?;
    rest.is_empty().then_some(cert).ok_or("chain")
}

pub(super) fn verify(
    mut payload: &[u8],
    client_data_hash: &[u8],
    ids: &[String],
    root: &[u8],
    now: u64,
) -> Result<(), &'static str> {
    let obj: Value = ciborium::de::from_reader_with_recursion_limit(&mut payload, 12).map_err(|_| "format")?;
    if !payload.is_empty() || field(&obj, "fmt")?.as_text() != Some("apple-appattest") {
        return Err("format");
    }
    let stmt = field(&obj, "attStmt")?;
    field(stmt, "receipt")?.as_bytes().ok_or("format")?;
    let [leaf, intermediate] = field(stmt, "x5c")?.as_array().ok_or("format")?.as_slice() else { return Err("chain") };
    let leaf_der = CertificateDer::from(leaf.as_bytes().ok_or("chain")?.as_slice());
    let intermediate = CertificateDer::from(intermediate.as_bytes().ok_or("chain")?.as_slice());
    let root_der = CertificateDer::from(root);
    let time = ASN1Time::from_timestamp((now / 1000) as i64).map_err(|_| "chain")?;
    if !cert(root)?.validity().is_valid_at(time) {
        return Err("chain");
    }
    let leaf_cert = EndEntityCert::try_from(&leaf_der).map_err(|_| "chain")?;
    let anchors = [anchor_from_trusted_cert(&root_der).map_err(|_| "chain")?];
    let intermediates = [intermediate];
    let path = leaf_cert
        .verify_for_usage(
            webpki::ALL_VERIFICATION_ALGS,
            &anchors,
            &intermediates,
            UnixTime::since_unix_epoch(Duration::from_millis(now)),
            // Apple App Attest signing (1.2.840.113635.100.4.24).
            KeyUsage::required_if_present(&[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x63, 0x64, 0x04, 0x18]),
            None,
            None,
        )
        .map_err(|_| "chain")?;
    if path.intermediate_certificates().count() != 1 {
        return Err("chain");
    }

    let leaf = cert(&leaf_der)?;
    let data = field(&obj, "authData")?.as_bytes().ok_or("format")?;
    if data.len() < 87 || data[32] & 0x40 == 0 {
        return Err("format");
    }
    let extension = leaf
        .get_extension_unique(&x509_parser::asn1_rs::oid!(1.2.840 .113635 .100 .8 .2))
        .map_err(|_| "nonce")?
        .ok_or("nonce")?;
    let nonce = sha256(&[data.as_slice(), client_data_hash].concat());
    if extension.value.strip_prefix(&[0x30, 0x24, 0xa1, 0x22, 0x04, 0x20]) != Some(nonce.as_slice()) {
        return Err("nonce");
    }
    let spki = leaf.public_key();
    if spki.algorithm.algorithm != oid_registry::OID_KEY_TYPE_EC_PUBLIC_KEY
        || spki.algorithm.parameters().and_then(|p| p.as_oid().ok()) != Some(oid_registry::OID_EC_P256)
    {
        return Err("credential");
    }
    let point = spki.subject_public_key.data.as_ref();
    if point.len() != 65 || point[0] != 4 || data[53..55] != [0, 32] || data[55..87] != sha256(point) {
        return Err("credential");
    }
    if !ids.iter().any(|id| sha256(id.as_bytes()) == data[..32]) {
        return Err("rp_id");
    }
    if data[33..37] != [0; 4] {
        return Err("counter");
    }
    match &data[37..53] {
        b"appattest\0\0\0\0\0\0\0" => Ok(()),
        b"appattestdevelop" => Err("development"),
        _ => Err("aaguid"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, CustomExtension, DnType, IsCa, Issuer, KeyPair};

    fn ca(name: &str) -> (CertificateParams, KeyPair) {
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        (params, KeyPair::generate().unwrap())
    }

    #[test]
    fn attestation_chain_nonce_app_and_environment() {
        let (params, key) = ca("root");
        let root = params.self_signed(&key).unwrap();
        let root_issuer = Issuer::new(params, key);
        let (params, key) = ca("intermediate");
        let intermediate = params.signed_by(&key, &root_issuer).unwrap();
        let issuer = Issuer::new(params, key);
        let key = KeyPair::generate().unwrap();
        let ids = ["test.app".into()];
        let challenge = b"fresh challenge";
        let now = crate::now_ms();
        for (aaguid, expected) in [(b"appattest\0\0\0\0\0\0\0", Ok(())), (b"appattestdevelop", Err("development"))] {
            let mut auth = sha256(b"test.app").to_vec();
            auth.extend([0x40, 0, 0, 0, 0]);
            auth.extend(aaguid);
            auth.extend([0, 32]);
            auth.extend(sha256(key.public_key_raw()));
            let nonce = sha256(&[auth.as_slice(), &sha256(challenge)].concat());
            let mut ext = vec![0x30, 0x24, 0xa1, 0x22, 0x04, 0x20];
            ext.extend(nonce);
            let mut params = CertificateParams::default();
            params.custom_extensions.push(CustomExtension::from_oid_content(&[1, 2, 840, 113635, 100, 8, 2], ext));
            let leaf = params.signed_by(&key, &issuer).unwrap();
            let stmt = Value::Map(vec![
                (
                    "x5c".into(),
                    Value::Array(vec![Value::Bytes(leaf.der().to_vec()), Value::Bytes(intermediate.der().to_vec())]),
                ),
                ("receipt".into(), Value::Bytes(vec![])),
            ]);
            let obj = Value::Map(vec![
                ("fmt".into(), "apple-appattest".into()),
                ("attStmt".into(), stmt),
                ("authData".into(), Value::Bytes(auth)),
            ]);
            let mut payload = Vec::new();
            ciborium::into_writer(&obj, &mut payload).unwrap();
            assert_eq!(verify(&payload, &sha256(challenge), &ids, root.der(), now), expected);
            assert_eq!(verify(&payload, &sha256(b"other"), &ids, root.der(), now), Err("nonce"));
            assert_eq!(verify(&payload, &sha256(challenge), &[], root.der(), now), Err("rp_id"));
            assert_eq!(verify(&payload, &sha256(challenge), &ids, ROOT, now), Err("chain"));
        }
    }

    /// Apple's own sample (developer documentation, "Validating apps that connect to your server"), at a time its
    /// leaf was valid: the chain to the real root, its nonce and its app. Its clientDataHash is the text itself.
    #[test]
    fn apples_published_sample() {
        let payload = include_bytes!("apple-sample.cbor");
        let hash = b"example_server_challenge";
        let ids = ["1234567890.com.example.myapp".to_string()];
        assert_eq!(verify(payload, hash, &ids, ROOT, 1_776_772_800_000), Ok(()));
        assert_eq!(verify(payload, &sha256(b"other"), &ids, ROOT, 1_776_772_800_000), Err("nonce"));
    }
}
