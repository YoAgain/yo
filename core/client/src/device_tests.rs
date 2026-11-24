//! Exercise the real session stores and payloads without a network or device.
use super::*;
use yo_wire::content::{Read, SyncMessage};

struct NoNetwork;
impl Transport for NoNetwork {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> std::result::Result<Response, String> {
        Err("offline".into())
    }
}

fn phone(account: u8, number: u8) -> Client {
    let mut state = State::default();
    state.user_id = hex(&[account; 16]);
    state.account_secret = [account; 32];
    state.device_key = [number; 32];
    state.device_number = number;
    state.registration_id = 100;
    state.requests_open = true;
    let records = Records::new(&Default::default(), &state.account_secret, 100).unwrap();
    Client::with(Arc::new(NoNetwork), records, state)
}

fn rostered(c: &Client) -> api::Device {
    api::Device { key: c.state.device_key, id: c.state.device_number }
}

fn bundle(c: &mut Client) -> api::Bundle {
    let mut p = c.make_prekeys(1, true).unwrap();
    api::Bundle {
        device_key: c.state.device_key,
        registration_id: c.state.registration_id,
        signed: p.signed.unwrap(),
        kyber: p.kyber.pop().unwrap(),
        one_time: p.one_time.pop(),
    }
}

fn plaintext(to: Option<UserId>, content: Content) -> Vec<u8> {
    encode(&Plain { to, content: Message { sent_at_ms: 100, group: None, content }.encode().unwrap().into() })
}

fn receive(c: &mut Client, payload: &[u8]) -> Result<()> {
    c.receive(&[&[0; 16][..], payload].concat(), &mut BTreeSet::new(), &mut false)
}

#[test]
fn sixteen_session_copies_fit_and_every_device_can_answer() {
    use libsignal_protocol::{Aci, KeyPair, SenderCertificate, ServerCertificate};
    let mut sender = phone(33, 17);
    let mut peers: Vec<_> = (1..=16).map(|i| phone(44, i)).collect();
    let user = peers[0].user_id().to_owned();
    let key = peers[0].my_account_key();
    let roster: Vec<_> = peers.iter().map(rostered).collect();
    sender.take_devices(&user, key, &roster, Some("Peer".into())).unwrap();
    sender.take_bundles(&user, key, &peers.iter_mut().map(bundle).collect::<Vec<_>>()).unwrap();
    let yo = plaintext(None, Content::Yo);
    let (first, _) = sender.encrypt_for(&user, &yo, false, &[]).unwrap();
    let first_payload: Payload = decode(&first).unwrap();
    assert_eq!(first_payload.copies.len(), 16);
    assert!(first_payload.copies.iter().all(|c| c.kind == KIND_PREKEY));
    // Measured with the small direct Yo and registration id above: 16 PreKey copies total 29,389 bytes before sealing.
    for peer in &mut peers {
        peer.take_devices(sender.user_id(), sender.my_account_key(), &[rostered(&sender)], Some("Sender".into()))
            .unwrap();
        receive(peer, &first).unwrap();
        let (answer, _) = peer.encrypt_for(sender.user_id(), &yo, false, &[]).unwrap();
        receive(&mut sender, &answer).unwrap();
    }
    assert_eq!(sender.state.events.iter().filter(|e| matches!(e, Event::Message { .. })).count(), 16);
    let (ordinary, _) = sender.encrypt_for(&user, &yo, false, &[]).unwrap();
    let payload: Payload = decode(&ordinary).unwrap();
    assert_eq!(payload.copies.len(), 16);
    assert!(payload.copies.iter().all(|c| c.kind == KIND_MESSAGE));
    let own = identity::account_pair(&sender.state.account_secret).unwrap();
    let root = KeyPair::generate(&mut rand::rng());
    let server = KeyPair::generate(&mut rand::rng());
    let signer = ServerCertificate::new(1, server.public_key, &root.private_key, &mut rand::rng()).unwrap();
    let cert = SenderCertificate::new(
        Aci::from_uuid_bytes([33; 16]).service_id_string(),
        None,
        *own.public_key(),
        DeviceId::new(17).unwrap(),
        Timestamp::from_epoch_millis(10_000),
        signer,
        &server.private_key,
        &mut rand::rng(),
    )
    .unwrap();
    for bytes in [&first, &ordinary] {
        let envelope = sealed::seal(&cert, &own, &signal::identity_of(&key).unwrap(), bytes).unwrap();
        assert!(yo_wire::envelope::BUCKETS.contains(&envelope.len()));
    }

    let (misdirected, _) = sender.encrypt_for(&user, &plaintext(Some([5; 16]), Content::Yo), false, &[]).unwrap();
    assert_eq!(receive(&mut peers[0], &misdirected), Err(Error::Protocol("a copy for another")));
    let seen = Content::Sync(SyncMessage::Read(Read { user: Some([7; 16]), group: None, through_sent_at: 90 }));
    let (contact_seen, _) = sender.encrypt_for(&user, &plaintext(None, seen), false, &[]).unwrap();
    receive(&mut peers[0], &contact_seen).unwrap();
    assert!(peers[0].state.seen_through.is_empty());

    // Removing a member of the roster deletes only its session.
    sender.take_devices(&user, key, &roster[1..], None).unwrap();
    assert!(!sender.records.usable(&sender.address(&user, 1).unwrap()));
    assert!(!sender.no_session(&user));
}

