//! The MLS profile (spec 2, 4): one suite, ciphertext only, the same configuration on every device.

use crate::error::{Error, Result};
use openmls::prelude::*;
use openmls_memory_storage::MemoryStorage;
use openmls_rust_crypto::RustCrypto;
use std::collections::HashMap;
use std::sync::PoisonError;

pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519;
/// How far a sender ratchet may be read out of order, and how far ahead it may jump.
const OUT_OF_ORDER_TOLERANCE: u32 = 50;
const MAXIMUM_FORWARD_DISTANCE: u32 = 1000;
/// Epochs kept to read application messages sent before a commit arrived.
const MAX_PAST_EPOCHS: usize = 3;
/// KeyPackages live a year; the last resort is replaced every 30 days.
pub const KEY_PACKAGE_LIFETIME_S: u64 = 365 * 24 * 3600;

/// What OpenMLS works with: the Rust crypto backend and an in-memory store the client persists as a whole.
#[derive(Default)]
pub struct Provider {
    crypto: RustCrypto,
    storage: MemoryStorage,
}

impl OpenMlsProvider for Provider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &MemoryStorage {
        &self.storage
    }

    fn crypto(&self) -> &RustCrypto {
        &self.crypto
    }

    fn rand(&self) -> &RustCrypto {
        &self.crypto
    }
}

pub type Entries = HashMap<Vec<u8>, Vec<u8>>;

impl Provider {
    pub fn entries(&self) -> Entries {
        self.storage.values.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn restore(&self, entries: Entries) {
        *self.storage.values.write().unwrap_or_else(PoisonError::into_inner) = entries;
    }
}

pub fn capabilities() -> Capabilities {
    Capabilities::new(
        Some(&[ProtocolVersion::Mls10]),
        Some(&[CIPHERSUITE]),
        Some(&[ExtensionType::LastResort]),
        None,
        Some(&[CredentialType::Basic]),
    )
}

fn sender_ratchet() -> SenderRatchetConfiguration {
    SenderRatchetConfiguration::new(OUT_OF_ORDER_TOLERANCE, MAXIMUM_FORWARD_DISTANCE)
}

pub fn create_config() -> Result<MlsGroupCreateConfig> {
    let required = Extension::RequiredCapabilities(RequiredCapabilitiesExtension::new(
        &[],
        &[],
        &[CredentialType::Basic],
    ));
    Ok(MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .capabilities(capabilities())
        .with_group_context_extensions(Extensions::single(required).map_err(|_| Error::Internal("extensions"))?)
        .use_ratchet_tree_extension(true)
        .sender_ratchet_configuration(sender_ratchet())
        .max_past_epochs(MAX_PAST_EPOCHS)
        .lifetime(Lifetime::new(KEY_PACKAGE_LIFETIME_S))
        .build())
}

pub fn join_config() -> MlsGroupJoinConfig {
    MlsGroupJoinConfig::builder()
        .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .use_ratchet_tree_extension(true)
        .sender_ratchet_configuration(sender_ratchet())
        .max_past_epochs(MAX_PAST_EPOCHS)
        .build()
}
