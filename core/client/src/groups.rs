//! Groups (spec 4.9): up to `MAX_GROUP` accounts, no state at the hub. A group's Yo goes to each member over the
//! sessions the members have with each other; its id and the sender's revision ride along. A change (made, renamed,
//! someone added, removed or gone) is the whole group, one revision on, to everyone in it before and after: the higher
//! revision stands, of two at the same revision the one by the lower user id. Only a member's change counts.

use crate::state::{hex, unhex, Group};
use crate::{retryable, shown, user_bytes, wall_ms, Client, Content, Error, Event, Result, KEPT_FOR_RETRY};
use yo_wire::content::{GroupId, GroupInfo, GroupTag, Message, GROUP_ID_LEN, MAX_GROUP};
use yo_wire::envelope::Plain;
use yo_wire::handle::Handle;
use yo_wire::limits::MAX_BLOB_FETCHES;
use yo_wire::{encode, UserId};

/// A group as the app shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupView {
    /// Its id (hex).
    pub id: String,
    /// Empty while only its Yos are known, not yet the group itself (it is asked for).
    pub name: String,
    /// Its members by user id (hex), this account among them while it is in.
    pub members: Vec<String>,
    pub revision: u32,
    /// This account is not in it (any more).
    pub left: bool,
}

fn group_id(id: &str) -> Result<GroupId> {
    unhex(id).and_then(|bytes| bytes.try_into().ok()).ok_or(Error::Protocol("a group id"))
}

impl Client {
    /// The groups this account is or was in.
    pub fn groups(&self) -> Vec<GroupView> {
        self.state
            .groups
            .iter()
            .map(|(id, g)| GroupView {
                id: id.clone(),
                name: g.name.clone(),
                members: g.members.clone(),
                revision: g.revision,
                left: g.left,
            })
            .collect()
    }

    /// Makes a group of this account and `members` (user ids), named `name`, and tells them; returns its id.
    pub fn create_group(&mut self, name: &str, members: &[String]) -> Result<(String, Vec<Event>)> {
        let mut id = [0u8; GROUP_ID_LEN];
        rand::fill(&mut id);
        let id = hex(&id);
        let me = self.state.user_id.clone();
        let mut all: Vec<String> = members.iter().map(|m| m.to_ascii_lowercase()).collect();
        all.push(me.clone());
        let result = self.change_group(&id, all, name, Some(&me));
        self.finish(result).map(|events| (id, events))
    }

    pub fn rename_group(&mut self, id: &str, name: &str) -> Result<Vec<Event>> {
        let result = self.group(id).and_then(|g| self.change_group(id, g.members, name, None));
        self.finish(result)
    }

    pub fn add_to_group(&mut self, id: &str, users: &[String]) -> Result<Vec<Event>> {
        let result = self.group(id).and_then(|g| {
            let members = g.members.into_iter().chain(users.iter().map(|u| u.to_ascii_lowercase())).collect();
            self.change_group(id, members, &g.name, None)
        });
        self.finish(result)
    }

    pub fn remove_from_group(&mut self, id: &str, users: &[String]) -> Result<Vec<Event>> {
        let result = self.group(id).and_then(|g| {
            let members = g.members.into_iter().filter(|m| !users.iter().any(|u| u.eq_ignore_ascii_case(m))).collect();
            self.change_group(id, members, &g.name, None)
        });
        self.finish(result)
    }

    /// Leaves a group: the others are told, this account gets nothing of it any more.
    pub fn leave_group(&mut self, id: &str) -> Result<Vec<Event>> {
        let me = self.state.user_id.clone();
        let result = self.group(id).and_then(|g| {
            let members = g.members.into_iter().filter(|m| *m != me).collect();
            self.change_group(id, members, &g.name, None)
        });
        self.finish(result)
    }

    /// A Yo (with a photo or video or not) to a group this account is in.
    pub fn send_group(&mut self, id: &str, content: Content) -> Result<Vec<Event>> {
        let result = self.send_group_inner(id, content);
        self.finish(result)
    }

    /// How many fetches a photo or video to the group is uploaded for: every device of its other members, and this
    /// account's other devices (`MAX_BLOB_FETCHES` at most).
    pub fn group_media_fetches(&self, id: &str) -> Option<u32> {
        let group = self.state.groups.get(id).filter(|g| !g.left)?;
        let devices: usize = group
            .members
            .iter()
            .filter(|m| **m != self.state.user_id)
            .filter_map(|m| self.state.contacts.get(m))
            .map(|c| c.devices.len().max(1))
            .sum();
        Some(((devices + self.state.own.len()) as u32).clamp(1, MAX_BLOB_FETCHES))
    }

