//! An account and what its devices ask for themselves (spec 4.1, 4.2): registration, username, prekeys, lookup,
//! sender certificates, push token, access key, storage (4.3), backup (2.4), deletion.

use crate::db::{self, Key, NewDevice};
use crate::error::{refuse, Refusal, Res};
use crate::request::{bad, named_app, path_id, signature_of, Auth, Hubs, Peer, DAY};
use crate::{now_ms, Hub};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use bytes::Bytes;
use yoip::net::cert::DeviceCertificate;
use yoip::net::handle::{Handle, KIND_USERNAME};
use yoip::net::limits::{self, MAX_DEVICES, MAX_PREKEYS_PER_UPLOAD, PREKEY_TARGET};
use yoip::{decode, encode, net::api, sha256, UserId};

impl Hub {
    /// The fewest characters of a new name (spec 2.2): `YO_NAME_MIN` while `YO_NAME_ATTEST` is off; with it on,
    /// `NAME_MIN_ATTESTED` for a client with a valid attestation, `NAME_MIN_UNATTESTED` for everyone else (old
    /// apps, F-Droid, CLI, bots; old apps just see `400` as for any name too short).
    pub(crate) fn name_min(&self, attested: bool) -> usize {
        use crate::config::{NAME_MIN_ATTESTED, NAME_MIN_UNATTESTED};
        match (self.config.name_attest, attested) {
            (false, _) => self.config.name_min,
            (true, true) => NAME_MIN_ATTESTED,
            (true, false) => NAME_MIN_UNATTESTED,
        }
    }

    /// A username at least `least` long; shorter is `400`. Only new names are measured: a name someone has from
    /// before a higher least stays theirs.
    fn long_enough(name: &[u8], least: usize) -> Res<()> {
        let name = std::str::from_utf8(name).map_err(|_| bad())?;
        match yoip::net::handle::username_len(name) >= least {
            true => Ok(()),
            false => Err(bad()),
        }
    }

