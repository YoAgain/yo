use super::*;

/// One side of a safety number: a user id and its account key.
type Side = (Vec<u8>, Key);

impl Client {
    /// Says yes to a request (spec 4.10): a contact from now on, sent the profile key, the name and a receipt.
    pub fn accept(&mut self, who: &str) -> Result<Vec<Event>> {
        let result = (|| {
            let user = self.user_named(&Handle::parse(who)?)?.0;
            let name = self.state.name.is_some();
            let contact = self.state.contacts.entry(user).or_default();
            contact.accepted = true;
            contact.key_due = true;
            contact.receipt_due = contact.chat;
            contact.profile_due = name;
            self.persist()?;
            self.profile_keys()?;
            self.send_profiles();
            self.deliver()
        })();
        self.finish(result)
    }

    /// Says no to a request: it is gone from the list (`chat` off); what comes from them later is a request again.
    pub fn decline(&mut self, who: &str) -> Result<()> {
        let user = self.user_named(&Handle::parse(who)?)?.0;
        self.atomic(|c| {
            if let Some(contact) = c.state.contacts.get_mut(&user).filter(|c| !c.accepted) {
                contact.chat = false;
            }
            let theirs = |e: &Event| matches!(e, Event::Message { user: from, .. } if *from == user);
            c.state.events.retain(|e| !theirs(e));
            c.persist()
        })
    }

    /// Whether strangers may send requests (spec 4.10); off, the hub refuses them, and only contacts reach this
    /// account.
    pub fn set_requests_open(&mut self, open: bool) -> Result<()> {
        self.state.requests_open = open;
        self.state.access_key_told = false;
        self.persist()?;
        self.profile_keys()
    }

    pub fn requests_open(&self) -> bool {
        self.state.requests_open
    }

    /// A new profile key after a block: the blocked one's deliveries become requests, every other contact gets it.
    pub(super) fn rotate_profile_key(&mut self) {
        self.state.profile_key = Some(new_profile_key());
        self.state.profile_key_gen = self.state.profile_key_gen.max(1) + 1;
        self.store_own_key();
        self.state.access_key_told = false;
        self.key_due_to_all();
    }

    /// Sets the name contacts see, and tells every contact this device writes to.
    pub fn set_name(&mut self, name: &str) -> Result<Vec<Event>> {
        let result = (|| {
            Message { sent_at_ms: 0, group: None, content: Content::Profile { name: name.to_owned() } }.encode()?;
            self.state.name = Some(name.to_owned());
            // Not to a request (spec 4.10): it gets the name with a yes.
            for contact in self.state.contacts.values_mut().filter(|c| c.chat && c.accepted) {
                contact.profile_due = true;
            }
            self.send_profiles();
            self.deliver()
        })();
        self.finish(result)
    }

    /// Tells the name to every contact due it that can take a message now: not blocked (4.8), unconfirmed (3.4) or
    /// gone.
    pub(super) fn send_profiles(&mut self) {
        let Some(name) = self.state.name.clone() else { return };
        let due: Vec<String> = self
            .state
            .contacts
            .iter()
            .filter(|(_, c)| {
                c.profile_due && c.accepted && !c.blocked && !c.unconfirmed && !c.gone && c.device.is_some()
            })
            .map(|(user, _)| user.clone())
            .collect();
        for user in due {
            let sent = self.atomic(|c| {
                c.send_message(&user, Content::Profile { name: name.clone() })?;
                c.state.contacts.get_mut(&user).ok_or(Error::Internal("contact"))?.profile_due = false;
                Ok(())
            });
            if let Err(error) = sent {
                self.note(format!("name: {error}"));
            }
        }
    }

