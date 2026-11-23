//! An account and what its devices ask for themselves (spec 6.3, 6.4): registration, username, prekeys, lookup,
//! sender certificates, push token, access key, storage (6.9), backup (7), deletion.

use crate::db::{self, Key, NewDevice};
use crate::error::{refuse, Refusal, Res};
use crate::request::{bad, named_app, path_id, signature_of, Auth, Hubs, Peer, DAY, HOUR, MINUTE};
use crate::{now_ms, Hub};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use bytes::Bytes;
use yo_wire::cert::DeviceCertificate;
use yo_wire::handle::{Handle, KIND_USERNAME};
use yo_wire::limits::{self, MAX_DEVICES, MAX_PREKEYS_PER_UPLOAD, PREKEY_TARGET};
use yo_wire::{api, decode, encode, sha256, UserId};

impl Hub {
    /// A username at least `YO_NAME_MIN` long (spec 3.3); shorter is `400`. Only new names are measured: a name
    /// someone has from before a higher least stays theirs.
    fn long_enough(&self, name: &[u8]) -> Res<()> {
        let name = std::str::from_utf8(name).map_err(|_| bad())?;
        match yo_wire::handle::username_len(name) >= self.config.name_min {
            true => Ok(()),
            false => Err(bad()),
        }
    }

    /// A handle's value as the hub keeps it: a username by its skeleton (spec 3.3), so LE0 finds LEO.
    fn stored_handle(&self, handle: &Handle) -> Vec<u8> {
        match handle.kind {
            KIND_USERNAME => std::str::from_utf8(handle.value.as_slice())
                .map(|name| yo_wire::handle::skeleton(name).into_bytes())
                .unwrap_or_default(),
            _ => handle.value.as_slice().to_vec(),
        }
    }

    /// An upload of prekeys (spec 4.1): their number and sizes; what they are, the devices that fetch them check.
    fn check_prekeys(prekeys: &api::Prekeys, first: bool) -> Res<()> {
        use limits::{MAX_PREKEY_LEN, MAX_PREKEY_SIGNATURE_LEN};
        if prekeys.one_time.len() > MAX_PREKEYS_PER_UPLOAD || prekeys.kyber.len() > MAX_PREKEYS_PER_UPLOAD {
            return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
        }
        if first && (prekeys.signed.is_none() || prekeys.last_resort.is_none()) {
            return Err(bad());
        }
        let key = |k: &yo_wire::VLBytes| (1..=MAX_PREKEY_LEN).contains(&k.as_slice().len());
        let signed = |p: &api::SignedPrekey| {
            key(&p.key) && (1..=MAX_PREKEY_SIGNATURE_LEN).contains(&p.signature.as_slice().len())
        };
        let signed_ok = prekeys.signed.iter().chain(&prekeys.last_resort).chain(&prekeys.kyber).all(signed);
        match signed_ok && prekeys.one_time.iter().all(|p| key(&p.key)) {
            true => Ok(()),
            false => Err(bad()),
        }
    }

    /// A registration id in libsignal's range (14 bits, not 0) and a canonical Curve25519 auth key (spec 4.1, 6.2).
    pub(crate) fn device_keys_usable(registration_id: u32, auth_key: &Key) -> bool {
        (1..=0x3fff).contains(&registration_id) && yo_wire::curve_key_usable(auth_key)
    }
}

/// `POST /v1/register` (spec 6.4): first come, first served, limited per address; the current account key
/// registers its user again.
pub(crate) async fn post_register(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    body: Bytes,
) -> Res<Vec<u8>> {
    hub.limit_ip("register", peer, &headers, hub.config.limits.register_per_ip_hour, HOUR)?;
    let request: api::Register = decode(&body)?;
    let handle = Handle { kind: KIND_USERNAME, value: request.username.clone() };
    handle.check()?;
    let device = &request.device;
    if device.push.token.as_slice().len() > limits::MAX_PUSH_TOKEN_LEN
        || !Hub::device_keys_usable(device.registration_id, &device.auth_key)
    {
        return Err(bad());
    }
    // The account key's proof of possession over this handle and device.
    let account_key = DeviceCertificate::verify(device.certificate.as_slice(), &device.device_key)?.account_key;
    let signed = api::registration_to_be_signed(&handle, &device.device_id, &device.device_key, &device.auth_key);
    if !yo_wire::xeddsa_verify(&account_key, &signed, &device.proof) {
        return Err(bad());
    }
    // A known account key restores its user and keeps its name; only a new user's name is measured.
    if hub.db.user_of_account_key(&account_key)?.is_none() {
        hub.long_enough(request.username.as_slice())?;
    }
    Hub::check_prekeys(&device.prekeys, true)?;

    let new = NewDevice {
        device_id: device.device_id,
        device_key: device.device_key,
        auth_key: device.auth_key,
        registration_id: device.registration_id,
        push_platform: device.push.platform,
        push_token: device.push.token.as_slice().to_vec(),
        app: named_app(&headers).map(str::to_owned),
    };
    let mut fresh: UserId = [0; 16];
    getrandom::fill(&mut fresh).map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
    let spelling = std::str::from_utf8(handle.value.as_slice()).map_err(|_| bad())?;
    let skeleton = hub.stored_handle(&handle);
    let most = PREKEY_TARGET * 2;
    let registered =
        hub.db.register(&skeleton, spelling, &fresh, &account_key, &new, &device.prekeys, most, now_ms())?;
    let (user_id, username, device_id) = registered.done()?;
    Ok(encode(&api::Registered { user_id, username: username.map(|name| name.into_bytes().into()), device_id }))
}