    /// A handle's value as the hub keeps it: a username by its skeleton (spec 2.2), so LE0 finds LEO.
    fn stored_handle(&self, handle: &Handle) -> Vec<u8> {
        match handle.kind {
            KIND_USERNAME => std::str::from_utf8(handle.value.as_slice())
                .map(|name| yoip::net::handle::skeleton(name).into_bytes())
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
        let key = |k: &yoip::VLBytes| (1..=MAX_PREKEY_LEN).contains(&k.as_slice().len());
        let signed = |p: &api::SignedPrekey| {
            key(&p.key) && (1..=MAX_PREKEY_SIGNATURE_LEN).contains(&p.signature.as_slice().len())
        };
        let signed_ok = prekeys.signed.iter().chain(&prekeys.last_resort).chain(&prekeys.kyber).all(signed);
        match signed_ok && prekeys.one_time.iter().all(|p| key(&p.key)) {
            true => Ok(()),
            false => Err(bad()),
        }
    }

    /// A registration id in libsignal's range (14 bits, not 0) and a canonical Curve25519 auth key (spec 3.1, 4.2).
    pub(crate) fn device_keys_usable(registration_id: u32, auth_key: &Key) -> bool {
        (1..=0x3fff).contains(&registration_id) && yoip::curve_key_usable(auth_key)
    }
}

/// `POST /v1/register` (spec 4.2): first come, first served, limited per address; the current account key
/// registers its user again.
pub(crate) async fn post_register(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    body: Bytes,
) -> Res<Vec<u8>> {
    hub.limit_expensive(peer, &headers)?;
    let (request, replace) = api::Register::decode_replacing(&body).map_err(|_| bad())?;
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
    if !yoip::xeddsa_verify(&account_key, &signed, &device.proof) {
        return Err(bad());
    }
    // A known account key restores its user and keeps its name; only a new user's name is measured.
    // The attestation, if any, over the unsigned challenge named in `Yo-Attest-Challenge`, consumed once.
    let now = now_ms();
    let checked = match &request.attest {
        Some(att) if hub.config.attest => {
            let challenge = crate::attest::named_challenge(&headers).and_then(|c| {
                hub.challenges.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take_open(&c, hub.clock_ms())
            });
            crate::attest::check(&hub, att, challenge, hub.clock_ms())?
        }
        _ => None,
    };
    let attested = checked.as_ref().is_some_and(|c| c.result.is_ok());
    if hub.db.user_of_account_key(&account_key)?.is_none() {
        Hub::long_enough(request.username.as_slice(), hub.name_min(attested))?;
    }
    Hub::check_prekeys(&device.prekeys, true)?;

    let new = NewDevice {
        device_id: device.device_id,
        device_key: device.device_key,
        auth_key: device.auth_key,
        registration_id: device.registration_id,
        push_platform: device.push.platform,
        push_token: device.push.token.as_slice().to_vec(),
        app: named_app(&headers).map(|(app, _)| app.to_owned()),
        version: named_app(&headers).and_then(|(_, version)| version).map(str::to_owned),
    };
    let mut fresh: UserId = [0; 16];
    getrandom::fill(&mut fresh).map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
    let spelling = std::str::from_utf8(handle.value.as_slice()).map_err(|_| bad())?;
    let skeleton = hub.stored_handle(&handle);
    let most = PREKEY_TARGET * 2;
    let registered =
        hub.db.register(&skeleton, spelling, &fresh, &account_key, &new, &device.prekeys, most, replace, now)?;
    // Past the account's devices (a hub that adds them): `409 device_limit` with the devices, for the owner to
    // choose the one that goes (`RegisterReplacing`). Only the account key's holder gets here (its proof above).
    if let db::Write::Refused(db::Conflict::DeviceLimit) = registered {
        let user = hub.db.user_of_account_key(&account_key)?.ok_or_else(|| refuse(StatusCode::CONFLICT))?;
        let devices = hub.db.devices_of_user(&user)?;
        let body = [api::DEVICE_LIMIT, &encode(&api::OwnDevices { devices })].concat();
        return Err(Refusal(StatusCode::CONFLICT, body));
    }
    let (user_id, username, device_id) = registered.done()?;
    // Recorded as the account's attestation, as `POST /attest` would.
    if let Some(checked) = checked {
        crate::attest::report(&hub, &checked, new.app.as_deref(), now)?;
        hub.db.record_attestation(&new.device_id, checked.platform, checked.result.err(), checked.level, now)?;
    }
    Ok(encode(&api::Registered { user_id, username: username.map(|name| name.into_bytes().into()), device_id }))
}

/// `GET /v1/username/{name}` (spec 2.2): `204` free, `409` taken or reserved, `400` no username or too short; by
/// skeleton. Every answer carries `Yo-Name-Min`, the minimum for this caller: with `YO_NAME_ATTEST` on, 4 while the
/// unsigned challenge it names in `Yo-Attest-Challenge` is open (the client attests over it and sends the result
/// with `Register`, which decides), else 12.
pub(crate) async fn get_username(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let open = crate::attest::named_challenge(&headers).is_some_and(|c| {
        hub.challenges.lock().unwrap_or_else(std::sync::PoisonError::into_inner).open(&c, hub.clock_ms())
    });
    let least = hub.name_min(open);
    let answer = (|| {
        hub.limit_anonymous(peer, &headers)?;
        yoip::net::handle::check_username_form(&name)?;
        // Taken before too short: a name someone has from before a higher least is found, not refused.
        if hub.db.username_taken(yoip::net::handle::skeleton(&name).as_bytes())? {
            return Err(refuse(StatusCode::CONFLICT));
        }
        Hub::long_enough(name.as_bytes(), least)?;
        Ok(StatusCode::NO_CONTENT)
    })();
    let mut response = answer.into_response();
    response.headers_mut().insert(api::HEADER_NAME_MIN, least.into());
    response
}

/// `PUT /v1/prekeys` (spec 4.2).
pub(crate) async fn put_prekeys(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let request: api::Prekeys = decode(&body)?;
    Hub::check_prekeys(&request, false)?;
    hub.db.add_prekeys(&device.device_id, &request, PREKEY_TARGET * 2)?.done()?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/lookup` (spec 3.1): who a handle is, signed when asked (4.1).
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
    // One without prekeys counts as any signed request.
    if take {
        hub.limit_signed(&device, "lookup", &device.device_id, hub.config.limits.lookup_per_device_day, DAY)?;
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
    let named = |result| api::LookupNamed {
        found: result,
        username: found.username.clone().map(|name| name.into_bytes().into()),
    };
    let asked = |flag: u8| request.flags & flag != 0;
    let answer = match (asked(api::LOOKUP_USERNAME), asked(api::LOOKUP_BOT)) {
        (false, _) => encode(&result),
        (true, false) => encode(&named(result)),
        (true, true) => encode(&api::LookupBot { named: named(result), bot: found.bot.into() }),
    };
    Ok(hub.signed_answer(&headers, uri.path(), &body, answer))
}

/// `GET /v1/certificate` (spec 4.1): a sender certificate for the signing device, good for a day.
pub(crate) async fn get_certificate(State(hub): Hubs, Auth(device, _): Auth) -> Res<Vec<u8>> {
    let number = hub.db.device_number(&device.device_id)?.ok_or_else(|| refuse(StatusCode::UNAUTHORIZED))?;
    let account_key = hub.db.account_key(&device.user_id)?.ok_or_else(|| refuse(StatusCode::UNAUTHORIZED))?;
    hub.signer.issue(&device.user_id, number, &account_key, hub.clock_ms()).map_err(|error| {
        eprintln!("yo-hub: sender certificate: {error}");
        refuse(StatusCode::INTERNAL_SERVER_ERROR)
    })
}

/// `PUT /v1/push` (spec 4.2).
pub(crate) async fn put_push(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let request: api::Push = decode(&body)?;
    if request.token.as_slice().len() > limits::MAX_PUSH_TOKEN_LEN {
        return Err(bad());
    }
    hub.db.set_push(&device.device_id, request.platform, request.token.as_slice())?.done()?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/devices` (multi-device): the account's devices, for the owner to choose one to remove.
pub(crate) async fn get_devices(State(hub): Hubs, Auth(device, _): Auth) -> Res<Vec<u8>> {
    let devices = hub.db.own_devices(&device.device_id)?.done()?;
    Ok(encode(&api::OwnDevices { devices }))
}

/// `DELETE /v1/devices/{n}` (multi-device): removes another device of the account; it gets `410` from then on and
/// forgets what it held. Its own number is `400`, an unknown one `404`.
pub(crate) async fn delete_device(
    State(hub): Hubs,
    axum::extract::Path(number): axum::extract::Path<u8>,
    Auth(device, _): Auth,
) -> Res<StatusCode> {
    match hub.db.remove_own_device(&device.device_id, number, now_ms())?.done()? {
        Some(true) => Ok(StatusCode::NO_CONTENT),
        Some(false) => Err(bad()),
        None => Err(refuse(StatusCode::NOT_FOUND)),
    }
}

/// `PUT /v1/bot` (spec 2.1, 4.1): marks the account a bot for everyone who looks it up; never cleared (`409`).
pub(crate) async fn put_bot(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let bot = match body[..] {
        [0] => false,
        [1] => true,
        _ => return Err(bad()),
    };
    if !hub.db.set_bot(&device.device_id, bot)?.done()? {
        return Err(refuse(StatusCode::CONFLICT));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /v1/access-key` (spec 3.3): the profile key check, and its commitment for groups (3.4).
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

/// `DELETE /v1/account` (spec 4.2): everything the hub holds about the account; its other devices answer `410`.
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

// ---- storage (spec 4.3) ----

/// `GET /v1/storage`: `404` while there is none.
pub(crate) async fn get_storage(State(hub): Hubs, Auth(device, _): Auth) -> Res<Vec<u8>> {
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
    let stored = hub.db.put_storage(&device.device_id, request.version, request.data.as_slice(), now_ms())?.done()?;
    if let Err((version, data)) = stored {
        return Err(Refusal(StatusCode::CONFLICT, encode(&api::Storage { version, data: data.into() })));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- backup (spec 2.4) ----

/// `PUT /v1/backup`: the account's backup record, replacing the one before.
pub(crate) async fn put_backup(State(hub): Hubs, Auth(device, body): Auth) -> Res<StatusCode> {
    let backup: api::Backup = decode(&body)?;
    backup.check()?;
    // Keys a signature can be checked against (Curve25519, canonical): any other is refused here, not at every fetch.
    let (passkey, code) = backup.auth_keys();
    if passkey.into_iter().chain(code).any(|key| !yoip::curve_key_usable(&key)) {
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
    let not_found = || match hub.limit_expensive(peer, &headers) {
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
            yoip::net::request::verify_hash(&key.unwrap_or(*NO_KEY), method.as_str(), path, time, &empty, &signature);
        verified.is_ok() && key.is_some()
    };
    // Both doors are always checked.
    let by_passkey = signed_by(auth.passkey);
    let by_code = signed_by(auth.code);
    match found {
        // Signed with the hub's key when asked with a nonce (spec 4.1): nobody between hands back an older record.
        Some((record, _))
            if (by_passkey || by_code)
                && hub.first_time(&id, &signature, time, now, crate::mail::peer_forwarded(&hub, &headers)) =>
        {
            Ok(hub.signed_answer(&headers, uri.path(), &[], record))
        }
        _ => Err(not_found()),
    }
}
