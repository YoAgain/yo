//! Groups (spec 4.9): up to `MAX_GROUP` accounts, no state at the hub. A group's Yo goes to each member over the
//! sessions the members have with each other; its id and the sender's revision ride along. A change (made, renamed,
//! someone added, removed or gone) is the whole group, one revision on, to everyone in it before and after: the higher
//! revision stands, of two at the same revision the one by the lower user id. Only a member's change counts.

use crate::state::{hex, unhex, Group, Mine};
use crate::{retryable, shown, user_bytes, Client, Content, Error, Event, Result, KEPT_FOR_RETRY};
use yo_wire::content::{group_hash, GroupId, GroupInfo, GroupTag, Message, GROUP_HASH_LEN, GROUP_ID_LEN, MAX_GROUP};
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
    /// Joined: else someone not yet a contact added this account to it, a request (spec 4.10).
    pub accepted: bool,
}

/// A refusal that holds for now only: `retryable`, a signature refused (`401`: the clock off, or a replay), a save that
/// failed, this device replaced.
fn not_now(error: &Error) -> bool {
    retryable(error) || matches!(error, Error::Refused(401) | Error::Save(_) | Error::Replaced)
}

fn group_id(id: &str) -> Result<GroupId> {
    unhex(id).and_then(|bytes| bytes.try_into().ok()).ok_or(Error::Protocol("a group id"))
}

/// `group_hash` of a group as this device has it.
fn hash_of(id: &GroupId, group: &Group) -> Result<[u8; GROUP_HASH_LEN]> {
    let members = group.members.iter().map(|m| user_bytes(m)).collect::<Result<Vec<_>>>()?;
    Ok(group_hash(id, &group.name, &members))
}

/// What a group Yo names: the group as this device has it.
fn tag_of(id: &str, group: &Group) -> Result<GroupTag> {
    let id = group_id(id)?;
    Ok(GroupTag { id, revision: group.revision, hash: hash_of(&id, group)? })
}