#[test]
fn invalid_rosters_do_not_change_pins_or_devices() {
    let mut c = phone(1, 1);
    let user = hex(&[2; 16]);
    let good = api::Device { key: [2; 32], id: 1 };
    c.take_devices(&user, [3; 32], &[good], None).unwrap();
    for bad in [
        vec![good, api::Device { id: 2, ..good }],
        vec![good, api::Device { key: [4; 32], ..good }],
        vec![api::Device { id: 0, ..good }],
        vec![api::Device { id: 128, ..good }],
        (1..=17).map(|id| api::Device { key: [id; 32], id }).collect(),
    ] {
        assert!(matches!(c.take_devices(&user, [9; 32], &bad, None), Err(Error::Protocol(_))));
        assert_eq!(c.state.contacts[&user].account_key, [3; 32]);
        assert_eq!(c.roster_keys(&user), [[2; 32]]);
    }
}

#[test]
fn siblings_take_sent_and_read_without_an_event_or_response() {
    let mut a = phone(1, 1);
    let mut b = phone(1, 2);
    let user = a.user_id().to_owned();
    let roster = [rostered(&a), rostered(&b)];
    for phone in [&mut a, &mut b] {
        phone.take_devices(&user, phone.my_account_key(), &roster, None).unwrap();
    }
    a.take_bundles(&user, b.my_account_key(), &[bundle(&mut b)]).unwrap();
    let direct = [3; 16];
    let group = [4; 32];
    let mut messages = vec![plaintext(Some(direct), Content::Yo)];
    for at in [18, 8] {
        messages.push(plaintext(
            None,
            Content::Sync(SyncMessage::Read(Read { user: None, group: Some(group), through_sent_at: at })),
        ));
    }
    for plain in messages {
        let (payload, _) = a.encrypt_for(&user, &plain, false, &[]).unwrap();
        receive(&mut b, &payload).unwrap();
        receive(&mut b, &payload).unwrap();
    }
    assert!(b.state.contacts[&hex(&direct)].chat);
    assert_eq!(b.state.sibling_sent_at[&hex(&direct)], 100);
    assert_eq!(b.state.seen_through[&hex(&group)], 18);
    // Handed to the app once, for its notifications (`take_seen`).
    assert_eq!(b.state.seen_due.get(&hex(&group)), Some(&18));
    assert!(b.state.events.is_empty() && b.state.outbox.is_empty());
}