/// `GET /v1/username/{name}` (spec 6.3): `204` free, `409` taken or reserved, `400` no username; by skeleton.
pub(crate) async fn get_username(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Res<StatusCode> {
    hub.limit_ip("name", peer, &headers, hub.config.limits.name_check_per_ip_minute, MINUTE)?;
    yo_wire::handle::check_username_form(&name)?;
    // Taken before too short: a name someone has from before a higher least is found, not refused.
    if hub.db.username_taken(yo_wire::handle::skeleton(&name).as_bytes())? {
        return Err(refuse(StatusCode::CONFLICT));
    }
    hub.long_enough(name.as_bytes())?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /v1/username`: claims or gives up the account's username.
pub(crate) async fn put_username(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let request: api::SetUsername = decode(&body)?;
    let spelling = match &request.username {
        Some(name) => {
            Handle { kind: KIND_USERNAME, value: name.clone() }.check()?;
            let spelling = std::str::from_utf8(name.as_slice()).map_err(|_| bad())?;
            // The account's own name in another spelling is no new name.
            let owner = hub.db.username_owner(yo_wire::handle::skeleton(spelling).as_bytes())?;
            if owner != Some(device.user_id) {
                hub.long_enough(name.as_slice())?;
            }
            Some(spelling)
        }
        None => None,
    };
    let skeleton = spelling.map(yo_wire::handle::skeleton);
    let name = skeleton.as_deref().zip(spelling).map(|(skeleton, spelling)| (skeleton.as_bytes(), spelling));
    hub.db.set_username(&device.device_id, name)?.done()?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /v1/prekeys` (spec 4.1).
pub(crate) async fn put_prekeys(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let request: api::Prekeys = decode(&body)?;
    Hub::check_prekeys(&request, false)?;
    hub.db.add_prekeys(&device.device_id, &request, PREKEY_TARGET * 2)?.done()?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/lookup` (spec 6.3): who a handle is, signed when asked (6.10).
pub(crate) async fn post_lookup(
    State(hub): Hubs,
    uri: Uri,
    headers: HeaderMap,
    Auth(device, body): Auth,
) -> Res<Vec<u8>> {
    let request: api::Lookup = decode(&body)?;
    request.handle.check()?;
    if request.except.len() > MAX_DEVICES {
        return Err(bad());
    }
    let take = request.flags & api::LOOKUP_PREKEYS != 0;
    let limits = &hub.config.limits;
    match take {
        true => hub.limit_signed(&device, "lookup", &device.device_id, limits.lookup_per_device_day, DAY)?,
        false => {
            hub.limit_signed(&device, "identity", &device.device_id, limits.identity_lookup_per_device_day, DAY)?
        }
    }
    let found = hub
        .db
        .lookup(request.handle.kind, &hub.stored_handle(&request.handle), take, &request.except)?
        .ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    let result = api::LookupResult {
        user_id: found.user_id,
        account_key: found.account_key,
        devices: found.devices.into_iter().take(MAX_DEVICES).collect(),
        bundles: found.bundles.into_iter().take(MAX_DEVICES).collect(),
    };
    let answer = if request.flags & api::LOOKUP_USERNAME == 0 {
        encode(&result)
    } else {
        encode(&api::LookupNamed { found: result, username: found.username.map(|name| name.into_bytes().into()) })
    };
    Ok(hub.signed_answer(&headers, uri.path(), &body, answer))
}

/// `GET /v1/certificate` (spec 6.1): a sender certificate for the signing device, good for a day.
pub(crate) async fn get_certificate(State(hub): Hubs, Auth(device, _): Auth) -> Res<Vec<u8>> {
    let number = hub.db.device_number(&device.device_id)?.ok_or_else(|| refuse(StatusCode::UNAUTHORIZED))?;
    let account_key = hub.db.account_key(&device.user_id)?.ok_or_else(|| refuse(StatusCode::UNAUTHORIZED))?;
    hub.signer.issue(&device.user_id, number, &account_key, hub.clock_ms()).map_err(|error| {
        eprintln!("yo-hub: sender certificate: {error}");
        refuse(StatusCode::INTERNAL_SERVER_ERROR)
    })
}

/// `PUT /v1/push` (spec 6.7).
pub(crate) async fn put_push(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let request: api::Push = decode(&body)?;
    if request.token.as_slice().len() > limits::MAX_PUSH_TOKEN_LEN {
        return Err(bad());
    }
    hub.db.set_push(&device.device_id, request.platform, request.token.as_slice())?.done()?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /v1/access-key` (spec 4.10): the profile key check, and its commitment for groups (4.9).
pub(crate) async fn put_access_key(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let request: api::AccessKey = decode(&body)?;
    let commitment = request.commitment.as_slice();
    let committed = zkgroup::deserialize::<zkgroup::profiles::ProfileKeyCommitment>(commitment).is_ok();
    if request.requests_open > 1 || !committed {
        return Err(bad());
    }
    // `requests_open` is kept as told; the device drops strangers' requests itself.
    hub.db.set_access_key(&device.device_id, &request.check, request.requests_open == 1, commitment)?.done()?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /v1/account` (spec 6.4): everything the hub holds about the account; its other devices answer `410`.
pub(crate) async fn delete_account(State(hub): Hubs, Auth(device, _): Auth) -> Res<StatusCode> {
    let blobs = hub.db.blobs_of(&device.device_id)?;
    hub.db.delete_account(&device.device_id, now_ms())?.done()?;
    hub.count(crate::Event::AccountGone, 1);
    // Its photos and videos with it; one uploaded meanwhile goes with the sweep.
    for blob in &blobs {
        hub.blobs.remove(blob);
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- storage (spec 6.9) ----

/// `GET /v1/storage`: `404` while there is none.
pub(crate) async fn get_storage(State(hub): Hubs, Auth(device, _): Auth) -> Res<Vec<u8>> {
    hub.limit_signed(&device, "storage", &device.user_id, hub.config.limits.storage_per_user_hour, HOUR)?;
    let (version, data) = hub.db.storage(&device.user_id)?.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    Ok(encode(&api::Storage { version, data: data.into() }))
}

/// `PUT /v1/storage`: replaces it if the request names the version stored now; else `409` with that one.
pub(crate) async fn put_storage(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let request: api::Storage = decode(&body)?;
    if request.data.as_slice().len() > limits::MAX_STORAGE_LEN {
        return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
    }
    if request.data.as_slice().is_empty() {
        return Err(bad());
    }
    hub.limit_signed(&device, "storage", &device.user_id, hub.config.limits.storage_per_user_hour, HOUR)?;
    let stored = hub.db.put_storage(&device.device_id, request.version, request.data.as_slice(), now_ms())?.done()?;
    if let Err((version, data)) = stored {
        return Err(Refusal(StatusCode::CONFLICT, encode(&api::Storage { version, data: data.into() })));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- backup (spec 7) ----

/// `PUT /v1/backup`: the account's backup record, replacing the one before.
pub(crate) async fn put_backup(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let backup: api::Backup = decode(&body)?;
    backup.check()?;
    // Keys a signature can be checked against (Curve25519, canonical): any other is refused here, not at every fetch.
    let (passkey, code) = backup.auth_keys();
    if passkey.into_iter().chain(code).any(|key| !yo_wire::curve_key_usable(&key)) {
        return Err(bad());
    }
    let locator = backup.locator();
    let auth = db::BackupAuth { passkey, code };
    hub.db.put_backup(&device.device_id, locator.as_ref().map(|l| &l[..]), &body, &auth, now_ms())?.done()?;
    Ok(StatusCode::NO_CONTENT)
}

/// Checked where a record has no key: every fetch verifies twice, so its time tells nothing of a record or passkey.
static NO_KEY: std::sync::LazyLock<Key> = std::sync::LazyLock::new(|| {
    let mut key = [0; 32];
    key[0] = 9; // Curve25519's base point
    key
});

/// `GET /v1/backup/{id}`: the record, to a request signed by one of its doors' keys; anything else `404`, as for none.
pub(crate) async fn get_backup(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<Vec<u8>> {
    // Only failures spend the address's limit.
    let not_found = || match hub.limit_ip("backup", peer, &headers, hub.config.limits.backup_per_ip_hour, HOUR) {
        Ok(()) => refuse(StatusCode::NOT_FOUND),
        Err(limited) => limited,
    };
    let id: [u8; api::LOCATOR_LEN] = path_id(&id).map_err(|_| not_found())?;
    let now = now_ms();
    let found = hub.db.backup(&id)?;
    let (time, signature) = signature_of(&headers, now).ok_or_else(not_found)?;
    let auth = found.as_ref().map(|(_, auth)| auth.clone()).unwrap_or_default();
    let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let empty = sha256(&[]);
    let signed_by = |key: Option<Key>| {
        let verified =
            yo_wire::request::verify_hash(&key.unwrap_or(*NO_KEY), method.as_str(), path, time, &empty, &signature);
        verified.is_ok() && key.is_some()
    };
    // Both doors are always checked.
    let by_passkey = signed_by(auth.passkey);
    let by_code = signed_by(auth.code);
    match found {
        Some((record, _)) if (by_passkey || by_code) && hub.first_time(&signature, time, now) => Ok(record),
        _ => Err(not_found()),
    }
}