    pub fn contacts(&self) -> Vec<ContactInfo> {
        self.state
            .contacts
            .iter()
            .filter(|(_, c)| !c.probe_service || c.chat)
            .map(|(user, c)| ContactInfo {
                user: user.clone(),
                handles: c.handles.clone(),
                name: c.name.clone(),
                verified: c.verified,
                unconfirmed: c.unconfirmed,
                blocked: c.blocked,
                gone: c.gone,
                outdated: c.outdated_at > 0,
                accepted: c.accepted,
            })
            .collect()
    }

    /// The session with a contact's device (user id), with what it is like.
    pub fn sessions_with(&self, user: &str) -> Vec<SessionInfo> {
        let Some(contact) = self.state.contacts.get(user) else { return Vec::new() };
        contact
            .device
            .iter()
            .map(|device| SessionInfo {
                device: hex(&device.key),
                usable: self.address(user, device.slot).is_ok_and(|a| self.records.usable(&a)),
                retrying: contact.decryption_errors.iter().any(|e| e.device == device.key)
                    || self.state.outbox.iter().any(|item| item.user == user && item.device == Some(device.key)),
            })
            .collect()
    }

    /// The account key a contact is pinned to (hex): what the safety number is made of (3.4).
    pub fn account_key_of(&self, user: &str) -> Option<String> {
        self.state.contacts.get(user).map(|c| hex(&c.account_key))
    }

    /// The contact a handle names, as this device knows it.
    pub(super) fn contact_user(&self, who: &str) -> Result<String> {
        self.known_user(&Handle::parse(who)?).ok_or(Error::NotOnYo)
    }

    /// Who a name is, asked without holding the client: `whois_begin` (locked) signs the lookup (no prekey spent),
    /// `Whois::send` (unlocked) asks. Nothing is kept until a message.
    pub fn whois_begin(&mut self, who: &str) -> Result<Whois> {
        let handle = Handle::parse(who)?;
        if self.is_me(&handle) {
            return Err(Error::Protocol("oneself"));
        }
        let body = encode(&api::Lookup { handle, flags: api::LOOKUP_USERNAME, except: Vec::new() });
        let (headers, _) = self.sign("POST", api::LOOKUP, &body);
        let nonce = self.hub_nonce();
        // The time signed saved before the request leaves: another process of this device signs later.
        self.persist()?;
        Ok(Whois {
            transport: self.transport.clone(),
            headers: with_nonce(headers, nonce),
            body,
            me: self.state.user_id.clone(),
            hub_keys: self.hub_keys.clone(),
            nonce,
        })
    }

    /// Takes in what `Whois::send` said before a send by user id (spec 4.2): the contact is known by the name now, and
    /// no other contact any more.
    pub fn whois_end(&mut self, name: &str, user: &str, username: Option<String>) -> Result<()> {
        let Ok(handle) = Handle::parse(name) else { return Ok(()) };
        if !self.state.contacts.contains_key(user) {
            return Ok(());
        }
        self.take_handle(user, username.unwrap_or_else(|| handle.display()));
        self.persist()
    }

    /// Whether `to` names a user by its id (hex), not by a name.
    pub fn is_user_id(to: &str) -> bool {
        Handle::parse(to).is_ok_and(|handle| handle.kind == KIND_USER)
    }

    /// The user a handle names: known here, else asked of the hub (no prekey); with the username its owner writes.
    pub(super) fn user_named(&mut self, handle: &Handle) -> Result<(String, Option<String>)> {
        if self.is_me(handle) {
            return Err(Error::Protocol("oneself"));
        }
        let (user, shown) = match self.known_user(handle) {
            Some(user) => (user, None),
            None => {
                let (found, username) = self.lookup(handle, 0, Vec::new())?;
                let shown = username.or_else(|| (handle.kind != KIND_USER).then(|| handle.display()));
                (hex(&found.user_id), shown)
            }
        };
        if user == self.state.user_id {
            return Err(Error::Protocol("oneself"));
        }
        Ok((user, shown))
    }