#[test]
fn lookup_and_signed_conflict_keep_usable_sessions_and_reencrypt() {
    use std::sync::Mutex;
    struct LookupHub {
        current: Mutex<api::LookupResult>,
        requests: Mutex<Vec<api::Lookup>>,
    }
    let signer = || identity::account_pair(&[7; 32]).unwrap();
    impl Transport for LookupHub {
        fn request(
            &self,
            _: &str,
            path: &str,
            headers: &[(String, String)],
            body: &[u8],
        ) -> std::result::Result<Response, String> {
            if path == api::inbox_path(0) {
                return Ok(Response { status: 200, body: encode(&api::Inbox { items: vec![], prekeys_left: 10 }) });
            }
            assert_eq!(path, api::LOOKUP);
            let lookup: api::Lookup = decode(body).unwrap();
            let mut found = self.current.lock().unwrap().clone();
            found.bundles.retain(|b| !lookup.except.contains(&b.device_key));
            self.requests.lock().unwrap().push(lookup);
            let mut answer = encode(&api::LookupNamed { found, username: Some(b"Peer".as_slice().into()) });
            let nonce =
                headers.iter().find(|(k, _)| k == hubsig::NONCE_HEADER).and_then(|(_, v)| hubsig::nonce(v)).unwrap();
            let signature = identity::sign(
                &identity::account_pair(&[7; 32]).unwrap(),
                &hubsig::message(&nonce, path, body, &answer),
            );
            answer.extend(signature);
            Ok(Response { status: 200, body: answer })
        }
    }
    let mut c = phone(1, 1);
    c.state.storage_fetched = true;
    let (mut p, mut q, mut r) = (phone(2, 2), phone(2, 3), phone(2, 4));
    let key = p.my_account_key();
    let user = p.user_id().to_owned();
    let hub = Arc::new(LookupHub {
        current: Mutex::new(api::LookupResult {
            user_id: [2; 16],
            account_key: key,
            devices: vec![rostered(&p), rostered(&q)],
            bundles: vec![bundle(&mut p), bundle(&mut q)],
        }),
        requests: Mutex::default(),
    });
    c.transport = hub.clone();
    c.set_hub_keys(vec![identity::account_key(&signer())]);
    c.start_chat(&Handle::user(&[2; 16])).unwrap();
    c.enqueue(&user, plaintext(None, Content::Yo));
    let deliveries = c.deliveries().unwrap();
    let item = &deliveries.items[0];
    let devices = vec![rostered(&q), rostered(&r)];
    let mut body = encode(&api::Devices { account_key: key, devices: devices.clone() });
    body.extend(identity::sign(&signer(), &hubsig::message(&item.nonce.unwrap(), &item.path, &item.envelope, &body)));
    assert!(c.delivered(&deliveries, vec![Some(Ok(Response { status: 409, body }))]).unwrap());
    assert!(c.state.outbox[0].payload.is_none());
    assert!(!c.records.usable(&c.address(&user, 2).unwrap()));
    assert!(c.records.usable(&c.address(&user, 3).unwrap()));
    {
        let mut current = hub.current.lock().unwrap();
        current.devices = devices;
        current.bundles = vec![bundle(&mut r)];
    }
    c.prepare().unwrap();
    let retried = c.deliveries().unwrap();
    let payload: Payload =
        decode(yo_wire::envelope::unpad(&retried.items[0].envelope, &yo_wire::envelope::BUCKETS).unwrap()).unwrap();
    assert_eq!(
        payload.copies.iter().map(|c| c.device_key).collect::<Vec<_>>(),
        [q.state.device_key, r.state.device_key]
    );
    let requests = hub.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|r| r.flags & api::LOOKUP_PREKEYS != 0));
    assert_eq!(requests[1].except, [q.state.device_key]);
}

#[test]
fn sibling_storage_changed_reads_and_merges_dirty_fields() {
    struct StorageRead(Vec<u8>);
    impl Transport for StorageRead {
        fn request(
            &self,
            method: &str,
            path: &str,
            _: &[(String, String)],
            _: &[u8],
        ) -> std::result::Result<Response, String> {
            assert_eq!((method, path), ("GET", api::STORAGE));
            Ok(Response { status: 200, body: self.0.clone() })
        }
    }
    let mut b = phone(1, 2);
    b.state.storage_fetched = true;
    b.set_left_handed(true).unwrap();
    let remote = storage::Storage { version: 3, icon: (Some("Original".into()), 19).into(), ..Default::default() };
    let data = storage::seal(&b.account_secret(), &[1; 16], &remote).unwrap().into();
    b.transport = Arc::new(StorageRead(encode(&api::Storage { version: 3, data })));
    let plain: Plain = decode(&plaintext(None, Content::Sync(SyncMessage::StorageChanged))).unwrap();
    let message = Message::decode(plain.content.as_slice()).unwrap();
    b.receive_sibling(plain, message).unwrap();
    assert_eq!(b.app_icon().as_deref(), Some("Original"));
    assert_eq!(b.left_handed(), Some(true));
    assert_eq!(b.state.storage_version, 3);
    assert!(b.state.storage_dirty);
    assert!(b.state.events.is_empty() && b.state.outbox.is_empty());
}

#[test]
fn an_oversized_envelope_returns_an_error_and_keeps_the_prepared_item() {
    let mut sender = phone(1, 1);
    let peer = phone(2, 2);
    let user = peer.user_id();
    sender.take_devices(user, peer.my_account_key(), &[rostered(&peer)], None).unwrap();
    sender.enqueue(user, plaintext(None, Content::Yo));
    let item = sender.state.outbox.back_mut().unwrap();
    item.payload = Some(vec![5; 65536]);
    item.made_for = vec![peer.state.device_key];
    let before = item.clone();
    let deliveries = sender.deliveries().unwrap();
    assert!(deliveries.items.is_empty());
    assert!(sender.delivered(&deliveries, vec![]).is_err());
    assert_eq!(sender.state.outbox[0], before);
}