/// Groups this device keeps at most: a sender naming new group ids cannot fill it (its asks are bounded the same).
const MAX_GROUPS: usize = 256;
/// Group Yos held at once from senders not in their group yet (`State::group_held`).
const MAX_HELD: usize = 64;

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
                accepted: g.accepted,
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
        let name = name.to_owned();
        if self.state.groups.len() >= MAX_GROUPS {
            return Err(Error::Protocol("too many groups"));
        }
        let result = self.change_group(&id, true, move |_| Ok((all, name)));
        self.finish(result).map(|events| (id, events))
    }

    /// For tests: a group's message naming any revision, to one user, as only a broken or hostile client sends it.
    #[cfg(feature = "testing")]
    pub fn send_group_tagged_for_tests(&mut self, to: &str, id: &str, revision: u32, content: Content) -> Result<()> {
        // As this device would only if it were in the group with them: its own view says so, so it leaves.
        let me = self.state.user_id.clone();
        let group = self.state.groups.entry(id.to_owned()).or_insert_with(|| Group {
            name: String::new(),
            members: Vec::new(),
            revision,
            by: me.clone(),
            left: false,
            before: Vec::new(),
            accepted: true,
            synced: Default::default(),
            mine: None,
        });
        group.left = false;
        let mut members = vec![me, to.to_owned()];
        members.sort();
        group.members = members;
        let tag = GroupTag { revision, ..tag_of(id, group)? };
        self.queue(&[to.to_owned()], Some(tag), content)?;
        self.persist()?;
        self.deliver_now()
    }

    /// For tests: this device's group changed in place, at the same revision, telling nobody: two members whose
    /// lists diverged (a change lost, a bug).
    #[cfg(feature = "testing")]
    pub fn set_group_for_tests(&mut self, id: &str, name: &str, members: &[String]) -> Result<()> {
        let group = self.state.groups.get_mut(id).ok_or(Error::Protocol("not in this group"))?;
        let mut members = members.to_vec();
        members.sort();
        (group.name, group.members) = (name.to_owned(), members);
        self.persist()
    }

    pub fn rename_group(&mut self, id: &str, name: &str) -> Result<Vec<Event>> {
        let name = name.to_owned();
        let result = self.change_group(id, false, move |g| Ok((g.members.clone(), name)));
        self.finish(result)
    }

    pub fn add_to_group(&mut self, id: &str, users: &[String]) -> Result<Vec<Event>> {
        let users: Vec<String> = users.iter().map(|u| u.to_ascii_lowercase()).collect();
        let result = self
            .change_group(id, false, move |g| Ok((g.members.iter().cloned().chain(users).collect(), g.name.clone())));
        self.finish(result)
    }

    pub fn remove_from_group(&mut self, id: &str, users: &[String]) -> Result<Vec<Event>> {
        let users = users.to_vec();
        let result = self.change_group(id, false, move |g| {
            let members = g.members.iter().filter(|m| !users.iter().any(|u| u.eq_ignore_ascii_case(m))).cloned();
            Ok((members.collect(), g.name.clone()))
        });
        self.finish(result)
    }

    /// Leaves a group: the others are told, this account gets nothing of it any more.
    pub fn leave_group(&mut self, id: &str) -> Result<Vec<Event>> {
        let me = self.state.user_id.clone();
        let result = self.change_group(id, false, move |g| {
            Ok((g.members.iter().filter(|m| **m != me).cloned().collect(), g.name.clone()))
        });
        self.finish(result)
    }

    /// A Yo (with a photo or video or not) to a group this account is in.
    pub fn send_group(&mut self, id: &str, content: Content) -> Result<Vec<Event>> {
        let result = self.send_group_inner(id, content);
        self.finish_sent(result)
    }

    /// How many fetches a photo or video to the group is uploaded for: every device of its other members, and this
    /// account's other devices (`MAX_BLOB_FETCHES` at most).
    pub fn group_media_fetches(&self, id: &str) -> Option<u32> {
        let group = self.state.groups.get(id).filter(|g| !g.left)?;
        let devices: usize = group
            .members
            .iter()
            .filter(|m| **m != self.state.user_id)
            // A member not known here yet has one device at least (its session is made as the Yo goes).
            .map(|m| self.state.contacts.get(m).map_or(1, |c| c.devices.len().max(1)))
            .sum();
        Some(((devices + self.state.own.len()) as u32).clamp(1, MAX_BLOB_FETCHES))
    }

    /// Opens what a group's photo or video needs (a session with each member), as a send would, and says for how many
    /// fetches to upload it.
    pub fn open_group(&mut self, id: &str) -> Result<(u32, Vec<Event>)> {
        let result = self.group(id).and_then(|g| {
            self.not_empty(&g)?;
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
        self.catch_up()?;
        let group = self.group(id)?;
        self.not_empty(&group)?;
        let others = self.reach_all(&group.members)?;
        self.send_profiles();
        let tag = tag_of(id, &group)?;
        self.atomic(|c| {
            c.queue(&others, Some(tag), content)?;
            c.persist()
        })?;
        self.deliver_now()
    }

    /// A group of this account alone takes no Yo (`GroupEmpty`): the app offers to delete it.
    fn not_empty(&self, group: &Group) -> Result<()> {
        match group.members.iter().any(|m| *m != self.state.user_id) {
            true => Ok(()),
            false => Err(Error::GroupEmpty),
        }
    }

    /// A session with each other member this device can write to (spec 4.2), without making them a chat of their own:
    /// the members it can reach. One gone, blocked by the hub, or without devices is left out (noted); one that cannot
    /// be reached now fails the call.
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
                    // Not now (no network, a limit, a signature refused while the clock is off, a full disk, this
                    // device replaced): the whole send fails now, rather than leave this member out for good.
                    Err(error) if not_now(&error) => return Err(error),
                    // A verified member's new key, not confirmed yet: theirs is queued and waits in the outbox until
                    // it is (as a 1:1 message would), not left out for good.
                    Err(Error::Unconfirmed) => {}
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
    /// The one path of every message (1:1 through `send_message`, a group's, a group's changes): a 1:1 copy names the
    /// user it went to (spec 4.4), a group's names its group inside.
    pub(crate) fn queue(&mut self, users: &[String], group: Option<GroupTag>, content: Content) -> Result<()> {
        // A photo or video already sent to a member (an app that lost its answer and sends it again) goes only to the
        // members it did not reach yet; to none, if it reached them all (spec 4.7, 5).
        let users: Vec<String> = match &content {
            Content::Photo(media) | Content::Video { media, .. } => {
                let in_group = group.as_ref().map(|g| &g.id);
                users.iter().filter(|user| !self.sent_media(user, &media.id, in_group)).cloned().collect()
            }
            _ => users.to_vec(),
        };
        if users.is_empty() {
            return Ok(());
        }
        let sent_at_ms = self.wall().max(self.state.last_sent_ms + 1);
        self.state.last_sent_ms = sent_at_ms;
        let keep = shown(&content);
        let message = Message { sent_at_ms, group, content }.encode()?;
        for user in &users {
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
            let to = match (&group, users.as_slice()) {
                (None, [user]) => Some(user_bytes(user)?),
                _ => None,
            };
            self.enqueue(&me, encode(&Plain { to, content: message.into() }));
        }
        Ok(())
    }

    /// Makes revision + 1 of a group, as `edit` makes it of the group as it is once the inbox is taken in (a change
    /// that came meanwhile is built on, not overwritten), tells everyone in it before and after, and keeps it.
    /// `creating`: a new group (by this account), revision 1. The last member leaving keeps it to itself.
    fn change_group(
        &mut self,
        id: &str,
        creating: bool,
        edit: impl FnOnce(&Group) -> Result<(Vec<String>, String)>,
    ) -> Result<()> {
        let me = self.state.user_id.clone();
        // A change is built on what came meanwhile (spec 4.9): without the inbox read now, not at all (a new group has
        // nothing to come, and is made offline too).
        match self.deliver().and_then(|()| self.receive_all(0)) {
            Ok(()) => {}
            Err(error) if creating && retryable(&error) => {}
            Err(error) => return Err(error),
        }
        let current = match creating {
            true => Group {
                name: String::new(),
                members: Vec::new(),
                revision: 0,
                by: me.clone(),
                left: false,
                before: Vec::new(),
                accepted: true,
                synced: Default::default(),
                mine: None,
            },
            false => self.group(id)?,
        };
        let (members, name) = edit(&current)?;
        self.make_revision(id, current, members, name, creating, !creating)?;
        match self.deliver() {
            Ok(()) | Err(Error::Network(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Revision + 1 of `current` with `members` and `name`, by this account: queued to everyone in it before and
    /// after, and kept (`record`: as this account's change, made again should a rival at its revision win).
    fn make_revision(
        &mut self,
        id: &str,
        current: Group,
        mut members: Vec<String>,
        name: String,
        creating: bool,
        record: bool,
    ) -> Result<()> {
        let me = self.state.user_id.clone();
        members.sort();
        members.dedup();
        if members.len() > MAX_GROUP {
            return Err(Error::Protocol("a group has 20 members at most"));
        }
        let revision = current
            .revision
            .checked_add(1)
            .filter(|r| *r < u32::MAX)
            .ok_or(Error::Protocol("a group's last revision"))?;
        let left = !members.contains(&me);
        if left && members.is_empty() {
            // The last one out: nobody to tell, the group simply ends here.
            self.atomic(|c| {
                if let Some(g) = c.state.groups.get_mut(id) {
                    g.left = true;
                }
                c.push_event(Event::GroupChanged { group: id.to_owned() });
                c.persist()
            })?;
            return Ok(());
        }
        let ids: Vec<UserId> = members.iter().map(|m| user_bytes(m)).collect::<Result<_>>()?;
        let info = GroupInfo { id: group_id(id)?, revision, by: user_bytes(&me)?, name: name.clone(), members: ids };
        // Checked as it goes on the wire: the name and the members as they may be.
        Message { sent_at_ms: 0, group: None, content: Content::Group(info.clone()) }.encode()?;
        let mut told: Vec<String> = current.members.iter().chain(members.iter()).cloned().collect();
        told.sort();
        told.dedup();
        let reached = self.reach_all(&told)?;
        self.send_profiles();
        // A new group counted again after the inbox was read: one that came meanwhile may have reached the bound.
        if creating && self.state.groups.len() >= MAX_GROUPS {
            return Err(Error::Protocol("too many groups"));
        }
        let mine = record.then(|| Mine {
            revision,
            add: members.iter().filter(|m| !current.members.contains(m)).cloned().collect(),
            remove: current.members.iter().filter(|m| !members.contains(m)).cloned().collect(),
            name: (name != current.name).then(|| name.clone()),
            due: false,
        });
        self.atomic(|c| {
            c.queue(&reached, None, Content::Group(info))?;
            let group = Group {
                name,
                members,
                revision,
                by: me.clone(),
                left,
                before: current.members.clone(),
                accepted: true,
                synced: Default::default(),
                mine,
            };
            // Left (a request said no to among them): its members get nothing more, the key neither.
            if !left {
                c.contact_key_due(&group.members);
            }
            c.state.groups.insert(id.to_owned(), group);
            c.push_event(Event::GroupChanged { group: id.to_owned() });
            c.persist()
        })
    }

    /// This account's changes that lost to a rival at their revision, made again on top of the winner as the next
    /// revision, once each; nothing if the winner already has them (spec 4.9).
    pub(crate) fn reapply_groups(&mut self) -> Result<()> {
        let due: Vec<(String, Mine)> = (self.state.groups.iter())
            .filter_map(|(id, g)| g.mine.clone().filter(|m| m.due && !g.left).map(|m| (id.clone(), m)))
            .collect();
        for (id, mine) in due {
            let Some(group) = self.state.groups.get_mut(&id) else { continue };
            group.mine = None;
            let current = group.clone();
            let mut members: Vec<String> =
                (current.members.iter()).filter(|m| !mine.remove.contains(m)).chain(mine.add.iter()).cloned().collect();
            members.sort();
            members.dedup();
            let name = mine.name.clone().unwrap_or_else(|| current.name.clone());
            if members == current.members && name == current.name {
                continue;
            }
            match self.make_revision(&id, current, members, name, false, false) {
                Ok(()) => {}
                Err(error) if not_now(&error) => {
                    // Not now: still due, for the next call.
                    if let Some(group) = self.state.groups.get_mut(&id) {
                        group.mine = Some(mine);
                    }
                    return Err(error);
                }
                Err(error) => self.note(format!("group: {error}")),
            }
        }
        Ok(())
    }

    /// `from`, a member, has this revision of the group otherwise: it gets the group as it is here, once per
    /// revision, and the rule of rivals (spec 4.9) makes both the same.
    fn diverged(&mut self, from: &str, id: &str) {
        let Some(group) = self.state.groups.get_mut(id) else { return };
        if group.synced.insert(from.to_owned()) {
            self.group_asked(from, id);
        }
    }

    /// A group as `from` (or, `own`, this account's other device) has it: taken if it is newer and from a member.
    pub(crate) fn group_update(&mut self, from: &str, info: GroupInfo, own: bool) {
        let id = hex(&info.id);
        let me = self.state.user_id.clone();
        let members: Vec<String> = info.members.iter().map(|m| hex(m)).collect();
        // The last revision there is would leave no room for a change after it; 0 is no change (a group starts at 1).
        if info.revision == u32::MAX || info.revision == 0 {
            self.note("a group change at no revision".into());
            return;
        }
        // The tie-break is by the revision's author, whoever passes it on (an answer to an ask): a member's word, which
        // cannot be checked against revisions skipped here.
        let author = hex(&info.by);
        let theirs = group_hash(&info.id, &info.name, &info.members);
        let ours = self.state.groups.get(&id).and_then(|g| hash_of(&info.id, g).ok());
        let newer = match self.state.groups.get(&id) {
            // New here, or known only by its Yos: from a member of it as it is now.
            None => own || members.iter().any(|m| m == from),
            Some(g) if g.revision == 0 => own || members.iter().any(|m| m == from),
            // A newer revision from a member as it is now; a rival at the same revision from a member of the revision
            // both were made from (two removing each other at once: each is still heard by the other).
            // Or from one asked for it (their Yo named this newer revision: added since) who is in it.
            // A newer revision only from a member as it is here: one not in it (removed, or added since) is never
            // its own witness; its Yo makes this device ask a member instead (`group_yo`).
            Some(g) if info.revision > g.revision => own || g.members.iter().any(|m| m == from),
            // A rival at the same revision: from a member of the revision both were made from.
            Some(g) => {
                let base = if g.before.is_empty() { &g.members } else { &g.before };
                // Of one author's two (never so between honest ends): the lower hash, so both still end the same.
                let wins = author < g.by || (author == g.by && Some(theirs) < ours);
                info.revision == g.revision && wins && (own || base.iter().any(|m| m == from))
            }
        };
        if !newer {
            // A member behind: it gets the group as it is here.
            let behind =
                self.state.groups.get(&id).is_some_and(|g| {
                    !own && !g.left && info.revision < g.revision && g.members.iter().any(|m| m == from)
                });
            // A member's rival that lost here: it gets the winner, as one behind would.
            let rival =
                self.state.groups.get(&id).is_some_and(|g| {
                    !own && !g.left && info.revision == g.revision && g.members.iter().any(|m| m == from)
                }) && ours != Some(theirs);
            if behind {
                self.group_asked(from, &id);
            } else if rival {
                self.diverged(from, &id);
            } else {
                self.note("a group change not taken".into());
            }
            return;
        }
        // A sender cannot fill this device with groups (spec 4.9): past `MAX_GROUPS` a group not here yet is not taken
        // in, by an update as by a Yo.
        if !self.state.groups.contains_key(&id) && self.state.groups.len() >= MAX_GROUPS {
            self.note("a group past the most kept".into());
            return;
        }
        let left = !members.contains(&me);
        // What it was made from: a rival at the same revision shares it; a newer one, made from what this device had,
        // as far as it knows.
        let before = match self.state.groups.get(&id) {
            Some(g) if g.revision == info.revision => g.before.clone(),
            Some(g) => g.members.clone(),
            None => Vec::new(),
        };
        // Joined only from an accepted contact (or by this account); else a request until said yes to (spec 4.10).
        let accepted = own
            || self.state.groups.get(&id).is_some_and(|g| g.accepted)
            || self.state.contacts.get(from).is_some_and(|c| c.accepted);
        // This account's own change, beaten by a rival at its revision: made again on the winner after the sync.
        let mine = match self.state.groups.get(&id) {
            Some(g) if !own && g.revision == info.revision => {
                g.mine.clone().filter(|m| m.revision == g.revision).map(|m| Mine { due: true, ..m })
            }
            _ => None,
        };
        let group = Group {
            name: info.name,
            members,
            revision: info.revision,
            by: author,
            left,
            before,
            accepted,
            synced: Default::default(),
            mine,
        };
        self.state.group_asks.retain(|(g, _)| *g != id); // answered: an ask is no lasting exception
        let members = group.members.clone();
        let joined = group.accepted && !group.left;
        self.state.groups.insert(id.clone(), group);
        // Its members get this account's contact key, so their Yos to the group come as a contact's (spec 4.10).
        if joined {
            self.contact_key_due(&members);
        }
        self.push_event(Event::GroupChanged { group: id.clone() });
        // Yos held from one not in the group then: shown if this change has them in it, else dropped; never one of a
        // sender blocked meanwhile, nor in a group this account is no longer in.
        let (theirs, rest): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.state.group_held).into_iter().partition(|(g, _)| *g == id);
        self.state.group_held = rest;
        for (_, event) in theirs {
            let shown = !left
                && matches!(&event, crate::state::StoredEvent::GroupMessage { user, .. }
                    if members.contains(user) && !self.blocked(user));
            if shown {
                self.state.events.push(event);
            }
        }
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
        let Ok(by) = user_bytes(&group.by) else { return };
        let info = GroupInfo { id: gid, revision: group.revision, by, name: group.name, members: ids };
        if let Err(error) = self.queue(&[from.to_owned()], None, Content::Group(info)) {
            self.note(format!("group: {error}"));
        }
    }

    /// A Yo from `from` to a group: shown if they are in it; a group not known yet, or a newer revision of it, is asked
    /// of them.
    pub(crate) fn group_yo(&mut self, from: &str, tag: GroupTag, sent_at_ms: u64, content: Content) {
        let id = hex(&tag.id);
        let me = self.state.user_id.clone();
        // One not in the group as it is here, naming a newer revision (added since, or removed and claiming it): its
        // Yo is not shown until a member's change brings it in (spec 4.9); the group is asked of a member.
        let mut outsider = false;
        let ask = match self.state.groups.get(&id) {
            // A sender cannot fill this device with groups: past `MAX_GROUPS` a group not known is not taken in.
            None if self.state.groups.len() >= MAX_GROUPS => {
                self.note("a Yo to a group not known, too many groups".into());
                return;
            }
            None => {
                // Known by its Yo only: shown at once, the group itself asked for.
                let mut members = vec![from.to_owned(), me.clone()];
                members.sort();
                let group = Group {
                    name: String::new(),
                    members,
                    revision: 0,
                    by: from.to_owned(),
                    left: false,
                    before: Vec::new(),
                    accepted: self.state.contacts.get(from).is_some_and(|c| c.accepted),
                    synced: Default::default(),
                    mine: None,
                };
                self.state.groups.insert(id.clone(), group);
                self.push_event(Event::GroupChanged { group: id.clone() });
                true
            }
            // To a group left: dropped whatever revision it names (only a change adding this account back brings
            // it back). From no member of it as it is here: dropped; with a newer revision the group is asked of a
            // member too (a member added since: its next Yos show once the change is here).
            Some(g)
                if g.left || (tag.revision <= g.revision && g.revision > 0 && !g.members.iter().any(|m| m == from)) =>
            {
                self.note("a Yo to a group from no member".into());
                return;
            }
            Some(g) => {
                let (behind, ask) = (tag.revision < g.revision, tag.revision > g.revision);
                outsider = g.revision > 0 && !g.members.iter().any(|m| m == from);
                // The same revision, other members or name (a change lost): the sender gets the group as it is here.
                let diverged = tag.revision == g.revision
                    && g.revision > 0
                    && !outsider
                    && hash_of(&tag.id, g).is_ok_and(|ours| ours != tag.hash);
                // A member behind (its Yo names an older revision): it gets the group as it is here.
                if behind {
                    self.group_asked(from, &id);
                } else if diverged {
                    self.diverged(from, &id);
                }
                ask
            }
        };
        if ask {
            // Whom to ask: for a group known here, a member of it as it is here (its answer counts; the sender, not in
            // it here, could not vouch for its own addition); for a group not known, the sender.
            let asked = match self.state.groups.get(&id) {
                Some(g) if g.revision > 0 => g.members.iter().filter(|m| **m != me).min().cloned(),
                _ => Some(from.to_owned()),
            };
            // Asked once while unanswered: the outbox brings the ask there; every further Yo naming the revision
            // asks nothing more.
            // Not of one whose 1:1 request waits unanswered, for a group not joined: that would answer it.
            let joined = self.state.groups.get(&id).is_some_and(|g| g.accepted);
            let request = self.state.contacts.get(from).is_some_and(|c| !c.accepted && c.chat);
            let asked = asked.filter(|_| joined || !request || self.in_joined_group(from));
            if let Some(asked) = asked {
                if self.state.group_asks.len() < MAX_GROUPS && self.state.group_asks.insert((id.clone(), asked.clone()))
                {
                    if let Err(error) = self.queue(&[asked], None, Content::GroupAsk { id: tag.id }) {
                        self.note(format!("group: {error}"));
                    }
                }
            }
        }
        // A receipt only in a group joined, or to a contact: a request gets nothing back (spec 4.10).
        let joined = self.state.groups.get(&id).is_some_and(|g| g.accepted);
        if let Some(contact) = self.state.contacts.get_mut(from).filter(|c| joined || c.accepted) {
            contact.receipt_due = true;
        }
        let content = self.forget_keys(content);
        let event = Event::GroupMessage { group: id.clone(), user: from.to_owned(), sent_at_ms, content };
        if outsider {
            // Held until a member's change says whether its sender is in (`group_update`).
            if self.state.group_held.len() < MAX_HELD {
                self.state.group_held.push((id, crate::stored(event)));
            } else {
                self.note("a Yo to a group from one not in it yet, too many held".into());
            }
            return;
        }
        self.push_event(event);
    }
}

impl Client {
    /// This account's contact key is to go to these users (all but itself), as members of a group it is in.
    pub(crate) fn contact_key_due(&mut self, users: &[String]) {
        for user in users.iter().filter(|u| **u != self.state.user_id) {
            let contact = self.state.contacts.entry(user.clone()).or_insert_with(crate::state::Contact::unknown);
            // Once per key: a group's every change need not send it again.
            if !contact.key_given {
                contact.key_due = true;
            }
        }
    }
}
