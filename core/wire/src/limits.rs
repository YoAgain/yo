//! The numbers both sides rely on (spec 6.5, 6.6). Rate limits live in the hub's configuration.

/// The largest KeyPackage accepted.
pub const MAX_KEY_PACKAGE_LEN: usize = 4096;
/// The most KeyPackages in one upload.
pub const MAX_KEY_PACKAGES_PER_UPLOAD: usize = 100;
/// How many KeyPackages a device keeps at the hub.
pub const KEY_PACKAGE_TARGET: u32 = 50;
/// The core refills below this many.
pub const KEY_PACKAGE_REFILL_BELOW: u32 = 20;
/// The most envelopes one fetch returns.
pub const MAX_INBOX_FETCH: usize = 100;
/// The most ids one ack names.
pub const MAX_ACK: usize = 1000;
/// The longest a fetch waits for mail, in seconds.
pub const MAX_WAIT_S: u64 = 60;
/// The largest request body: the largest envelope bucket and some room.
pub const MAX_BODY: usize = 64 * 1024 + 1024;
/// A signed request's time may differ from the hub's clock by this much.
pub const MAX_SKEW_MS: u64 = 5 * 60 * 1000;
/// The longest a push token may be.
pub const MAX_PUSH_TOKEN_LEN: usize = 512;
/// The longest an SMS code may be.
pub const MAX_CODE_LEN: usize = 12;
/// The most devices a lookup answers with.
pub const MAX_DEVICES: usize = 16;
