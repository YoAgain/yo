use super::*;

impl Client {
    /// Sends the outbox now, the message just queued and saved. What cannot go now waits: the send succeeded (4.7), so
    /// the app never sends it a second time.
    pub(crate) fn deliver_now(&mut self) -> Result<()> {
        match self.deliver() {
            // A failed save after it: the saved outbox still has it, and a second copy is known by its plaintext.
            Err(error) if !retryable(&error) && !matches!(error, Error::Save(_)) => Err(error),
            _ => Ok(()),
        }
    }

    /// Makes the envelopes of what waits for `user` where its session is there (local work; the rest at send).
    pub(super) fn encrypt_waiting(&mut self, user: &str) {
        if self.delivering.contains(user)
            || self.devices_of(user).is_empty()
            || self.no_session(user)
            || self.blocked(user)
        {
            return;
        }
        for i in 0..self.state.outbox.len() {
            let item = &self.state.outbox[i];
            if item.user != user || !item.payloads.is_empty() || item.device.is_some() || !self.still_in_group(item) {
                continue;
            }
            let plain = item.plain.clone();
            let audience = item.audience.clone();
            if let Ok((payloads, made_for)) = self.atomic(|c| c.encrypt_for(user, &plain, false, &audience)) {
                let item = &mut self.state.outbox[i];
                item.payloads = payloads;
                item.made_for = made_for;
                self.envelopes_unsaved.set(true);
            }
        }
    }

    pub(super) fn enqueue(&mut self, user: &str, plain: Vec<u8>) {
        self.enqueue_with_urgency(user, plain, false);
    }

    pub(super) fn enqueue_with_urgency(&mut self, user: &str, plain: Vec<u8>, non_urgent: bool) {
        self.state.next_seq += 1;
        let (queued_at, seq, user) = ((self.clock)(), self.state.next_seq, user.to_owned());
        self.state.outbox.push_back(Outgoing { queued_at, seq, user, plain, non_urgent, ..Default::default() });
        self.prune_outbox();
    }

    /// Keep recent envelopes, newest first for the cap, without changing their delivery order.
    pub(super) fn prune_outbox(&mut self) -> bool {
        let now = (self.clock)();
        let mut counts = BTreeMap::new();
        let mut dropped = Vec::new();
        for item in self.state.outbox.iter().rev() {
            let count = counts.entry(item.user.clone()).or_insert(0usize);
            if now.saturating_sub(item.queued_at) > OUTBOX_KEPT_MS || *count >= OUTBOX_PER_RECIPIENT {
                dropped.push((item.seq, item.user.clone()));
            } else {
                *count += 1;
            }
        }
        for (seq, user) in &dropped {
            self.key_lost(*seq, user);
            self.drop_one("outbox retention limit".into());
        }
        let seqs: BTreeSet<_> = dropped.iter().map(|(seq, _)| *seq).collect();
        self.state.outbox.retain(|item| !seqs.contains(&item.seq));
        !dropped.is_empty()
    }

    /// Queues a `PlaintextContent` for one device of a user (a decryption error, spec 4.6): no session needed.
    pub(super) fn enqueue_plaintext(&mut self, user: &str, device: Key, content: Vec<u8>) {
        self.enqueue(user, content);
        if let Some(item) = self.state.outbox.back_mut() {
            item.device = Some(device);
        }
    }

    /// Whether this blob went to the user already in the same conversation (1:1 or that group), among the last kept.
    pub(crate) fn sent_media(&self, user: &str, id: &[u8; 16], group: Option<&yo_wire::content::GroupKey>) -> bool {
        self.state.contacts.get(user).is_some_and(|c| {
            c.sent.iter().any(|plaintext| {
                Message::decode(plaintext).is_ok_and(|m| {
                    m.group.as_ref().map(|g| &g.key) == group
                        && matches!(m.content, Content::Photo(media) | Content::Video { media, .. } if &media.id == id)
                })
            })
        })
    }

