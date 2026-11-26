//! The numbers both sides rely on (spec 4.2, 4.1). Rate limits live in the hub's configuration.

/// The largest prekey accepted: a Kyber-1024 public key is 1569 bytes.
pub const MAX_PREKEY_LEN: usize = 2048;
/// The largest prekey signature: XEdDSA's 64 bytes.
pub const MAX_PREKEY_SIGNATURE_LEN: usize = 64;
/// The most one-time prekeys of each kind in one upload.
pub const MAX_PREKEYS_PER_UPLOAD: usize = 32;
/// How many one-time prekeys of each kind a device keeps at the hub: a registration with as many Kyber keys still fits
/// one request.
pub const PREKEY_TARGET: u32 = 32;
/// The core refills below this many.
pub const PREKEY_REFILL_BELOW: u32 = 16;
/// The most envelopes one fetch returns.
pub const MAX_INBOX_FETCH: usize = 100;
/// The most ids one ack names.
pub const MAX_ACK: usize = 1000;
/// The longest a fetch waits for mail, in seconds.
pub const MAX_WAIT_S: u64 = 60;
/// The largest blob (spec 4.3): a photo or a short video, encrypted.
pub const MAX_BLOB_LEN: usize = 10 * 1024 * 1024;
/// The most fetches one blob is stored for: every device it goes to, 16 devices of a 20-member group (spec 4.3).
pub const MAX_BLOB_FETCHES: u32 = 320;
/// The most while every account has one device (the hub replaces, spec 4.2): a 20-member group and room.
pub const MAX_BLOB_FETCHES_ONE_DEVICE: u32 = 64;
/// The largest request body: the largest envelope bucket and some room.
pub const MAX_BODY: usize = 64 * 1024 + 1024;
/// The largest delivery (spec 3.2): one envelope per device of the user, together at most this.
pub const MAX_DELIVERY_BODY: usize = 256 * 1024 + 1024;
/// A signed request's time may differ from the hub's clock by this much.
pub const MAX_SKEW_MS: u64 = 5 * 60 * 1000;
/// The longest a push token may be.
pub const MAX_PUSH_TOKEN_LEN: usize = 512;
/// The most devices a lookup answers with.
pub const MAX_DEVICES: usize = 16;
/// The largest wrapped K in a backup record: its 32 bytes with ChaCha20-Poly1305's nonce and tag are 60.
pub const MAX_BACKUP_WRAP_LEN: usize = 256;
/// The largest sealed part of a backup record: the account and its friends, and the record still one request.
pub const MAX_BACKUP_SEALED_LEN: usize = 60 * 1024;
/// The sizes a sealed storage blob and a backup's sealed part can have (spec 4.3, 2.4): padded like an envelope, then
/// sealed, so the hub sees a size class. The largest is both limits.
pub const SEALED_SIZES: [usize; 4] = [1024, 4096, 16 * 1024, 60 * 1024];
/// The longest feedback text, in bytes of UTF-8 (spec 4).
pub const MAX_FEEDBACK_LEN: usize = 2000;
/// The account's sealed storage (spec 4.3).
pub const MAX_STORAGE_LEN: usize = 60 * 1024;
/// The longest app version sent with feedback.
pub const MAX_APP_VERSION_LEN: usize = 64;

/// Oblivious HTTP (spec 4.1): the sizes a whole wrapped body can have, both directions; the TLS proxy sees only the
/// bucket. One table, so it is easy to change. Measured before (#8, 26.11.): a one-device Yo is about 1.2 KB
/// directly (its envelope 1 KiB, headers 150), with `envelope::SMALL` it fits the smallest; lookups, certificates,
/// inbox fetches of a few Yos and group calls fit 4 KiB; registrations and prekey uploads (about 55 KB) 64 KiB;
/// the largest delivery (`MAX_DELIVERY_BODY`) 1 MiB; a full inbox fetch (100 envelopes of 64 KiB) 16 MiB.
pub const OHTTP_BUCKETS: [usize; 8] =
    [1024, 4096, 16 * 1024, 64 * 1024, 256 * 1024, 1024 * 1024, 4 * 1024 * 1024, 16 * 1024 * 1024];
/// The largest wrapped request the hub reads: the bucket that holds the largest delivery.
pub const MAX_OHTTP_REQUEST: usize = 1024 * 1024;
/// The largest wrapped answer: the largest bucket.
pub const MAX_OHTTP_RESPONSE: usize = OHTTP_BUCKETS[OHTTP_BUCKETS.len() - 1];
/// What wrapping adds to a request (RFC 9458, X25519, AES-128-GCM): key id and suite (7), `enc` (32), tag (16).
pub const OHTTP_REQUEST_OVERHEAD: usize = 7 + 32 + 16;
/// What wrapping adds to an answer: its nonce (16) and tag (16).
pub const OHTTP_RESPONSE_OVERHEAD: usize = 16 + 16;

/// The length to pad a Binary HTTP message of `len` bytes to, so that wrapped (`overhead`) it fills a bucket.
pub fn ohttp_padded(len: usize, overhead: usize) -> Option<usize> {
    OHTTP_BUCKETS.iter().find(|b| **b >= len + overhead).map(|b| b - overhead)
}

/// A blob ticket's lifetime (spec 4.1): 5 minutes.
pub const TICKET_MS: u64 = 5 * 60 * 1000;
/// A wrapped request's inner `Date` may differ from the hub's clock by this much (spec 4.1).
pub const OHTTP_SKEW_MS: u64 = 5 * 60 * 1000;