    /// Opens what a group's photo or video needs (a session with each member), as a send would, and says for how many
    /// fetches to upload it.
    pub fn open_group(&mut self, id: &str) -> Result<(u32, Vec<Event>)> {
        let result = self.group(id).and_then(|g| {
            self.reach_all(&g.members)?;
            Ok(self.group_media_fetches(id).unwrap_or(1))
        });
        let fetches = result.as_ref().ok().copied();
        let events = self.finish(result)?;
        Ok((fetches.unwrap_or(1), events))
    }

    // ---- inside ----

    fn group(&self, id: &str) -> Result<Group> {
        self.state.groups.get(id).filter(|g| !g.left).cloned().ok_or(Error::Protocol("not in this group"))
    }

    fn send_group_inner(&mut self, id: &str, content: Content) -> Result<()> {
        if !shown(&content) || !matches!(content, Content::Yo | Content::Photo(_) | Content::Video { .. }) {
            return Err(Error::Protocol("not content for a group"));
        }
        // As a send to one: what waits goes, the inbox is taken in (a change of the group among it).
        match self.deliver().and_then(|()| self.receive_all(0)) {
            Ok(()) => {}
            Err(error) if retryable(&error) => {}
            Err(error) => return Err(error),
        }
        let group = self.group(id)?;
        let others = self.reach_all(&group.members)?;
        self.send_profiles();
        let tag = GroupTag { id: group_id(id)?, revision: group.revision };
        self.atomic(|c| {
            c.enqueue_group(&others, Some(tag), content)?;
            c.persist()
        })?;
        match self.deliver() {
            Ok(()) | Err(Error::Network(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// A session with each other member this device can write to (spec 4.2), without making them a chat of their own:
    /// the members it can reach. One gone, blocked by the hub, or without devices is left out (noted).
    fn reach_all(&mut self, members: &[String]) -> Result<Vec<String>> {
        let mut reached = Vec::new();
        let me = self.state.user_id.clone();
        for member in members.iter().filter(|m| **m != me) {
            if self.state.contacts.get(member).is_some_and(|c| c.gone) {
                continue;
            }
            if !self.has_chat(member) {
                let chat = self.state.contacts.get(member).is_some_and(|c| c.chat);
                match self.start_chat(&Handle::user(&user_bytes(member)?)) {
                    Ok(_) => {}
                    Err(error) if retryable(&error) => return Err(error),
                    Err(error) => {
                        self.note(format!("group member: {error}"));
                        continue;
                    }
                }
                // A member is no friend for it: no chat of its own in the app.
                if let Some(contact) = self.state.contacts.get_mut(member) {
                    contact.chat = chat;
                }
            }
            reached.push(member.clone());
        }
        Ok(reached)
    }

    /// One message for each of `users` and a copy for this account's other devices: the same plaintext, one time.
    fn enqueue_group(&mut self, users: &[String], group: Option<GroupTag>, content: Content) -> Result<()> {
        let sent_at_ms = wall_ms().max(self.state.last_sent_ms + 1);
        self.state.last_sent_ms = sent_at_ms;
        let keep = shown(&content);
        let message = Message { sent_at_ms, group, content }.encode()?;
        for user in users {
            self.enqueue(user, encode(&Plain { to: None, content: message.clone().into() }));
            if keep {
                if let Some(contact) = self.state.contacts.get_mut(user) {
                    contact.sent.push_back(message.clone());
                    while contact.sent.len() > KEPT_FOR_RETRY {
                        contact.sent.pop_front();
                    }
                }
            }
        }
        if keep && !self.state.own.is_empty() {
            let me = self.state.user_id.clone();
            self.enqueue(&me, encode(&Plain { to: None, content: message.into() }));
        }
        Ok(())
    }

    /// Makes revision + 1 of a group with these members and name, tells everyone in it before and after, and keeps it.
    /// `creating`: a new group (by this account), revision 1.
    fn change_group(&mut self, id: &str, members: Vec<String>, name: &str, creating: Option<&str>) -> Result<()> {
        let me = self.state.user_id.clone();
        let mut members = members;
        members.sort();
        members.dedup();
        if members.len() > MAX_GROUP {
            return Err(Error::Protocol("a group has 20 members at most"));
        }
        let ids: Vec<UserId> = members.iter().map(|m| user_bytes(m)).collect::<Result<_>>()?;
        let before = match creating {
            Some(_) => Vec::new(),
            None => self.group(id)?.members,
        };
        let revision = match creating {
            Some(_) => 1,
            None => self.state.groups.get(id).map_or(1, |g| g.revision + 1),
        };
        let info = GroupInfo { id: group_id(id)?, revision, name: name.to_owned(), members: ids };
        // Checked as it goes on the wire: the name and the members as they may be.
        Message { sent_at_ms: 0, group: None, content: Content::Group(info.clone()) }.encode()?;
        match self.deliver().and_then(|()| self.receive_all(0)) {
            Ok(()) => {}
            Err(error) if retryable(&error) => {}
            Err(error) => return Err(error),
        }
        let mut told: Vec<String> = before.into_iter().chain(members.iter().cloned()).collect();
        told.sort();
        told.dedup();
        let reached = self.reach_all(&told)?;
        self.send_profiles();
        let left = !members.contains(&me);
        self.atomic(|c| {
            c.enqueue_group(&reached, None, Content::Group(info))?;
            let group = Group { name: name.to_owned(), members, revision, by: me.clone(), left };
            c.state.groups.insert(id.to_owned(), group);
            c.push_event(Event::GroupChanged { group: id.to_owned() });
            c.persist()
        })?;
        match self.deliver() {
            Ok(()) | Err(Error::Network(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// A group as `from` (or, `own`, this account's other device) has it: taken if it is newer and from a member.
    pub(crate) fn group_update(&mut self, from: &str, info: GroupInfo, own: bool) {
        let id = hex(&info.id);
        let me = self.state.user_id.clone();
        let members: Vec<String> = info.members.iter().map(|m| hex(m)).collect();
        let newer = match self.state.groups.get(&id) {
            // New here, or known only by its Yos: from a member of it as it is now.
            None => own || members.iter().any(|m| m == from),
            Some(g) if g.revision == 0 => own || members.iter().any(|m| m == from),
            Some(g) => {
                (own || g.members.iter().any(|m| m == from))
                    && (info.revision > g.revision || (info.revision == g.revision && from < g.by.as_str()))
            }
        };
        if !newer {
            // A member behind: it gets the group as it is here.
            let behind =
                self.state.groups.get(&id).is_some_and(|g| {
                    !own && !g.left && info.revision < g.revision && g.members.iter().any(|m| m == from)
                });
            if behind {
                self.group_asked(from, &id);
            } else {
                self.note("a group change not taken".into());
            }
            return;
        }
        let left = !members.contains(&me);
        let group = Group { name: info.name, members, revision: info.revision, by: from.to_owned(), left };
        self.state.groups.insert(id.clone(), group);
        self.push_event(Event::GroupChanged { group: id });
    }

    /// `from` asks for a group (spec 4.9): it goes to them as it is here, if they are in it.
    pub(crate) fn group_asked(&mut self, from: &str, id: &str) {
        let Some(group) = self.state.groups.get(id).filter(|g| !g.left && g.revision > 0).cloned() else { return };
        if !group.members.iter().any(|m| m == from) {
            return;
        }
        let (Ok(gid), Ok(ids)) =
            (group_id(id), group.members.iter().map(|m| user_bytes(m)).collect::<Result<Vec<_>>>())
        else {
            return;
        };
        let info = GroupInfo { id: gid, revision: group.revision, name: group.name, members: ids };
        if let Err(error) = self.enqueue_group(&[from.to_owned()], None, Content::Group(info)) {
            self.note(format!("group: {error}"));
        }
    }

    /// A Yo from `from` to a group: shown if they are in it; a group not known yet, or a newer revision of it, is asked
    /// of them.
    pub(crate) fn group_yo(&mut self, from: &str, tag: GroupTag, sent_at_ms: u64, content: Content) {
        let id = hex(&tag.id);
        let me = self.state.user_id.clone();
        let ask = match self.state.groups.get(&id) {
            None => {
                // Known by its Yo only: shown at once, the group itself asked for.
                let mut members = vec![from.to_owned(), me];
                members.sort();
                let group = Group { name: String::new(), members, revision: 0, by: from.to_owned(), left: false };
                self.state.groups.insert(id.clone(), group);
                self.push_event(Event::GroupChanged { group: id.clone() });
                true
            }
            Some(g) if g.left || (g.revision > 0 && !g.members.iter().any(|m| m == from)) => {
                self.note("a Yo to a group from no member".into());
                return;
            }
            Some(g) => tag.revision > g.revision,
        };
        if ask {
            if let Err(error) = self.enqueue_group(&[from.to_owned()], None, Content::GroupAsk { id: tag.id }) {
                self.note(format!("group: {error}"));
            }
        }
        if let Some(contact) = self.state.contacts.get_mut(from) {
            contact.receipt_due = true;
        }
        let content = self.forget_keys(content);
        self.push_event(Event::GroupMessage { group: id, user: from.to_owned(), sent_at_ms, content });
    }
}