    // ---- delivery ----

    /// Sends the outbox in order per user, taking in any device the hub names (4.4), until the outbox moves.
    pub(super) fn deliver(&mut self) -> Result<()> {
        let mut identified = false;
        let mut result = Ok(());
        for _ in 0..DEVICE_ROUNDS {
            self.prepare()?;
            let deliveries = self.deliveries()?;
            identified |= deliveries.items.iter().any(|d| !d.sealed && d.access.is_some());
            let answers = deliveries.send();
            match self.delivered(&deliveries, answers) {
                Ok(true) => {}
                other => {
                    result = other.map(|_| ());
                    break;
                }
            }
        }
        // Sent identified for want of a certificate: one now, for the next to go sealed.
        if identified {
            self.refresh_certificate()?;
        }
        result
    }

    /// Whether a queued message may still go: one of a group only while this account and its user are in the group.
    pub(super) fn still_in_group(&self, item: &Outgoing) -> bool {
        let tag = decode::<yo_wire::envelope::Plain>(&item.plain)
            .ok()
            .and_then(|plain| Message::decode(plain.content.as_slice()).ok())
            .and_then(|m| m.group);
        self.both_in(tag.as_ref(), &item.user)
    }

    /// The sessions the outbox needs, from the hub. A user it cannot get them for now waits.
    pub(super) fn prepare(&mut self) -> Result<()> {
        if self.prune_outbox() {
            self.persist()?;
        }
        let now = (self.clock)();
        let outdated: Vec<String> =
            self.state.contacts.iter().filter(|(_, c)| c.outdated_at > 0).map(|(user, _)| user.clone()).collect();
        for user in outdated {
            self.note_updated(&user);
        }
        let users: BTreeSet<String> = self
            .state
            .outbox
            .iter()
            .filter(|item| item.device.is_none() && !self.blocked(&item.user) && !self.delivering.contains(&item.user))
            .filter(|item| self.roster_unknown(&item.user) || self.missing_session(&item.user, &item.audience))
            .filter(|item| {
                self.state.contacts.get(&item.user).is_none_or(|c| passed(now, c.outdated_at, OUTDATED_RETRY_MS))
            })
            .map(|item| item.user.clone())
            .collect();
        for user in users {
            match self.fetch_bundles(&user) {
                Ok(()) => {}
                Err(error) if retryable(&error) || error == Error::Unconfirmed => {}
                Err(error @ (Error::Save(_) | Error::Replaced)) => return Err(error),
                Err(Error::NotOnYo) => self.user_gone(&user),
                Err(error) => self.drop_one(format!("sessions: {error}")),
            }
        }
        Ok(())
    }

