//! yo-yops: how many Yos one core makes and opens, in memory, without a hub or a network.
//!
//!   cargo run --release -p yo-tests --bin yo-yops [-- --ms 1000]
//!
//! The client's own steps as `Client` takes them (`encrypt_for`, `receive`), on its own stores: send (the Yo
//! encoded, Double Ratchet encrypted for each device, the payload padded and sealed to the account key), receive
//! (opened, decrypted, parsed), the round trip Alice to Bob, fan-out to 2 and 5 recipient devices, and the first Yo
//! of a chat (Bob's prekeys made, PQXDH with Kyber, Bob opens the PreKeySignalMessage). Each alone and with the
//! client state sealed after every Yo, as the app saves it. One thread; the numbers depend on the machine. The last
//! column is the same case on the MLS core (commit 930e34e), measured once on the same machine.

use libsignal_protocol::{
    kem, message_decrypt, message_encrypt, process_prekey_bundle, CiphertextMessage, CiphertextMessageType, DeviceId,
    PreKeyBundle, PreKeySignalMessage, ProtocolAddress, PublicKey, SignalMessage,
};
use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime};
use yo_core::identity::{self, Key};
use yo_core::signal::{self, Records};
use yo_core::state::{self, State};
use yo_core::{sealed, Content};
use yo_wire::content::Message;
use yo_wire::envelope::{ForDevice, Payload, Plain};
use yo_wire::{decode, encode};

/// The MLS core's numbers (930e34e, one E-core of an Intel Core Ultra X7 358H): us per op, without and with saving.
const MLS: [(f64, f64); 6] =
    [(263.7, 261.9), (260.8, 282.8), (492.3, 544.3), (405.1, 429.1), (892.2, 934.9), (2036.9, 2085.5)];

struct Device {
    /// The account secret.
    account: Key,
    user: [u8; 16],
    state: State,
    records: Records,
}

impl Device {
    fn new(account: &Key, user: [u8; 16], number: u8) -> Device {
        let device_key: Key = identity::random();
        let registration_id = 1 + u32::from(number);
        let state = State {
            user_id: state::hex(&user),
            username: None,
            account_secret: *account,
            auth_secret: identity::random(),
            device_id: identity::random(),
            device_key,
            device_number: number,
            certificate: identity::certify(&identity::account_pair(account).unwrap(), &device_key).to_bytes(),
            registration_id,
            name: None,
            siblings: Vec::new(),
            own_roster_at: 0,
            seen_through: Default::default(),
            seen_due: Default::default(),
            sibling_sent_at: Default::default(),
            contacts: Default::default(),
            outbox: VecDeque::new(),
            next_seq: 0,
            events: Vec::new(),
            handed: Vec::new(),
            seen: VecDeque::new(),
            prekeys: Default::default(),
            signed_until: 0,
            storage: Default::default(),
            storage_version: 0,
            storage_fetched: false,
            storage_dirty: false,
            legacy_storage_key: None,
            storage_locked: false,
            storage_saved_at: 0,
            last_sent_ms: 0,
            received: VecDeque::new(),
            profile_key: None,
            profile_key_gen: 0,
            requests_open: true,
            access_key_told: false,
            sender_certificate: None,
            certificate_clock: 0,
            registering: None,
            groups: Default::default(),
            roster_asked: Default::default(),
            recovery_lookups: (0, 0),
            group_held: Vec::new(),
            deferred: Default::default(),
            unread: 0,
        };
        let records = Records::new(&Default::default(), &state.account_secret, registration_id).unwrap();
        Device { account: *account, user, state, records }
    }

    /// A new account with `devices` devices, numbered from 1.
    fn account(devices: u8) -> Vec<Device> {
        let account = identity::new_account_secret();
        let user: [u8; 16] = identity::random();
        (1..=devices).map(|n| Device::new(&account, user, n)).collect()
    }

    fn account_key(&self) -> Key {
        yo_tests::account_key(&self.account)
    }

    fn address(&self) -> ProtocolAddress {
        signal::address(&self.user, self.state.device_number).unwrap()
    }