    /// The safety number with a contact (spec 3.4): this user's id and account key, and theirs as pinned here.
    pub fn safety_number(&self, who: &str) -> Result<String> {
        let ((mine, my_key), (theirs, key)) = self.safety_sides(who)?;
        identity::safety_number((&mine, &my_key), (&theirs, &key))
    }

    /// Both sides of the safety number with a contact: this user's id and key, and theirs as pinned here.
    fn safety_sides(&self, who: &str) -> Result<(Side, Side)> {
        let user = self.contact_user(who)?;
        let key = self.state.contacts.get(&user).ok_or(Error::NotOnYo)?.account_key;
        let (mine, theirs) = (unhex(&self.state.user_id).unwrap_or_default(), unhex(&user).unwrap_or_default());
        Ok(((mine, self.my_account_key()), (theirs, key)))
    }

    /// The safety number's QR code payload as this user shows it to a contact (spec 3.4).
    pub fn safety_code(&self, who: &str) -> Result<Vec<u8>> {
        let ((mine, my_key), (theirs, key)) = self.safety_sides(who)?;
        identity::safety_code((&mine, &my_key), (&theirs, &key))
    }

    /// Whether the code scanned off the contact's screen matches (spec 3.4, `compare`).
    pub fn safety_code_matches(&self, who: &str, scanned: &[u8]) -> Result<bool> {
        let ((mine, my_key), (theirs, key)) = self.safety_sides(who)?;
        identity::safety_code_matches((&mine, &my_key), (&theirs, &key), scanned)
    }

    /// Marks a contact verified (or not) after comparing safety numbers. Any contact can be verified (spec 3.4).
    pub fn set_verified(&mut self, who: &str, verified: bool) -> Result<()> {
        let user = self.contact_user(who)?;
        // Saved, or not made at all: a change kept only in memory would hold until a restart undid it.
        self.atomic(|c| {
            let contact = c.state.contacts.get_mut(&user).ok_or(Error::NotOnYo)?;
            contact.verified = verified;
            if verified {
                contact.unconfirmed = false;
            }
            c.persist()
        })
    }

    /// Accepts a verified contact's new account key: sending to it goes on (unverified until compared again).
    pub fn confirm_identity(&mut self, who: &str) -> Result<()> {
        let user = self.contact_user(who)?;
        self.atomic(|c| {
            c.state.contacts.get_mut(&user).ok_or(Error::NotOnYo)?.unconfirmed = false;
            c.persist()
        })
    }

    /// Blocks or unblocks a user by user id or any handle: nothing from them is shown or sent to them (spec 4.8).
    pub fn set_blocked(&mut self, who: &str, blocked: bool) -> Result<()> {
        let (user, shown) = self.user_named(&Handle::parse(who)?)?;
        // Saved, or not made at all: a block only in memory would be lost on restart with what came meanwhile.
        self.atomic(|c| {
            c.apply_block(&user, blocked);
            if blocked && c.state.profile_key.is_some() {
                c.rotate_profile_key();
            }
            if let Some(shown) = shown {
                note_handle(c.state.contacts.get_mut(&user).expect("blocked"), shown);
            }
            // Kept in the storage too, so a new phone has it (spec 6.9).
            c.storage_block(&user, blocked);
            c.persist()
        })?;
        // The new key out now, so the blocked one's next delivery is a request (spec 4.10).
        match self.profile_keys() {
            Err(error) if retryable(&error) => {}
            other => other?,
        }
        self.persist()?;
        Ok(())
    }

    /// Blocks or unblocks a user on this device, also hiding what of theirs waits to be handed out.
    pub(crate) fn apply_block(&mut self, user: &str, blocked: bool) {
        let in_group = self.in_joined_group(user);
        let contact = self.state.contacts.entry(user.to_owned()).or_default();
        // Unblocked: the key made at the block goes to them, else what they send comes as a request (spec 4.10).
        if contact.blocked && !blocked && !contact.key_given && (contact.accepted || in_group) {
            contact.key_due = true;
        }
        contact.blocked = blocked;
        if blocked {
            // From waiting and from handed-out, unacknowledged events, which a restart hands out again.
            let theirs = |e: &Event| {
                matches!(e, Event::Message { user: from, .. }
                    | Event::GroupMessage { user: from, .. } if from == user)
            };
            self.state.events.retain(|e| !theirs(e));
            self.state.handed.retain(|e| !theirs(e));
        }
    }

