use super::*;

impl Client {
    /// Silent probe requests for an echo service; never app events or synced content.
    pub fn take_probe_requests(&mut self) -> Vec<(String, [u8; 32])> {
        std::mem::take(&mut self.probe_requests)
    }

    /// Echoes a probe token without app events. Accepts protocol replies so subsequent probes can use sealed delivery
    /// in both directions.
    pub fn echo_probe(&mut self, user: &str, token: [u8; 32]) -> Result<Vec<Event>> {
        let result = self
            .atomic(|c| {
                c.accept_writing(user);
                c.queue_key(user)?;
                c.send_message(user, Content::ProbeReply(token))?;
                c.encrypt_waiting(user);
                c.persist()
            })
            .and_then(|()| self.deliver_now());
        self.finish_sent(result)
    }

    /// Starts a probe to the reserved service; events received meanwhile stay queued for the inbox consumer.
    pub fn round_trip_probe_begin(&mut self, target: &str, timeout_ms: u64) -> Result<Probe> {
        if !same_name(target, PROBE_HANDLE) {
            return Err(Error::Protocol("probe target must be PING"));
        }
        let handle = Handle::parse(PROBE_HANDLE)?;
        let user = match self.known_user(&handle).filter(|u| self.has_chat(u)) {
            Some(user) => user,
            None => self.start_chat(&handle)?,
        };
        self.storage_fetched()?;
        self.acknowledge_inbox()?;
        let ticket = identity::random();
        let wait = self.probe_wait(timeout_ms);
        self.atomic(|c| {
            c.queue_key(&user)?;
            c.state.contacts.get_mut(&user).expect("service").probe_pending = Some(ticket);
            c.send_message(&user, Content::Probe(ticket))?;
            c.encrypt_waiting(&user);
            c.persist()
        })?;
        // The save above also reserves the signed request for other processes.
        let sending = wait.clone();
        let began = std::time::Instant::now();
        let (received, response) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = received.send(sending.send());
        });
        let delivered = self.deliver_now();
        // A probe never becomes a deferred send; the ratchet state stays saved.
        self.state.outbox.retain(|item| {
            item.user != user
                || !decode::<Plain>(&item.plain)
                    .ok()
                    .and_then(|plain| Message::decode(plain.content.as_slice()).ok())
                    .is_some_and(|message| message.content == Content::Probe(ticket))
        });
        self.persist()?;
        delivered?;
        self.unacked.extend(std::mem::take(&mut self.received_ids));
        Ok(Probe { user, ticket, began, wait, response })
    }

    pub(super) fn probe_wait(&mut self, timeout_ms: u64) -> InboxWait {
        let path = api::inbox_path(timeout_ms.div_ceil(1000).clamp(1, 10));
        let (headers, time) = self.sign("GET", &path, &[]);
        InboxWait { transport: self.transport.clone(), path, headers, time }
    }

    /// Processes a completed long poll, preserving ordinary events for the app.
    pub fn round_trip_probe_poll(&mut self, reply: &mut ProbeReply) -> Result<bool> {
        let (_, ids) = self.inbox(
            std::mem::replace(&mut reply.response, Err("taken".into())).map_err(Error::Network)?,
            reply.wait.time,
            true,
        )?;
        if !ids.is_empty() {
            self.persist()?;
            self.unacked.extend(ids);
        }
        Ok(self.state.contacts.get(&reply.user).is_some_and(|c| c.probe_reply_tag == reply.ticket))
    }

    /// Continues waiting when unrelated mail woke the first long poll.
    pub fn round_trip_probe_wait(&mut self, user: &ProbeReply, timeout_ms: u64) -> Result<Probe> {
        self.acknowledge_inbox()?;
        let wait = self.probe_wait(timeout_ms);
        self.persist()?; // Reserve the signed request before another process can sign.
        let sending = wait.clone();
        let (received, response) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = received.send(sending.send());
        });
        Ok(Probe { began: user.began, user: user.user.clone(), ticket: user.ticket, wait, response })
    }

    /// Locally timed convenience API. No app events are taken from the inbox.
    pub fn round_trip_probe(&mut self, target: &str, timeout_ms: u64) -> Result<Option<u64>> {
        let mut probe = self.round_trip_probe_begin(target, timeout_ms)?;
        let began = probe.began;
        loop {
            let Some(mut reply) = probe.receive(timeout_ms) else { return Ok(None) };
            let done = self.round_trip_probe_poll(&mut reply)?;
            let elapsed = began.elapsed().as_millis() as u64;
            if done || elapsed >= timeout_ms {
                return Ok((done && elapsed < timeout_ms).then_some(elapsed));
            }
            probe = self.round_trip_probe_wait(&reply, timeout_ms - elapsed)?;
        }
    }
}
