use crate::Result;
use rustls::crypto::aws_lc_rs::default_provider;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::{path::Path, sync::Arc};

pub struct Certs {
    pub ca: CertificateDer<'static>,
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
}

impl Certs {
    pub fn load(dir: &Path) -> Result<Self> {
        Ok(Self {
            ca: CertificateDer::from_pem_file(dir.join("ca.crt"))?,
            chain: CertificateDer::pem_file_iter(dir.join("node.crt"))?.collect::<std::result::Result<_, _>>()?,
            key: PrivateKeyDer::from_pem_file(dir.join("node.key"))?,
        })
    }

    fn roots(&self) -> Result<RootCertStore> {
        let mut roots = RootCertStore::empty();
        roots.add(self.ca.clone())?;
        Ok(roots)
    }
}

pub fn server_config(certs: &Certs) -> Result<Arc<ServerConfig>> {
    let provider = Arc::new(default_provider());
    let verifier = WebPkiClientVerifier::builder_with_provider(certs.roots()?.into(), provider.clone()).build()?;
    let mut tls = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs.chain.clone(), certs.key.clone_key())?;
    tls.alpn_protocols = vec![b"h2".to_vec()];
    Ok(Arc::new(tls))
}

pub fn client_config(certs: &Certs) -> Result<Arc<ClientConfig>> {
    let mut tls = ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(certs.roots()?)
        .with_client_auth_cert(certs.chain.clone(), certs.key.clone_key())?;
    tls.alpn_protocols = vec![b"h2".to_vec()];
    Ok(Arc::new(tls))
}