    /// The bundle the hub would hand out for this device: fresh one-time, signed and Kyber prekeys.
    fn bundle(&mut self, id: u32) -> PreKeyBundle {
        let one = self.records.new_prekey(id).unwrap();
        let signed = self.records.new_signed(id, 1).unwrap();
        let kyber = self.records.new_kyber(id, false).unwrap();
        PreKeyBundle::new(
            self.state.registration_id,
            DeviceId::new(self.state.device_number).unwrap(),
            Some((one.id.into(), PublicKey::deserialize(one.key.as_slice()).unwrap())),
            signed.id.into(),
            PublicKey::deserialize(signed.key.as_slice()).unwrap(),
            signed.signature.as_slice().to_vec(),
            kyber.id.into(),
            kem::PublicKey::deserialize(kyber.key.as_slice()).unwrap(),
            kyber.signature.as_slice().to_vec(),
            signal::identity_of(&self.account_key()).unwrap(),
        )
        .unwrap()
    }

    /// PQXDH with each of `to`'s devices, from their bundles.
    fn start(&mut self, to: &mut [Device], id: u32) {
        let local = self.address();
        for device in to {
            let bundle = device.bundle(id);
            let address = device.address();
            self.records.trust(&address, signal::identity_of(&device.account_key()).unwrap());
            let r = &mut self.records;
            signal::now(process_prekey_bundle(
                &address,
                &local,
                &mut r.sessions,
                &mut r.identities,
                &bundle,
                SystemTime::now(),
                &mut rand::rng(),
            ))
            .unwrap();
        }
    }

    /// The whole state as the app's store gets it after every change: its size.
    fn persist(&self) -> usize {
        yo_tests::saved_bytes(&self.state, self.records.export()).len()
    }

    /// A Yo for every device of `to` (one account), sealed to it: one envelope.
    fn send(&mut self, to: &[Device]) -> Vec<u8> {
        let content = Message { sent_at_ms: 1, group: None, content: Content::Yo }.encode().unwrap();
        let plain = encode(&Plain { to: None, content: content.into() });
        let identity = signal::identity_of(&to[0].account_key()).unwrap();
        let local = self.address();
        let mut copies = Vec::new();
        for device in to {
            let address = device.address();
            self.records.trust(&address, identity);
            let r = &mut self.records;
            let message = signal::now(message_encrypt(
                &plain,
                &address,
                &local,
                &mut r.sessions,
                &mut r.identities,
                SystemTime::now(),
                &mut rand::rng(),
            ))
            .unwrap();
            copies.push(ForDevice {
                device_key: device.state.device_key,
                kind: (message.message_type() == CiphertextMessageType::PreKey) as u8,
                message: message.serialize().to_vec().into(),
            });
        }
        let payload = Payload {
            account_key: self.account_key(),
            device_key: self.state.device_key,
            device_id: self.state.device_number,
            sent_at: 0,
            copies,
        };
        let now = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let certificate = yo_tests::sender_certificate(&self.user, self.state.device_number, &self.account_key(), now);
        sealed::testing::seal(&self.account, &certificate, &to[0].account_key(), &encode(&payload)).unwrap()
    }

    fn receive(&mut self, from: &Device, sealed: &[u8]) {
        let own = yo_core::identity::account_pair(&self.state.account_secret).unwrap();
        let now = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let roots = sealed::roots(&[yo_tests::trust_root()]);
        let payload: Payload = decode(&sealed::open(&own, sealed, &roots, now).unwrap().payload).unwrap();
        let copy = payload.copies.iter().find(|c| c.device_key == self.state.device_key).unwrap();
        let bytes = copy.message.as_slice();
        let message = match copy.kind {
            1 => CiphertextMessage::PreKeySignalMessage(PreKeySignalMessage::try_from(bytes).unwrap()),
            _ => CiphertextMessage::SignalMessage(SignalMessage::try_from(bytes).unwrap()),
        };
        let address = signal::address(&from.user, payload.device_id).unwrap();
        self.records.trust(&address, signal::identity_of(&payload.account_key).unwrap());
        let local = self.address();
        let r = &mut self.records;
        let plain = signal::now(message_decrypt(
            &message,
            &address,
            &local,
            &mut r.sessions,
            &mut r.identities,
            &mut r.prekeys,
            &r.signed,
            &mut r.kyber,
            &mut rand::rng(),
        ))
        .unwrap();
        let plain: Plain = decode(&plain).unwrap();
        assert!(matches!(Message::decode(plain.content.as_slice()).unwrap().content, Content::Yo));
    }
}

