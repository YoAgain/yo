//! Encrypted deliveries between devices of one account. None of their contents are app events.
use super::*;
use yo_wire::content::{Read, SyncMessage};

impl Client {
    fn enqueue_siblings(&mut self, plain: Plain, audience: Vec<Key>) {
        if audience.is_empty() {
            return;
        }
        let own = self.state.user_id.clone();
        self.enqueue_with_urgency(&own, encode(&plain), true);
        self.state.outbox.back_mut().expect("enqueued").audience = audience;
    }

    /// The first accepted leg releases one transcript. The remaining group legs lose their intent
    /// in the same state change, including when the app resumes with a partly delivered group Yo.
    pub(super) fn release_sent_copy(&mut self, seq: u64) -> Result<bool> {
        let Some(item) = self.state.outbox.iter_mut().find(|i| i.seq == seq && !i.sent_to_siblings.is_empty()) else {
            return Ok(false);
        };
        let mut plain: Plain = decode(&item.plain)?;
        let group = Message::decode(plain.content.as_slice())?.group;
        if group.is_none() {
            plain.to = Some(user_bytes(&item.user)?);
        }
        let audience = std::mem::take(&mut item.sent_to_siblings);
        if group.is_some() {
            let original = item.plain.clone();
            for leg in self.state.outbox.iter_mut().filter(|i| i.plain == original) {
                leg.sent_to_siblings.clear();
            }
        }
        self.enqueue_siblings(plain, audience);
        Ok(true)
    }

    pub(super) fn queue_seen(&mut self, seen: SyncMessage) -> Result<()> {
        if self.state.siblings.is_empty() {
            return Ok(());
        }
        let content =
            Message { sent_at_ms: self.next_sent_at(), group: None, content: Content::Sync(seen) }.encode()?;
        self.enqueue_siblings(Plain { to: None, content: content.into() }, self.roster_keys(&self.state.user_id));
        Ok(())
    }

    /// The user id (16 bytes in hex) or group id (32 bytes in hex) whose messages were read.
    pub fn mark_seen(&mut self, user_or_group: &str, through_sent_at_ms: u64) -> Result<()> {
        let id = unhex(user_or_group).ok_or(Error::Protocol("thread id"))?;
        let (user, group) = match id.len() {
            16 => (Some(id.try_into().expect("16 bytes")), None),
            32 => (None, Some(id.try_into().expect("32 bytes"))),
            _ => return Err(Error::Protocol("thread id")),
        };
        self.refresh_own_roster()?;
        // No other device of this account: nothing to tell (one device per account, as the hub keeps it now).
        if self.state.siblings.is_empty() {
            return Ok(());
        }
        self.queue_seen(SyncMessage::Read(Read { user, group, through_sent_at: through_sent_at_ms }))?;
        self.persist()?;
        self.deliver_now()
    }

    /// Threads another device of this account showed since the last call, with the time shown through (ms):
    /// the app clears their notifications here. Taken once.
    pub fn take_seen(&mut self) -> Result<Vec<(String, u64)>> {
        if self.state.seen_due.is_empty() {
            return Ok(Vec::new());
        }
        let due = std::mem::take(&mut self.state.seen_due).into_iter().collect();
        self.persist()?;
        Ok(due)
    }

    pub(super) fn receive_sibling(&mut self, plain: Plain, message: Message) -> Result<()> {
        match (plain.to, message.content) {
            (None, Content::Sync(SyncMessage::Read(read))) => {
                let thread = read.user.map(|u| hex(&u)).or_else(|| read.group.map(|g| hex(&g)));
                if let Some(thread) = thread {
                    let at = self.state.seen_through.entry(thread.clone()).or_default();
                    if read.through_sent_at > *at {
                        *at = read.through_sent_at;
                        // For the app: its notifications of this thread go (`take_seen`).
                        self.state.seen_due.insert(thread, read.through_sent_at);
                    }
                }
            }
            (None, Content::Sync(SyncMessage::StorageChanged)) => self.read_storage()?,
            (to, Content::Yo | Content::Photo(_) | Content::Video { .. }) => {
                let thread = match (to, message.group) {
                    (Some(user), None) if hex(&user) != self.state.user_id => {
                        let user = hex(&user);
                        self.state.contacts.entry(user.clone()).or_default().chat = true;
                        user
                    }
                    (None, Some(group)) => groups::group_id_of(&group.key),
                    _ => return Ok(()),
                };
                let at = self.state.sibling_sent_at.entry(thread).or_default();
                *at = (*at).max(message.sent_at_ms);
            }
            _ => {}
        }
        Ok(())
    }
}
