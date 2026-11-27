CREATE TABLE message (
    id INTEGER NOT NULL DEFAULT 0 PRIMARY KEY,
    device_id BLOB NOT NULL DEFAULT x'',
    envelope BLOB NOT NULL DEFAULT x'',
    received_at INTEGER NOT NULL DEFAULT 0,
    hub TEXT NOT NULL DEFAULT ''
);
CREATE INDEX message_device ON message(device_id, id);
CREATE INDEX message_received ON message(received_at);
CREATE TABLE account (
    id BLOB NOT NULL DEFAULT x'' PRIMARY KEY,
    account_key BLOB NOT NULL DEFAULT x'',
    created_at INTEGER NOT NULL DEFAULT 0,
    bot INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE account_device (
    id BLOB NOT NULL DEFAULT x'' PRIMARY KEY,
    user_id BLOB NOT NULL DEFAULT x'',
    device_key BLOB NOT NULL DEFAULT x'',
    auth_key BLOB NOT NULL DEFAULT x'',
    push_platform INTEGER NOT NULL DEFAULT 0,
    push_token BLOB NOT NULL DEFAULT x'',
    created_at INTEGER NOT NULL DEFAULT 0,
    last_seen_at INTEGER NOT NULL DEFAULT 0,
    app TEXT,
    registration_id INTEGER NOT NULL DEFAULT 0,
    number INTEGER NOT NULL DEFAULT 1,
    prekey_max INTEGER NOT NULL DEFAULT 0,
    version TEXT,
    ohttp_at INTEGER
);
CREATE INDEX account_device_user ON account_device(user_id);
CREATE TABLE account_device_gone (
    id BLOB NOT NULL DEFAULT x'' PRIMARY KEY,
    gone_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE account_contact_key (
    user_id BLOB NOT NULL DEFAULT x'' PRIMARY KEY,
    current BLOB NOT NULL DEFAULT x'',
    requests_open INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE account_prekey_left (
    device_id BLOB NOT NULL DEFAULT x'' PRIMARY KEY,
    n INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE hub_stat_minute (
    hub TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL DEFAULT '',
    minute INTEGER NOT NULL DEFAULT 0,
    n INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(hub, kind, minute)
);