    /// What of the outbox can go now, in order per user, encrypted for the user's device with its device check (4.4);
    /// its users are marked as on their way. Local work: `Deliveries::send` posts, `delivered` takes the answers.
    pub(super) fn deliveries(&mut self) -> Result<Deliveries> {
        if self.replaced.get() {
            return Err(Error::Replaced);
        }
        if self.prune_outbox() {
            self.persist()?;
        }
        // Users whose envelopes another flush is sending, or that cannot be written to now: theirs wait, in order.
        let mut waiting = self.delivering.clone();
        let now = self.monotonic();
        // A request's hold ends at its time or with their key, however it came (an extension may have taken it in).
        let contacts = &self.state.contacts;
        self.request_wait
            .retain(|user, (until, _)| *until > now && contacts.get(user).is_none_or(|c| c.profile_key.is_none()));
        let mut encrypted = false;
        let mut i = 0;
        while i < self.state.outbox.len() {
            let user = self.state.outbox[i].user.clone();
            // Nothing goes to a blocked user (4.8), nor to one whose new key waits for confirmation (3.4).
            let held = self.blocked(&user) || self.state.contacts.get(&user).is_some_and(|c| c.unconfirmed);
            if waiting.contains(&user) || held {
                waiting.insert(user);
                i += 1;
                continue;
            }
            // A group's Yo or media only while both are still in it (a change goes to everyone and carries no tag).
            if !self.still_in_group(&self.state.outbox[i]) {
                self.state.outbox.remove(i);
                self.note("a group's message after its member left".into());
                continue;
            }
            let current = self.roster_keys(&user);
            let item = &self.state.outbox[i];
            // Never redirect a repair or transcript to a device that joined later.
            if item.device.is_some_and(|key| !current.contains(&key))
                || (!item.audience.is_empty() && !item.audience.iter().any(|key| current.contains(key)))
            {
                self.state.outbox.remove(i);
                continue;
            }
            let item = &self.state.outbox[i];
            if item.payloads.is_empty() || item.made_for != current {
                // A plaintext (a decryption error) needs no session, only the device.
                if (item.device.is_none() && self.missing_session(&user, &item.audience)) || self.roster_unknown(&user)
                {
                    waiting.insert(user);
                    i += 1;
                    continue;
                }
                let (plain, plaintext) = (item.plain.clone(), item.device.is_some());
                let audience = item.device.map_or_else(|| item.audience.clone(), |d| vec![d]);
                match self.atomic(|c| c.encrypt_for(&user, &plain, plaintext, &audience)) {
                    Ok((payloads, made_for)) => {
                        let item = &mut self.state.outbox[i];
                        item.payloads = payloads;
                        item.made_for = made_for;
                        encrypted = true;
                        self.envelopes_unsaved.set(true);
                    }
                    Err(error) => {
                        self.note(format!("encrypt: {error}"));
                        waiting.insert(user);
                    }
                }
            }
            i += 1;
        }
        // Saved before anything leaves: a crash after the hub took an envelope must not bring back the ratchet that
        // made it. A repeat after a crash is harmless: the receiver skips it.
        if encrypted || self.envelopes_unsaved.get() {
            self.persist()?;
        }
        let mut items = Vec::new();
        let mut too_large = Vec::new();
        // Sealed while the certificate is in date; else identified and signed at once, and a fresh one fetched after.
        let certificate = self.certificate();
        let own = identity::account_pair(&self.state.account_secret)?;
        let mut first = BTreeSet::new();
        for item in &self.state.outbox {
            if item.payloads.is_empty() {
                continue;
            }
            if waiting.contains(&item.user) || self.refused_until_read.contains(&item.user) {
                continue;
            }
            // A request held back by the hub's limit (`429`) waits, all but the key ahead of it: keys first (4.10).
            let leading = first.insert(item.user.clone());
            let key_may = leading && is_key(item);
            if self.request_wait.get(&item.user).is_some_and(|(_, key_refused)| *key_refused || !key_may) {
                continue;
            }
            // To this account's other devices: identified and signed, so the hub leaves out this one (Signal's sync).
            let sibling = item.user == self.state.user_id;
            let (account_key, access) = if sibling {
                (self.my_account_key(), None)
            } else if let Some(contact) = self.state.contacts.get(&item.user) {
                (contact.account_key, contact.profile_key)
            } else {
                continue;
            };
            let Ok(user) = user_bytes(&item.user) else { continue };
            let hash = yo_wire::devices::hash(&account_key, &item.made_for);
            let nonce = self.hub_nonce();
            let mut headers = vec![
                (api::HEADER_DEVICES.to_owned(), yo_wire::b64(&hash)),
                (api::HEADER_COPIES.to_owned(), "1".to_owned()),
                (
                    api::HEADER_URGENT.to_owned(),
                    urgent(&item.plain, item.non_urgent || item.device.is_some()).to_string(),
                ),
            ];
            if let Some(key) = access {
                headers.push((api::HEADER_ACCESS.to_owned(), yo_wire::b64(&access_key(&key))));
            }
            // Without their key, a fellow member's group send token (spec 4.9): sealed all the same.
            let endorsement = match (access, &certificate) {
                (None, Some(_)) if !sibling => self.group_send_token(&item.user),
                _ => None,
            };
            let endorsed = endorsement.is_some();
            if let Some(token) = endorsement {
                headers.push((api::HEADER_GROUP_SEND.to_owned(), token));
            }
            // Identified for want of a certificate, or as a request (spec 4.10); an envelope per device.
            let identity = signal::identity_of(&account_key)?;
            let sealed = certificate.as_ref().filter(|_| access.is_some() || endorsed);
            let copies: Result<Vec<api::CopyFor>> = item
                .payloads
                .iter()
                .map(|p| {
                    let envelope = match sealed {
                        Some(certificate) => sealed::seal(certificate, &own, &identity, &p.payload)?,
                        None => yo_wire::envelope::pad(&p.payload, &yo_wire::envelope::BUCKETS)?,
                    };
                    Ok(api::CopyFor { device: p.device, envelope: envelope.into() })
                })
                .collect();
            // Too large for an envelope (or a delivery): dropped, it can never go.
            let body = copies.map(|copies| encode(&api::Copies { copies }));
            let body = match body {
                Ok(body) if body.len() <= yo_wire::limits::MAX_DELIVERY_BODY => body,
                _ => {
                    too_large.push(item.seq);
                    continue;
                }
            };
            items.push(Delivery {
                access,
                endorsed,
                sealed: sealed.is_some(),
                seq: item.seq,
                user: item.user.clone(),
                path: api::deliver_path(&user),
                headers: with_nonce(headers, nonce),
                envelope: body,
                nonce,
            });
        }
        if !too_large.is_empty() {
            self.state.outbox.retain(|item| !too_large.contains(&item.seq));
            for _ in &too_large {
                self.drop_one("too large for an envelope".into());
            }
        }
        if items.iter().any(|d| !d.sealed && d.access.is_some()) {
            self.note("sent identified: no sender certificate in date".into());
        }
        for delivery in items.iter_mut().filter(|d| !d.sealed) {
            let (signed, _) = self.sign("POST", &delivery.path, &delivery.envelope);
            delivery.headers.extend(signed);
        }
        self.delivering.extend(items.iter().map(|d| d.user.clone()));
        Ok(Deliveries { transport: self.transport.clone(), items, to: None })
    }

