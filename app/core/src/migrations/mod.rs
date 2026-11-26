//! One-time migrations, each in a file of its own named by when it came (Rails style, YYYYMMDDHHMM_name.rs) and saying
//! when it may go. Everything else in the core is current.
//!
//! - `keychain_v1`: an old install's keychain item and state files, signed in once (iOS: Migrations/KeychainV1.swift).
//!   Its old state formats are read by `state::legacy_identity` and `state::open_legacy`. Remove once every keychain
//!   item is v3.
//! - `attest_old_devices`: devices registered before attestation are attested once; new ones are at registration.

#[path = "202611250034_attest_old_devices.rs"]
pub mod attest_old_devices;
#[path = "202611191330_keychain_v1.rs"]
pub mod keychain_v1;