/// Alice and an account of `devices` devices, a session with each, the first message already answered (so the
/// sessions are past their PreKeySignalMessage, as a chat's after its first Yo).
fn pair(devices: u8) -> (Device, Vec<Device>) {
    let mut alice = Device::account(1).remove(0);
    let mut bob = Device::account(devices);
    alice.start(&mut bob, 1);
    let sealed = alice.send(&bob);
    for b in bob.iter_mut() {
        b.receive(&alice, &sealed);
    }
    let sealed = bob[0].send(std::slice::from_ref(&alice));
    alice.receive(&bob[0], &sealed);
    (alice, bob)
}

/// Runs `op` for `budget`, after a short warm-up: microseconds per op.
fn time(budget: Duration, mut op: impl FnMut()) -> f64 {
    for _ in 0..20 {
        op();
    }
    let (start, mut n) = (Instant::now(), 0u64);
    while start.elapsed() < budget || n < 20 {
        op();
        n += 1;
    }
    start.elapsed().as_secs_f64() * 1e6 / n as f64
}

fn row(i: usize, name: &str, plain: f64, saved: f64) {
    let (m0, m1) = MLS[i];
    println!(
        "{name:<36} {:>8.0} {:>8.1}   {:>8.0} {:>8.1}   {:>7.0} {:>7.0}",
        1e6 / plain,
        plain,
        1e6 / saved,
        saved,
        m0,
        m1
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ms = args.iter().position(|a| a == "--ms").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok());
    let budget = Duration::from_millis(ms.unwrap_or(1000));

    println!("one core, in memory, {budget:?} per case");
    println!("{:<36} {:>17}   {:>17}   {:>15}", "", "state not saved", "state saved each", "MLS (930e34e)");
    println!("{:<36} {:>8} {:>8}   {:>8} {:>8}   {:>7} {:>7}", "", "Yops/s", "us/op", "Yops/s", "us/op", "us", "saved");

    let send = |save: bool| {
        let (mut alice, bob) = pair(1);
        time(budget, || {
            alice.send(&bob);
            if save {
                alice.persist();
            }
        })
    };
    row(0, "send (encode, encrypt, pad, seal)", send(false), send(true));

    // Receive alone: Yos made in batches outside the measured span, then opened in order.
    let receive = |save: bool| {
        let (mut alice, mut bob) = pair(1);
        let (mut total, mut n) = (Duration::ZERO, 0u64);
        while total < budget {
            let batch: Vec<Vec<u8>> = (0..200).map(|_| alice.send(&bob)).collect();
            let start = Instant::now();
            for sealed in &batch {
                bob[0].receive(&alice, sealed);
                if save {
                    bob[0].persist();
                }
                n += 1;
            }
            total += start.elapsed();
        }
        total.as_secs_f64() * 1e6 / n as f64
    };
    row(1, "receive (open, decrypt, parse)", receive(false), receive(true));

    let round = |save: bool| {
        let (mut alice, mut bob) = pair(1);
        time(budget, || {
            let sealed = alice.send(&bob);
            if save {
                alice.persist();
            }
            bob[0].receive(&alice, &sealed);
            if save {
                bob[0].persist();
            }
        })
    };
    row(2, "round trip Alice -> Bob", round(false), round(true));

    for (i, n) in [(3, 2u8), (4, 5)] {
        let fan = |save: bool| {
            let (mut alice, bob) = pair(n);
            time(budget, || {
                alice.send(&bob);
                if save {
                    alice.persist();
                }
            })
        };
        row(i, &format!("send, fan-out to {n} devices"), fan(false), fan(true));
    }

    // The first Yo: Bob's prekeys made, Alice's PQXDH (Kyber1024), her PreKeySignalMessage opened by Bob.
    let first = |save: bool| {
        let mut alice = Device::account(1).remove(0);
        let mut bob = Device::account(1);
        let mut id = 0;
        time(budget, || {
            id += 1;
            bob[0].records.forget("sp", id - 1);
            alice.start(&mut bob, id);
            let sealed = alice.send(&bob);
            if save {
                alice.persist();
            }
            bob[0].receive(&alice, &sealed);
            if save {
                bob[0].persist();
            }
        })
    };
    row(5, "first Yo with PQXDH setup (Kyber)", first(false), first(true));
}
