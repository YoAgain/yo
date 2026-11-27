use super::*;
use std::collections::VecDeque;

pub(super) fn inbox_path(wait: u64) -> String {
    if wait == 0 {
        format!("{}&split=1", api::inbox_path(0))
    } else {
        api::inbox_path(wait)
    }
}

impl Client {
    /// Fetches and processes the inbox until it is empty. Transport failures stop it with nothing lost.
    pub(super) fn receive_all(&mut self, wait_s: u64) -> Result<()> {
        self.storage_fetched()?;
        // What the hub would send again otherwise.
        self.acknowledge_inbox()?;
        let mut wait = wait_s;
        loop {
            let (response, time) = self.fetch_inbox(&inbox_path(wait))?;
            let (more, ids) = self.inbox(response, time, true)?;
            self.received_ids.extend(ids);
            if !more {
                // Groups the inbox named a newer revision of: from the hub now, so their held Yos show.
                self.refresh_groups_quietly();
                return Ok(());
            }
            wait = 0;
        }
    }

    fn fetch_inbox(&mut self, path: &str) -> Result<(Response, u64)> {
        let (headers, mut time) = self.sign("GET", path, &[]);
        let mut response = self.unsigned("GET", path, &headers, &[])?;
        if response.status == status::UNAUTHORIZED {
            // As `signed`: once more, signed anew.
            let (headers, again) = self.sign("GET", path, &[]);
            (response, time) = (self.signed_again(self.unsigned("GET", path, &headers, &[])?)?, again);
        }
        Ok((response, time))
    }

