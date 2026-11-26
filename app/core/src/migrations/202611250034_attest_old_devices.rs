//! Devices registered before attestation came (8764123, 25.11.2026) are attested once, like a new one at its
//! registration. Remove once the admin shows no device of a build before attestation; then a device registered
//! before it is never asked again.

use crate::state::State;

/// A device from before attestation, not attested yet: ask it (once it succeeds, `attested` ends it for good).
pub fn due(state: &State) -> bool {
    !state.attestation.at_registration
}