    /// Reports a user to the operator (spec 6.3): `api::REPORT_*`. Nothing they sent goes with it.
    pub fn report(&mut self, who: &str, reason: u8) -> Result<()> {
        let (user, _) = self.user_named(&Handle::parse(who)?)?;
        let body = encode(&api::Report { user_id: user_bytes(&user)?, reason });
        let response = self.signed("POST", api::REPORT, &body)?;
        match response.status {
            status::NO_CONTENT => Ok(()),
            status::NOT_FOUND => Err(Error::NotOnYo),
            _ => Err(refused(&response)),
        }
    }

    /// Signs a request to send without the client; the signing clock is saved first (spec 6.2).
    fn request(&mut self, method: &'static str, path: &'static str, body: Vec<u8>) -> Result<Request> {
        let (headers, _) = self.sign(method, path, &body);
        self.persist()?;
        Ok(Request { transport: self.transport.clone(), method, path, headers, body })
    }

    /// A read-only connectivity check (`Request::status`).
    pub fn status_probe_begin(&mut self) -> Result<Request> {
        self.request("GET", api::CERTIFICATE, Vec::new())
    }

    /// Feedback for the operator (spec 6.3), readable by the hub. Empty or too long text is refused here.
    pub fn feedback_begin(&mut self, text: &str, app_version: &str) -> Result<Request> {
        self.support_message_begin(text, app_version, api::FEEDBACK)
    }

    pub fn send_support_begin(&mut self, text: &str, app_version: &str) -> Result<Request> {
        self.support_message_begin(text, app_version, api::SUPPORT)
    }

    fn support_message_begin(&mut self, text: &str, app_version: &str, path: &'static str) -> Result<Request> {
        let feedback = api::Feedback { text: text.as_bytes().into(), app_version: app_version.as_bytes().into() };
        feedback.check().map_err(|_| Error::Protocol("feedback empty or too long"))?;
        self.request("POST", path, encode(&feedback))
    }

    /// The support thread (`Request::thread`), or with `through` its messages up to there marked read.
    pub fn support_begin(&mut self, through: Option<u64>) -> Result<Request> {
        match through {
            Some(through) => self.request("POST", api::SUPPORT_READ, encode(&api::SupportRead { through })),
            None => self.request("GET", api::SUPPORT, Vec::new()),
        }
    }

    /// Deletes the account at the hub with everything it holds (spec 6.3); the app then forgets its state. `Replaced`:
    /// this device was out already.
    pub fn delete_account(&mut self) -> Result<()> {
        let response = self.signed("DELETE", api::ACCOUNT, &[])?;
        expect(response, status::NO_CONTENT).map(drop)
    }

    /// Claims a username, kept as written ("Chris": found as "chris" and "CHRIS" too), or gives it up with `None`.
    pub fn set_username(&mut self, name: Option<&str>) -> Result<()> {
        let canonical = match name {
            Some(name) => {
                let handle = Handle::parse(name)?;
                if handle.kind != KIND_USERNAME {
                    return Err(Error::Protocol("not a username"));
                }
                Some(String::from_utf8_lossy(handle.value.as_slice()).into_owned())
            }
            None => None,
        };
        let body = encode(&api::SetUsername { username: canonical.as_ref().map(|n| n.as_bytes().to_vec().into()) });
        expect(self.signed("PUT", api::USERNAME, &body)?, status::NO_CONTENT)?;
        self.state.username = canonical;
        self.persist()
    }
}