    /// The storage, fetched once before anything is received, so its blocks hold from the first message on.
    pub(super) fn storage_fetched(&mut self) -> Result<()> {
        if self.state.storage_fetched {
            return Ok(());
        }
        match self.sync_storage() {
            Err(error) if retryable(&error) || error == Error::Replaced => Err(error),
            Err(error) => {
                self.drop_one(format!("storage: {error}"));
                self.state.storage_fetched = true;
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }

    /// Processes one inbox answer to a request signed at `requested_at` and acknowledges it; `true` when the hub has
    /// more. The answer may be older than the state: seen envelopes are skipped, an older prekey count ignored. With
    /// `later`, a complete answer is neither saved nor acknowledged here: its ids are returned for `sync_end`.
    pub(super) fn inbox(&mut self, response: Response, requested_at: u64, later: bool) -> Result<(bool, Vec<u64>)> {
        let partial = response.status == 206;
        let (local_full, mut ids) = self.inbox_batch(response, requested_at, later)?;
        if !partial {
            return Ok((local_full, ids));
        }
        let followup = self
            .fetch_inbox(&format!("{}&split=2", api::inbox_path(0)))
            .and_then(|(answer, time)| self.inbox_batch(answer, time, later));
        let (peer_full, peer_ids) = match followup {
            Ok(batch) => batch,
            Err(error) => {
                // A passing failure still hands out the local batch once it is saved.
                self.received_ids.extend(ids);
                return Err(error);
            }
        };
        ids.extend(peer_ids);
        let more = local_full || peer_full;
        if more && !ids.is_empty() {
            // The shorter half also has to advance before asking for another page.
            self.persist()?;
            return Ok((self.ack(ids)?, Vec::new()));
        }
        Ok((more, ids))
    }

    fn inbox_batch(&mut self, response: Response, requested_at: u64, later: bool) -> Result<(bool, Vec<u64>)> {
        if !matches!(response.status, status::OK | 206) {
            return Err(refused(&response));
        }
        let inbox: api::Inbox = decode(&response.body)?;
        self.refused_until_read.clear();
        if requested_at > self.prekeys_known_ms {
            self.state.prekeys.left = inbox.prekeys_left;
            self.prekeys_known_ms = requested_at;
        }
        let full = inbox.items.len() >= MAX_INBOX_FETCH;
        let mut done = Vec::new();
        let mut stopped = None;
        // Parked payloads go first; after one is parked again, its sender's next ones are too, keeping their order.
        // They stay out of the state while the batch is received: each envelope's transaction copies the state.
        // A sender is parked by its account key only once one of its messages opened (or a key no contact here has,
        // whose first message waits for its lookup): a payload names any account it likes, and a forgery naming a
        // contact must never hold back the contact's own messages (spec 3.2).
        let sender_of = |opened: &[u8]| opened.get(16..).and_then(|p| decode::<Payload>(p).ok()).map(|p| p.account_key);
        let mut kept: BTreeSet<Key> = BTreeSet::new();
        let mut refreshed = BTreeSet::new();
        let mut parked = VecDeque::new();
        for payload in std::mem::take(&mut self.state.deferred) {
            let sender = sender_of(&payload);
            if sender.is_some_and(|s| kept.contains(&s)) {
                parked.push_back(payload);
                continue;
            }
            let mut verified = false;
            match self.receive(&payload, &mut refreshed, &mut verified) {
                Ok(()) => {}
                Err(error) if retryable(&error) => {
                    kept.extend(sender.filter(|s| self.parks_by(s, verified)));
                    parked.push_back(payload)
                }
                Err(error) => self.drop_one(error.to_string()),
            }
            self.bound_guest_events();
        }
        for item in inbox.items {
            // Not sealed to this account, or its sender certificate was not good when the hub took it (spec 4.1).
            let payload = match self.open(item.envelope.as_slice(), item.received_ms) {
                Ok(payload) => payload,
                Err(error) => {
                    self.drop_one(error.to_string());
                    done.push(item.id);
                    continue;
                }
            };
            let sender = sender_of(&payload);
            if sender.is_some_and(|s| kept.contains(&s)) {
                self.park(&mut parked, payload);
                done.push(item.id);
                continue;
            }
            let mut verified = false;
            match self.receive(&payload, &mut refreshed, &mut verified) {
                Ok(()) => {}
                // A lookup limit for this sender: its envelopes are parked here and acknowledged, so what comes after
                // them is read and received (no envelope holds up the inbox).
                Err(Error::Refused(status::RATE_LIMITED)) => {
                    kept.extend(sender.filter(|s| self.parks_by(s, verified)));
                    self.park(&mut parked, payload);
                }
                // Not this envelope's fault: it stays at the hub and is read again next time.
                Err(error) if retryable(&error) => {
                    stopped = Some(error);
                    break;
                }
                Err(error) => self.drop_one(error.to_string()),
            }
            self.bound_guest_events();
            done.push(item.id);
        }
        self.state.deferred = parked;
        if later && !full && stopped.is_none() {
            return Ok((false, done));
        }
        if !done.is_empty() {
            // Saved before the hub may forget: what was processed must survive a crash right after the ack.
            self.persist()?;
            if !self.ack(done)? {
                return Ok((false, Vec::new()));
            }
        }
        match stopped {
            Some(error) => Err(error),
            None => Ok((full, Vec::new())),
        }
    }

    /// A guest holds at most `GUEST_EVENTS` events for the app (an app not opened for long): past them the oldest go,
    /// so the newest envelope is always read and shown, and the state stays bounded.
    fn bound_guest_events(&mut self) {
        let Some(returned) = self.guest_events else { return };
        let over = self.state.events.len().saturating_sub(GUEST_EVENTS);
        if over > 0 {
            self.state.events.drain(..over);
            self.guest_events = Some(returned.saturating_sub(over));
            self.dropped += over as u64;
            self.note(format!("{over} events held for the app, too many waiting"));
        }
    }

    /// Whether a sender's next payloads are parked behind one of theirs: its message opened, or no contact here has
    /// the key it names (its first message waits for a lookup, and so do the ones after it).
    fn parks_by(&self, account_key: &Key, verified: bool) -> bool {
        verified
            || (*account_key != self.my_account_key()
                && !self.state.contacts.values().any(|c| c.account_key == *account_key && !c.gone))
    }

    /// Parks a payload for a later sync (`State::deferred`); past `DEFERRED` the oldest parked one is dropped, so
    /// the inbox never stops behind them.
    fn park(&mut self, parked: &mut VecDeque<Vec<u8>>, payload: Vec<u8>) {
        if parked.len() >= DEFERRED {
            parked.pop_front();
            self.drop_one("a parked message, too many waiting for a roster".into());
        }
        parked.push_back(payload);
    }

    /// Acknowledges inbox ids saved as processed; `false` when the ack did not arrive (the hub repeats, they are seen).
    pub(super) fn ack(&mut self, ids: Vec<u64>) -> Result<bool> {
        match self.signed("POST", api::INBOX_ACK, &encode(&api::Ack { ids })) {
            Ok(response) if response.status == status::NO_CONTENT => Ok(true),
            Ok(response) if response.status == status::GONE => Err(Error::Replaced),
            _ => Ok(false),
        }
    }

    /// Acknowledges what `sync_end` received and saved (`unacked`).
    pub(super) fn acknowledge_inbox(&mut self) -> Result<()> {
        if !self.unacked.is_empty() {
            let ids = std::mem::take(&mut self.unacked);
            self.ack(ids)?;
        }
        Ok(())
    }

    /// Marks a session message as processed: its hash, whatever envelope it came in.
    pub(super) fn remember(&mut self, message: &[u8]) {
        self.state.seen.push_back(sha256(message));
        if self.state.seen.len() > SEEN {
            self.state.seen.pop_front();
        }
    }

    /// Whether the hub now lists the payload's device of `user` at its number under its account key (one lookup), the
    /// device then taken in. `unpinned`: a sender pinned only then.
    pub(super) fn hub_lists(&mut self, user: &str, payload: &Payload, unpinned: Option<&Unpinned>) -> Result<bool> {
        let (found, username) = match self.lookup(&Handle::user(&user_bytes(user)?), 0, Vec::new()) {
            Ok(found) => found,
            Err(Error::NotOnYo) => return Ok(false),
            Err(error) => return Err(error),
        };
        let listed = found.account_key == payload.account_key
            && found.devices.iter().any(|d| d.key == payload.device_key && d.id == payload.device_id);
        if !listed {
            return Ok(false);
        }
        let username = match unpinned {
            Some(unpinned) => {
                self.pin(user, unpinned.account_key, unpinned.username.clone());
                None
            }
            None => username,
        };
        self.take_devices(user, found.account_key, &found.devices, username)?;
        Ok(true)
    }

    /// Whether a payload may make this device look `user` up now: once a minute a user, within the day's limit.
    pub(super) fn may_ask_roster(&mut self, user: &str) -> bool {
        let now = (self.clock)();
        let last = self.state.roster_asked.get(user).copied().unwrap_or(0);
        if last != 0 && !passed(now, last, ROSTER_ASK_EVERY_MS) {
            return false;
        }
        if !self.recovery_lookup() {
            return false;
        }
        // Only the last minute's asks matter: older ones go, so the markers stay a handful.
        self.state.roster_asked.retain(|_, at| !passed(now, *at, ROSTER_ASK_EVERY_MS));
        self.state.roster_asked.insert(user.to_owned(), now);
        true
    }

    /// A contact known without a name (a group's member): the hub is asked once per process who it is, so its Yos say
    /// who they are from; a failed lookup is asked again on the next Yo.
    pub(super) fn learn_name(&mut self, user: &str, account_key: Key) {
        if !self.names_asked.insert(user.to_owned()) {
            return;
        }
        let Ok(id) = user_bytes(user) else { return };
        match self.lookup(&Handle::user(&id), 0, Vec::new()) {
            Ok((found, Some(username))) if found.account_key == account_key => self.take_handle(user, username),
            Ok(_) => {}
            Err(error) => {
                self.names_asked.remove(user);
                self.note(format!("name of a member: {error}"));
            }
        }
    }

    /// Takes one of the day's lookups for recovery, if one is left.
    pub(super) fn recovery_lookup(&mut self) -> bool {
        let day = (self.clock)() / (24 * 3600 * 1000);
        if self.state.recovery_lookups.0 != day {
            self.state.recovery_lookups = (day, 0);
        }
        if self.state.recovery_lookups.1 >= self.recovery_lookups_per_day {
            return false;
        }
        self.state.recovery_lookups.1 += 1;
        true
    }

    /// Who sent a payload: this account, a contact pinned to the key, or whom the hub says the key is (`Unpinned`,
    /// pinned only once the message opened: anyone can seal a payload naming any account). `None`: no user's key now.
    pub(super) fn sender(&mut self, account_key: Key) -> Result<Option<(String, Option<Unpinned>)>> {
        if account_key == self.my_account_key() {
            return Ok(Some((self.state.user_id.clone(), None)));
        }
        if let Some((user, c)) = self.state.contacts.iter().find(|(_, c)| c.account_key == account_key && !c.gone) {
            let (user, unnamed) = (user.clone(), c.handles.is_empty());
            if unnamed {
                self.learn_name(&user, account_key);
            }
            return Ok(Some((user, None)));
        }
        // Someone writing first, or a contact a rolled-back state forgot; a forger pays a delivery per lookup (4.1).
        match self.lookup(&Handle::account_key(&account_key), 0, Vec::new()) {
            Ok((found, username)) if found.account_key == account_key => {
                let user = hex(&found.user_id);
                Ok(Some((user, Some(Unpinned { account_key, username, roster: found.devices }))))
            }
            Ok(_) | Err(Error::NotOnYo) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Opens an envelope (spec 4.1): what `receive` takes is the user its certificate names (16 zero bytes when
    /// identified) and the payload. The certificate must name the payload's device and account key.
    pub(super) fn open(&self, envelope: &[u8], received_ms: u64) -> Result<Vec<u8>> {
        let own = identity::account_pair(&self.state.account_secret)?;
        let opened = sealed::open(&own, envelope, &sealed::roots(&self.trust_roots), received_ms)?;
        let payload: Payload = decode(&opened.payload)?;
        let user = match opened.sender {
            Some((user, device, identity)) => {
                if device != payload.device_id || identity != signal::identity_of(&payload.account_key)? {
                    return Err(Error::Protocol("a sender certificate for another"));
                }
                user
            }
            None => [0; 16],
        };
        Ok([&user[..], &opened.payload].concat())
    }

    /// Processes one opened envelope: the copy for this device, decrypted in the session with its sender's device.
    /// `verified`: its message opened in a session with the account it names (the sender is who it says).
    pub(super) fn receive(&mut self, opened: &[u8], refreshed: &mut BTreeSet<Key>, verified: &mut bool) -> Result<()> {
        let mut checked = None;
        let result = self.atomic(|c| c.receive_opened(opened, refreshed, &mut checked, verified));
        // A roster asked is shared, whether or not the envelope committed: the lookup was made.
        refreshed.extend(checked);
        result
    }

    /// Whether a copy opens in the session at `address` with the account key it names, tried and undone: a message
    /// only that account's device can have made (its sessions' keys, or its identity key for a first message).
    fn opens(&mut self, address: &ProtocolAddress, account_key: &Key, kind: u8, bytes: &[u8]) -> bool {
        let Ok(message) = session_message(kind, bytes) else { return false };
        let mut opened = false;
        let _ = self.atomic(|c| {
            opened = c.decrypt(address, account_key, &message).is_ok_and(|r| r.is_ok());
            Err::<(), _>(Error::Internal("only tried"))
        });
        opened
    }

    /// A session message decrypted at `address`, its peer's identity the account key named (the inner result:
    /// libsignal's).
    fn decrypt(
        &mut self,
        address: &ProtocolAddress,
        account_key: &Key,
        message: &CiphertextMessage,
    ) -> Result<std::result::Result<Vec<u8>, SignalProtocolError>> {
        self.records.trust(address, signal::identity_of(account_key)?);
        let local = self.local_address()?;
        let records = &mut self.records;
        Ok(signal::now(message_decrypt(
            message,
            address,
            &local,
            &mut records.sessions,
            &mut records.identities,
            &mut records.prekeys,
            &records.signed,
            &mut records.kyber,
            &mut rand::rng(),
        )))
    }

    pub(super) fn receive_opened(
        &mut self,
        opened: &[u8],
        refreshed: &BTreeSet<Key>,
        checked: &mut Option<Key>,
        verified: &mut bool,
    ) -> Result<()> {
        let (certified, payload) = opened.split_at_checked(16).ok_or(Error::Protocol("opened"))?;
        let certified = (certified != [0; 16]).then(|| hex(certified));
        let mut payload: Payload = decode(payload)?;
        let mine = self.state.device_key;
        // Made before the sender knew this device (it was added since): not for it.
        let Some(copy) = std::mem::take(&mut payload.copies).into_iter().find(|c| c.device_key == mine) else {
            return Ok(());
        };
        if payload.device_key == mine || self.state.seen.contains(&sha256(copy.message.as_slice())) {
            return Ok(());
        }
        let Some((user, unpinned)) = self.sender(payload.account_key)? else {
            self.drop_one("a sender that is no user now".into());
            return Ok(());
        };
        if certified.as_ref().is_some_and(|c| *c != user) {
            self.drop_one("a sender certificate for another user".into());
            return Ok(());
        }

        // Another device of the user under this number: one was replaced, and only the hub knows which (a replaced
        // device can still deliver). Nothing the payload claims is taken in before its message opens: anyone can seal
        // one. So it is opened first (and undone): only a message of that account's asks the hub, or waits for it; one
        // that does not open is no one's to ask about, and goes the way of any that does not (a decryption error).
        let other = unpinned.is_none() && !self.knows_device(&user, &payload.device_key, payload.device_id);
        let address = self.address(&user, payload.device_id)?;
        let bytes = copy.message.as_slice();
        if other && copy.kind != KIND_PLAINTEXT && self.opens(&address, &payload.account_key, copy.kind, bytes) {
            *verified = true;
            // Every envelope in this batch arrived before the lookup: no second lookup for it.
            if refreshed.contains(&payload.account_key) {
                self.note("a message from a replaced device".into());
                return Ok(());
            }
            // Asked at most once a minute a user: a forger claiming numbers costs a lookup a minute, no more.
            if !self.may_ask_roster(&user) {
                // As under a lookup limit: this envelope and the sender's next ones are deferred.
                self.note("a device under another's number: later".into());
                return Err(Error::Refused(status::RATE_LIMITED));
            }
            let (found, username) = self.lookup(&Handle::user(&user_bytes(&user)?), 0, Vec::new())?;
            if found.account_key == payload.account_key {
                self.take_devices(&user, found.account_key, &found.devices, username)?;
            }
            *checked = Some(payload.account_key);
            let current = found.devices.iter().any(|d| d.key == payload.device_key && d.id == payload.device_id);
            if found.account_key != payload.account_key || !current {
                self.note("a message from a replaced device".into());
                return Ok(());
            }
        }
        if copy.kind == KIND_PLAINTEXT {
            // A contact's device at its number, as this device knows it: nothing else is answered (3.2).
            let known = unpinned.is_none() && self.knows_device(&user, &payload.device_key, payload.device_id);
            if !known {
                self.drop_one("a decryption error from a device not known here".into());
                return Ok(());
            }
            self.remember(bytes);
            return self.decryption_error(&user, payload.device_key, &address, bytes);
        }
        if unpinned
            .as_ref()
            .is_some_and(|u| !u.roster.iter().any(|d| d.key == payload.device_key && d.id == payload.device_id))
        {
            self.drop_one("a message from a replaced device: not on the hub's roster".into());
            return Ok(());
        }
        let message = session_message(copy.kind, bytes)?;
        let decrypted = self.decrypt(&address, &payload.account_key, &message)?;
        let plaintext = match decrypted {
            Ok(plaintext) => plaintext,
            Err(error) => {
                // A session out of step (spec 3.2): a `DecryptionErrorMessage` goes to that device so it starts a new
                // session and resends. Also for an unseen "duplicate" made with used message keys by a sender whose
                // state went back. Only to the user's known device at its number (the hub asked at most once a minute),
                // at most `KEPT_FOR_RESEND` per contact; someone not pinned here is pinned for it only when the hub
                // names that device.
                let known = unpinned.is_none() && self.knows_device(&user, &payload.device_key, payload.device_id);
                // A lookup limit at the hub drops only the answer: the message did not open, so its sender is not who
                // it says and nothing is parked by it.
                let rostered = known
                    || (self.may_ask_roster(&user)
                        && match self.hub_lists(&user, &payload, unpinned.as_ref()) {
                            Err(Error::Refused(status::RATE_LIMITED)) => false,
                            listed => listed?,
                        });
                let kind = if copy.kind == KIND_PREKEY {
                    CiphertextMessageType::PreKey
                } else {
                    CiphertextMessageType::Whisper
                };
                let made = DecryptionErrorMessage::for_original(
                    bytes,
                    kind,
                    Timestamp::from_epoch_millis(payload.sent_at),
                    payload.device_id.into(),
                );
                if let (true, Ok(made)) = (rostered, made) {
                    let content = PlaintextContent::from(made).serialized().to_vec();
                    if user == self.state.user_id {
                        // As many due as for a contact (`KEPT_FOR_RESEND`).
                        let due = self.state.outbox.iter().filter(|o| o.user == user && o.device.is_some()).count();
                        if due < KEPT_FOR_RESEND {
                            self.enqueue_plaintext(&user, payload.device_key, content);
                        }
                    } else if let Some(contact) = self.state.contacts.get_mut(&user) {
                        if contact.decryption_errors.len() < KEPT_FOR_RESEND {
                            contact.decryption_errors.push(DecryptionError { device: payload.device_key, content });
                        }
                    }
                }
                self.drop_one(format!("session: {error}"));
                return Ok(());
            }
        };
        // It opened, so it is the account's. A device key unknown here is taken in only once the hub names it; else it
        // is a replaced device's, and dropped (3.2).
        *verified = true;
        self.remember(copy.message.as_slice());
        if let Some(Unpinned { account_key, username, roster }) = unpinned {
            self.take_devices(&user, account_key, &roster, username)?;
        }
        self.note_updated(&user);
        let plain: Plain = decode(&plaintext)?;
        let message = Message::decode(plain.content.as_slice())?;
        if user == self.state.user_id {
            if self.first_time(&user, &payload.device_key, &plaintext) {
                self.receive_sibling(plain, message)?;
            }
            return Ok(());
        }
        if plain.to.is_some() {
            return Err(Error::Protocol("a copy for another"));
        }
        if matches!(message.content, Content::Sync(_)) {
            return Ok(());
        }
        if certified.is_none() && !self.probe_service(&user) {
            self.identified_from(&user)?;
        }
        self.received(&user, &payload.device_key, plain.content.as_slice(), message)
    }

    /// Whether this plaintext from `user` comes for the first time (the last `SEEN` kept), noting it.
    pub(super) fn first_time(&mut self, user: &str, device: &Key, plaintext: &[u8]) -> bool {
        let id = sha256(&[user.as_bytes(), device, plaintext].concat());
        // Ids saved before multi-device name no device: read until `LEGACY_UNTIL_MS`.
        let legacy = (self.wall() < crate::LEGACY_UNTIL_MS).then(|| sha256(&[user.as_bytes(), plaintext].concat()));
        if self.state.received.contains(&id) || legacy.is_some_and(|legacy| self.state.received.contains(&legacy)) {
            return false;
        }
        self.state.received.push_back(id);
        if self.state.received.len() > SEEN {
            self.state.received.pop_front();
        }
        true
    }

    /// The keys of photos and videos are dropped as they arrive, for a client that never opens them.
    pub(super) fn forget_keys(&self, content: Content) -> Content {
        match content {
            Content::Photo(media) if self.media_keys_forgotten => Content::Photo(media::without_key(media)),
            Content::Video { media, duration_ms } if self.media_keys_forgotten => {
                Content::Video { media: media::without_key(media), duration_ms }
            }
            content => content,
        }
    }

    /// A carried profile key with its generation: taken unless older than the one held (spec 3.3).
    pub(super) fn take_key_carried(&mut self, user: &str, key: [u8; 32], generation: u32) {
        let Some(contact) = self.state.contacts.get_mut(user) else { return };
        if carried_key_taken(contact, &key, generation) {
            contact.key_gen = contact.key_gen.max(generation);
            self.adopt_key(user, key);
        }
    }

    /// The one way a contact's key is taken: kept, also in the storage (spec 4.3), and what waits for them goes now as
    /// a contact's. A type 10 tells no generation: the one held stays the bound until a content message tells it
    /// (3.3).
    pub(crate) fn adopt_key(&mut self, user: &str, key: [u8; 32]) {
        let Some(contact) = self.state.contacts.get_mut(user) else { return };
        if contact.profile_key != Some(key) {
            contact.profile_key = Some(key);
            self.store_key(user, Some(key));
        }
        self.state.request_wait.remove(user);
        self.refused_until_read.remove(user);
    }

    /// A message from `user` came identified (spec 3.3): an accepted contact or joined group member gets the current
    /// key back at once, at most once an hour per key, so a missed key heals and request limits bind only strangers.
    pub(super) fn identified_from(&mut self, user: &str) -> Result<()> {
        let now = (self.clock)();
        let in_group = self.in_joined_group(user);
        let Some(contact) = self.state.contacts.get_mut(user) else { return Ok(()) };
        if !(contact.accepted || in_group)
            || contact.blocked
            || contact.gone
            || !passed(now, contact.key_resent_at, HOUR_MS)
        {
            return Ok(());
        }
        contact.key_resent_at = now;
        contact.key_due = true;
        if self.guest_events.is_some() {
            return Ok(());
        }
        self.queue_key(user)
    }

    /// A message from a contact.
    pub(super) fn received(&mut self, user: &str, device: &Key, plaintext: &[u8], message: Message) -> Result<()> {
        if matches!(message.content, Content::Unknown(_)) {
            self.drop_one(yoip::Error::Unsupported.to_string());
            return Ok(());
        }
        if let Content::Probe(token) | Content::ProbeReply(token) = message.content {
            if self.first_time(user, device, plaintext) && message.group.is_none() {
                if matches!(message.content, Content::Probe(_)) {
                    if self.probe_requests.len() < 256 {
                        self.probe_requests.push((user.to_owned(), token));
                    }
                } else if let Some(contact) = self.state.contacts.get_mut(user) {
                    if contact.probe_pending == Some(token) {
                        contact.probe_reply_tag = token;
                        contact.probe_pending = None;
                    }
                }
            }
            return Ok(());
        }
        if let Content::Connect { kind, code } = message.content {
            if message.group.is_none() && self.first_time(user, device, plaintext) {
                self.connection_received(user, kind, code);
            }
            return Ok(());
        }
        // Group content is checked against membership; the probe service stays hidden in one-to-one exchanges.
        let in_group = message.group.is_some() || matches!(message.content, Content::Group(_));
        if self.probe_service(user) && !in_group {
            // Kept locally so repeated probes are not stranger requests; `store_key` keeps them out of the storage.
            if let Some((key, generation)) = Message::profile_key_of(plaintext) {
                self.take_key_carried(user, key, generation);
            }
            if let Content::ProfileKey(key) = &message.content {
                self.adopt_key(user, *key);
            }
            self.first_time(user, device, plaintext);
            return Ok(());
        }
        // The carried key is kept even from a blocked user: after an unblock theirs is a contact's again (spec 3.3).
        if let Some((key, generation)) = Message::profile_key_of(plaintext) {
            self.take_key_carried(user, key, generation);
        }
        // A blocked user's messages are opened (sessions go on) and dropped unseen (3.3); group changes still count.
        if self.blocked(user) && !matches!(message.content, Content::Group(_) | Content::ProfileKey(_)) {
            return Ok(());
        }
        // Requests off: drop strangers' direct messages and group changes, and anyone's Yos to an unjoined group.
        // A group known and not fetched yet holds its Yos until it is (`release_held` decides).
        if !self.state.requests_open && (message.group.is_some() || self.stranger(user)) {
            let tag = match &message.content {
                Content::Group(tag) => Some(tag),
                _ => message.group.as_ref(),
            };
            let unseen = match tag {
                None => shown(&message.content),
                Some(tag) => self
                    .state
                    .groups
                    .get(&groups::group_id_of(&tag.key))
                    .is_none_or(|g| g.revision > 0 && (!g.accepted || g.left)),
            };
            if unseen {
                return Ok(());
            }
        }
        // A copy resent for a decryption error and seen already (3.2): messages differ at least in their time.
        if !self.first_time(user, device, plaintext) {
            return Ok(());
        }
        // Everything but a receipt gets a receipt so the ratchet moves on (spec 3.2); not a request yet (3.3).
        if !matches!(message.content, Content::Receipt) {
            if let Some(contact) = self.state.contacts.get_mut(user).filter(|c| c.accepted && !c.blocked) {
                contact.receipt_due = true;
            }
        }
        match message.content {
            Content::Receipt => Ok(()),
            Content::ProfileKey(key) => {
                self.adopt_key(user, key);
                Ok(())
            }
            Content::Group(tag) => {
                self.group_named(tag, user);
                Ok(())
            }
            content if message.group.is_some() => {
                let tag = message.group.expect("a group");
                self.group_yo(user, tag, message.sent_at_ms, content);
                Ok(())
            }
            // A name from an app before profiles went: kept, so old apps' names still show.
            Content::Profile { name } => {
                if let Some(contact) = self.state.contacts.get_mut(user) {
                    contact.name = Some(name.clone());
                }
                self.push_event(Event::Profile { user: user.to_owned(), name });
                Ok(())
            }
            content => {
                let contact = self.state.contacts.get_mut(user).ok_or(Error::Internal("contact"))?;
                let accepted = contact.accepted;
                contact.receipt_due = accepted;
                if !std::mem::replace(&mut contact.chat, true) {
                    self.push_event(Event::NewChat { user: user.to_owned() });
                }
                let content = self.forget_keys(content);
                self.push_event(Event::Message { user: user.to_owned(), sent_at_ms: message.sent_at_ms, content });
                Ok(())
            }
        }
    }

    /// A `DecryptionErrorMessage` from a contact's device (spec 3.2), taken as Signal does: the session is archived if
    /// it still sends with the named ratchet key, and the message sent at the named time goes again if kept (else a
    /// receipt starts the new session). A forger (not the session's peer) gets at most a message the contact has
    /// already.
    pub(super) fn decryption_error(
        &mut self,
        user: &str,
        device: Key,
        address: &ProtocolAddress,
        bytes: &[u8],
    ) -> Result<()> {
        let Some(error) = PlaintextContent::try_from(bytes)
            .ok()
            .and_then(|content| extract_decryption_error_message_from_serialized_content(content.body()).ok())
        else {
            self.drop_one("a plaintext that is no decryption error".into());
            return Ok(());
        };
        if error.device_id() != u32::from(self.state.device_number) {
            self.drop_one("a decryption error for another device".into());
            return Ok(());
        }
        let archived = error.ratchet_key().is_some_and(|key| self.records.archive_if_current(address, key));
        let sent_at = error.timestamp().epoch_millis();
        // A sibling's: syncs are not kept to resend; a receipt starts the new session, as for a contact.
        if user == self.state.user_id {
            if archived {
                self.queue_own(Content::Receipt)?;
            }
            return Ok(());
        }
        let Some(contact) = self.state.contacts.get(user) else { return Ok(()) };
        let intended = contact.sent_devices.get(&sent_at).is_none_or(|keys| keys.contains(&device));
        let kept = contact
            .sent
            .iter()
            .filter(|_| intended)
            .find(|m| Message::decode(m).is_ok_and(|m| m.sent_at_ms == sent_at))
            .cloned();
        match kept {
            Some(message) => {
                // A group's message only while both are still in it (left, or the user removed: not again).
                let tag = Message::decode(&message).ok().and_then(|m| m.group);
                let in_group = self.both_in(tag.as_ref(), user);
                if in_group {
                    self.enqueue_with_urgency(user, encode(&Plain { to: None, content: message.into() }), true);
                    self.state.outbox.back_mut().expect("enqueued").audience = vec![device];
                }
            }
            None if archived => {
                if let Some(contact) = self.state.contacts.get_mut(user).filter(|c| c.accepted && !c.blocked) {
                    contact.receipt_due = true;
                }
            }
            None => {}
        }
        Ok(())
    }

    /// The receipts and decryption errors due (spec 3.2).
    pub(super) fn answer_due(&mut self) -> Result<()> {
        let due: Vec<(String, bool, Vec<DecryptionError>)> = self
            .state
            .contacts
            .iter()
            .filter(|(_, c)| !c.blocked && !c.gone && !c.unconfirmed)
            .map(|(user, c)| {
                // Not to a request not said yes to (3.3); one whose first message never opened gets one so its session
                // heals.
                let answers = c.accepted || !c.chat || self.in_joined_group(user);
                let errors = if answers { c.decryption_errors.clone() } else { Vec::new() };
                (user.clone(), c.receipt_due && !c.devices.is_empty(), errors)
            })
            .filter(|(_, receipt, errors)| *receipt || !errors.is_empty())
            .collect();
        for (user, receipt, errors) in due {
            self.atomic(|c| {
                for error in &errors {
                    c.enqueue_plaintext(&user, error.device, error.content.clone());
                }
                if receipt {
                    c.send_message(&user, Content::Receipt)?;
                }
                let contact = c.state.contacts.get_mut(&user).ok_or(Error::Internal("contact"))?;
                contact.receipt_due &= !receipt;
                contact.decryption_errors.retain(|e| !errors.contains(e));
                Ok(())
            })?;
        }
        Ok(())
    }
}

/// A copy's session message by its kind (spec 3.2).
fn session_message(kind: u8, bytes: &[u8]) -> Result<CiphertextMessage> {
    match kind {
        KIND_PREKEY => PreKeySignalMessage::try_from(bytes).map(CiphertextMessage::PreKeySignalMessage),
        KIND_MESSAGE => SignalMessage::try_from(bytes).map(CiphertextMessage::SignalMessage),
        _ => return Err(Error::Protocol("message type")),
    }
    .map_err(signal::signal)
}