    /// Takes the hub's answers: what it took leaves the outbox, what it refused for good is dropped, the rest waits.
    /// A `409` names the user's device as it is: taken in, and `true`, so the outbox goes again for it.
    pub(super) fn delivered(&mut self, deliveries: &Deliveries, answers: Vec<Option<Answer>>) -> Result<bool> {
        let mut done = BTreeSet::new();
        let mut gone = BTreeSet::new();
        let mut recover = BTreeSet::new();
        let mut changed = false;
        let mut result = Ok(());
        for (item, answer) in deliveries.items.iter().zip(answers) {
            self.delivering.remove(&item.user);
            // Signed with a time the hub's window missed: its clock kept, the envelope goes again, signed anew.
            if let Some(Ok(response)) = &answer {
                if !item.sealed && self.hub_told_time(response) {
                    changed = true;
                    continue;
                }
            }
            if let Some(answer) = &answer {
                self.last_answer.insert(item.user.clone(), answer.as_ref().map_or(0, |r| r.status));
            }
            match answer {
                Some(Ok(response)) if response.status == status::ACCEPTED => {
                    done.insert(item.seq);
                    if item.endorsed {
                        self.endorsed_sent += 1;
                    }
                }
                Some(Ok(response)) if response.status == status::CONFLICT => {
                    let answer =
                        hub_signed(&self.hub_keys, item.nonce.as_ref(), &item.path, &item.envelope, &response.body);
                    let roster = answer.and_then(|answer| Ok(decode::<api::Devices>(answer)?));
                    match roster.and_then(|devices| crate::validate_roster(&devices.devices).map(|()| devices)) {
                        Ok(devices) => match self
                            .read_before_roster(&item.user, &devices.devices)
                            .and_then(|()| self.take_devices(&item.user, devices.account_key, &devices.devices, None))
                        {
                            Ok(()) => changed = true,
                            // The inbox could not be read first: the envelope waits like any "not now".
                            Err(error) if retryable(&error) => self.note(format!("roster later: {error}")),
                            Err(error) => result = Err(error),
                        },
                        // The hub did not take it: kept, and tried again like any "not now".
                        Err(_) => self.note("hub answered 409 without a readable roster".into()),
                    }
                }
                Some(Ok(response)) if response.status == status::NOT_FOUND => {
                    done.insert(item.seq);
                    gone.insert(item.user.clone());
                }
                // The key shown is not the recipient's any more, or they take no requests and this account is no
                // contact (4.10).
                Some(Ok(response))
                    if response.status == status::FORBIDDEN
                        || (response.status == status::UNAUTHORIZED && item.access.is_some()) =>
                {
                    // A contact's key that changed: forgotten, the new one may wait in the inbox; it goes again after
                    // the next receive. Only a request's `403` drops it: a forgotten key never loses a message.
                    if let Some(contact) =
                        self.state.contacts.get_mut(&item.user).filter(|c| c.profile_key == item.access)
                    {
                        contact.profile_key = None;
                        self.forget_key_stored(&item.user);
                    }
                    // A group send token refused (expired): the endorsements are fetched anew, it goes again after the
                    // next read.
                    if item.endorsed {
                        self.endorsement_refused(&item.user);
                    }
                    match self.state.outbox.iter_mut().find(|o| o.seq == item.seq) {
                        Some(kept) if item.access.is_some() || item.endorsed => {
                            // One refresh for this envelope; a second refusal goes identified.
                            if !kept.refused {
                                recover.insert(item.user.clone());
                            }
                            kept.refused = true;
                            changed = true;
                        }
                        _ => {
                            done.insert(item.seq);
                            self.key_lost(item.seq, &item.user);
                            self.drop_one("the recipient takes no requests".into());
                        }
                    }
                }
                Some(Ok(response)) if matches!(limit_of(&response), Some(Limit::Recipient(_))) => {
                    if let Some(Limit::Recipient(wait)) = limit_of(&response) {
                        let key_refused = self.request_wait.contains_key(&item.user);
                        self.request_wait.insert(item.user.clone(), (self.monotonic() + wait, key_refused));
                    }
                }
                Some(Ok(response)) if gone_for_good(response.status) => {
                    done.insert(item.seq);
                    self.key_lost(item.seq, &item.user);
                    self.drop_one(format!("hub refused delivery: {}", response.status));
                }
                // Rate limit, hub trouble, no network, or held behind such an answer: later, in the same order.
                _ => {}
            }
        }
        self.state.outbox.retain(|item| !done.contains(&item.seq));
        for user in gone {
            self.user_gone(&user);
        }
        for user in recover {
            // The hub never has the plaintext profile key: its encrypted update is read first, then the device
            // refreshed.
            self.receive_all(0)?;
            self.fetch_bundles(&user)?;
        }
        result.map(|()| changed)
    }

    /// A key's envelope the hub would not take: due again with the next occasion (spec 4.10).
    pub(super) fn key_lost(&mut self, seq: u64, user: &str) {
        if self.state.outbox.iter().any(|o| o.seq == seq && is_key(o)) {
            if let Some(contact) = self.state.contacts.get_mut(user) {
                (contact.key_given, contact.key_resent_at) = (false, 0);
            }
        }
    }

    /// The hub knows no such user any more (`404`): its session and what waits for it go, its handles are forgotten
    /// (the name may be someone else's now), and sending to it is `NotOnYo`.
    pub(super) fn user_gone(&mut self, user: &str) {
        let Some(contact) = self.state.contacts.get_mut(user) else { return };
        contact.gone = true;
        contact.handles.clear();
        self.forget_device(user);
        self.state.outbox.retain(|item| item.user != user);
    }
}
