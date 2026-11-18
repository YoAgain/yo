CREATE TABLE state_meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    word TEXT,
    key_check BLOB
);
INSERT INTO state_meta (id) VALUES (1);

CREATE TABLE accounts (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    user_id TEXT,
    username TEXT,
    name TEXT,
    account_secret BLOB,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE devices (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    auth_secret BLOB,
    device_id BLOB,
    device_key BLOB,
    device_number INTEGER,
    certificate BLOB,
    registration_id INTEGER,
    signed_until TEXT,
    sender_certificate BLOB,
    certificate_clock TEXT,
    registering BLOB,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE settings (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    next_seq TEXT,
    last_sent_ms TEXT,
    requests_open INTEGER,
    recovery_lookups BLOB,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE profile_keys (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    profile_key BLOB,
    profile_key_gen INTEGER,
    access_key_told INTEGER,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE account_storage (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    storage_version TEXT,
    storage_dirty INTEGER,
    storage_saved_at TEXT,
    storage_fetched INTEGER,
    legacy_storage_key BLOB,
    version TEXT,
    icon BLOB,
    left_handed BLOB,
    pk BLOB,
    pg BLOB,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE storage_friends (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE storage_groups (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE storage_keys (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE contacts (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    probe_service INTEGER,
    probe_reply_tag BLOB,
    probe_pending BLOB,
    account_key BLOB,
    name TEXT,
    verified INTEGER,
    unconfirmed INTEGER,
    blocked INTEGER,
    gone INTEGER,
    chat INTEGER,
    profile_due INTEGER,
    receipt_due INTEGER,
    outdated_at TEXT,
    accepted INTEGER,
    profile_key BLOB,
    key_due INTEGER,
    key_given INTEGER,
    key_resent_at TEXT,
    key_gen INTEGER,
    PRIMARY KEY (owner, entry),
    UNIQUE (entry)
) WITHOUT ROWID;

CREATE TABLE contact_devices (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    key BLOB,
    slot INTEGER,
    PRIMARY KEY (owner, entry),
    FOREIGN KEY (owner) REFERENCES contacts(entry) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE contact_handles (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry),
    FOREIGN KEY (owner) REFERENCES contacts(entry) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE sent_cache (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry),
    FOREIGN KEY (owner) REFERENCES contacts(entry) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE decryption_errors (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry),
    FOREIGN KEY (owner) REFERENCES contacts(entry) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE groups (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    key TEXT,
    name TEXT,
    revision INTEGER,
    left INTEGER,
    accepted INTEGER,
    keyed INTEGER,
    named INTEGER,
    endorsed_until TEXT,
    PRIMARY KEY (owner, entry),
    UNIQUE (entry)
) WITHOUT ROWID;

CREATE TABLE group_members (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry),
    FOREIGN KEY (owner) REFERENCES groups(entry) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE group_endorsements (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry),
    FOREIGN KEY (owner) REFERENCES groups(entry) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE outbox (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    queued_at TEXT,
    seq TEXT,
    user TEXT,
    plain BLOB,
    device BLOB,
    non_urgent INTEGER,
    payload BLOB,
    made_for BLOB,
    refused INTEGER,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE inbox_events (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE inbox_dedup (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE deferred_payloads (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE roster_lookups (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE prekey_uploads (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    upload BLOB,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE retired_prekeys (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE prekey_state (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    next_id INTEGER,
    signed_at TEXT,
    left INTEGER,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE sessions (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE identities (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE prekeys (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE signed_prekeys (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE kyber_prekeys (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

CREATE TABLE kyber_replays (
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, entry)
) WITHOUT ROWID;

-- Reserved for libsignal SenderKeyStore. Yo does not currently create sender keys.
CREATE TABLE sender_keys (
    owner TEXT NOT NULL,
    device INTEGER NOT NULL,
    distribution_id TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (owner, device, distribution_id)
) WITHOUT ROWID;
