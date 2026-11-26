//! What one device of an account tells its other devices (spec 3.2, Signal's sync messages): the threads it showed
//! and that the storage changed. A device that joined after this one gets nothing from here until it has written
//! here itself (`siblings_held`): one the hub lists again after it was replaced, its keys gone, never does. (One
//! whose keys someone has can write: against that, as in Signal, the app is told of every device added.) None of it
//! is an app event.
use super::*;
use yoip::protocol::content::{Read, SyncMessage, MAX_READ};

/// The most threads whose newest message is kept (`last_in`).
const KEPT_THREADS: usize = 1000;

impl Client {
    /// A sync to this account's other devices but the held ones; `false`: none to take it.
    pub(super) fn queue_sync(&mut self, sync: SyncMessage) -> Result<bool> {
        self.queue_own(Content::Sync(sync))
    }

    /// Content for this account's other devices but the held ones (a sync, or a receipt that starts a new session).
    pub(super) fn queue_own(&mut self, content: Content) -> Result<bool> {
        let held = &self.state.siblings_held;
        let audience: Vec<Key> = self.state.siblings.iter().map(|d| d.key).filter(|k| !held.contains(k)).collect();
        if audience.is_empty() {
            return Ok(false);
        }
        let content = Message { sent_at_ms: self.next_sent_at(), group: None, content }.encode()?;
        let own = self.state.user_id.clone();
        self.enqueue_with_urgency(&own, encode(&Plain { to: None, content: content.into() }), true);
        self.state.outbox.back_mut().expect("enqueued").audience = audience;
        Ok(true)
    }

    /// A copy of a 1:1 message just queued for `user`, for this account's other devices (Signal's sent transcript):
    /// the same message, `to` naming whom it went to.
    pub(super) fn queue_transcript(&mut self, user: &str, message: Vec<u8>) -> Result<()> {
        let held = &self.state.siblings_held;
        let audience: Vec<Key> = self.state.siblings.iter().map(|d| d.key).filter(|k| !held.contains(k)).collect();
        if audience.is_empty() {
            return Ok(());
        }
        let to = user_bytes(user)?;
        let own = self.state.user_id.clone();
        self.enqueue_with_urgency(&own, encode(&Plain { to: Some(to), content: message.into() }), true);
        self.state.outbox.back_mut().expect("enqueued").audience = audience;
        Ok(())
    }

    /// Notes the newest message of a thread (user or group id), for what `mark_seen` names.
    pub(super) fn note_in(&mut self, thread: &str, sent_at_ms: u64) {
        let at = self.state.last_in.entry(thread.to_owned()).or_default();
        *at = (*at).max(sent_at_ms);
        while self.state.last_in.len() > KEPT_THREADS {
            let Some(oldest) = self.state.last_in.iter().min_by_key(|(_, at)| **at).map(|(t, _)| t.clone()) else {
                break;
            };
            self.state.last_in.remove(&oldest);
        }
    }

    /// The threads the app showed (user ids, 16 bytes in hex, or group ids, 32): this account's other devices let
    /// their notifications of them go, through the newest message received here (Signal's `SyncMessage.Read`).
    pub fn mark_seen(&mut self, threads: &[String]) -> Result<()> {
        self.refresh_own_roster()?;
        // No other device of this account: nothing to tell.
        if self.state.siblings.is_empty() {
            return Ok(());
        }
        let mut reads = Vec::new();
        for thread in threads {
            let thread = thread.to_ascii_lowercase();
            let Some(&through) = self.state.last_in.get(&thread) else { continue };
            let id = unhex(&thread).ok_or(Error::Protocol("thread id"))?;
            let (user, group) = match id.len() {
                16 => (Some(id.try_into().expect("16 bytes")), None),
                32 => (None, Some(id.try_into().expect("32 bytes"))),
                _ => return Err(Error::Protocol("thread id")),
            };
            if self.state.seen_through.get(&thread).is_none_or(|at| through > *at) {
                reads.push((thread, Read { user, group, through_sent_at: through }));
            }
        }
        if reads.is_empty() {
            return Ok(());
        }
        for reads in reads.chunks(MAX_READ) {
            // Noted as told only once it goes (none goes while every other device is held).
            if !self.queue_sync(SyncMessage::Read(reads.iter().map(|(_, r)| r.clone()).collect()))? {
                return Ok(());
            }
            for (thread, read) in reads {
                self.state.seen_through.insert(thread.clone(), read.through_sent_at);
            }
        }
        self.persist()?;
        self.deliver_now()
    }

    /// Threads another device of this account showed since the last call, with the message (sent at, ms) shown
    /// through: the app clears their notifications here. Taken once.
    pub fn take_seen(&mut self) -> Result<Vec<(String, u64)>> {
        if self.state.seen_due.is_empty() {
            return Ok(Vec::new());
        }
        let due = std::mem::take(&mut self.state.seen_due).into_iter().collect();
        self.persist()?;
        Ok(due)
    }

    /// A message from another device of this account, opened in its session.
    pub(super) fn receive_sibling(&mut self, device: Key, plain: Plain, message: Message) -> Result<()> {
        // It wrote here with its keys: it gets this device's syncs from now on.
        self.state.siblings_held.retain(|held| *held != device);
        // A copy of what it sent to someone (Signal's sent transcript): the thread shows it as sent from here, and
        // writing accepted the contact. Only a 1:1 message of a contact this device knows (a new chat is not made
        // from it: its keys come with the next lookup); a group's is not taken yet.
        if let Some(to) = plain.to {
            let user = hex(&to);
            if message.group.is_none() && shown(&message.content) && user != self.state.user_id {
                if let Some(contact) = self.state.contacts.get_mut(&user).filter(|c| !c.probe_service || c.chat) {
                    contact.accepted = true;
                    contact.chat = true;
                    self.push_event(Event::Sent { user, sent_at_ms: message.sent_at_ms, content: message.content });
                }
            }
            return Ok(());
        }
        match message.content {
            Content::Sync(SyncMessage::Read(reads)) => {
                for read in reads {
                    let Some(thread) = read.user.map(|u| hex(&u)).or_else(|| read.group.map(|g| hex(&g))) else {
                        continue;
                    };
                    let at = self.state.seen_through.entry(thread.clone()).or_default();
                    if read.through_sent_at > *at {
                        *at = read.through_sent_at;
                        // For the app: its notifications of this thread go (`take_seen`).
                        self.state.seen_due.insert(thread, read.through_sent_at);
                    }
                }
            }
            // Read once the inbox is (`after_sync`), as Signal's job fetches the latest storage.
            Content::Sync(SyncMessage::StorageChanged) => {
                self.state.storage_fetched = false;
                self.after_due = true;
            }
            _ => {}
        }
        Ok(())
    }
}
