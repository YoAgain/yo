//! Groups (spec 3.4): a small part of Signal's Groups v2 with zkgroup. The hub keeps each group encrypted under its
//! master key with its revision and checks every request by zero-knowledge proof; members hold the key. A change is
//! one revision on (of two at once, the loser is made again on the winner). A Yo goes to each member over the
//! sessions, endorsed where the sender lacks a member's profile key, and names the revision so a member behind fetches.

use crate::state::{hex, unhex, Group};
use crate::{expect, refused, retryable, shown, user_bytes, Client, Content, Error, Event, Result};
use libsignal_protocol::{Aci, Pni};
use yoip::net::api::{self, status, GroupAction, GroupAnswer, NewMember};
use yoip::net::limits::MAX_BLOB_FETCHES;
use yoip::protocol::content::{GroupKey, GroupTag, MAX_GROUP, MAX_NAME_LEN};
use yoip::{decode, encode};
use zkgroup::auth::{AuthCredentialWithPni, AuthCredentialWithPniZkc, AuthCredentialWithPniZkcResponse};
use zkgroup::groups::{
    GroupMasterKey, GroupSecretParams, GroupSendEndorsement, GroupSendEndorsementsResponse, UuidCiphertext,
};
use zkgroup::profiles::{ExpiringProfileKeyCredentialResponse, ProfileKey};
use zkgroup::{ServerPublicParams, Timestamp, SECONDS_PER_DAY};

/// A group as the app shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupView {
    /// Its id (hex): its identifier at the hub.
    pub id: String,
    /// Empty while it is not fetched yet.
    pub name: String,
    /// Its members by user id (hex), this account among them while it is in.
    pub members: Vec<String>,
    pub revision: u32,
    /// This account is not in it (any more).
    pub left: bool,
    /// Joined: else someone not yet a contact added this account to it, a request (spec 3.3).
    pub accepted: bool,
}

/// A refusal for now only: `retryable`, a refused signature (`401`: clock off, a replay), a failed save, replaced.
fn not_now(error: &Error) -> bool {
    retryable(error) || matches!(error, Error::Refused(401) | Error::Save(_) | Error::Replaced)
}

/// Groups this device keeps at most: a sender naming new group keys cannot fill it.
pub(crate) const MAX_GROUPS: usize = 256;
/// Group Yos held at once while their group is fetched (`State::group_held`).
const MAX_HELD: usize = 64;
/// How often a change is made again on what the hub has meanwhile (`409`) before it gives up.
const CHANGE_TRIES: usize = 4;
/// Endorsements are used until this long before they expire, then fetched anew.
const ENDORSED_MARGIN_S: u64 = 3600;
/// The own key into the groups after the hub refused its credential: a pause this long first, doubled each time.
const KEY_PAUSE_FIRST_MS: u64 = 60_000;
/// The longest pause.
const KEY_PAUSE_MAX_MS: u64 = 3_600_000;
/// The own credential from the hub that does not verify: paused as a refused one.
const UNVERIFIED: Error = Error::Protocol("the own profile key credential does not verify");

fn secret(key: &GroupKey) -> GroupSecretParams {
    GroupSecretParams::derive_from_master_key(GroupMasterKey::new(*key))
}

/// A group's id (hex): its identifier at the hub, from its key.
pub(crate) fn group_id_of(key: &GroupKey) -> String {
    hex(&secret(key).get_group_identifier())
}

fn key_of(group: &Group) -> Result<GroupKey> {
    unhex(&group.key).and_then(|k| k.try_into().ok()).ok_or(Error::Internal("a group key"))
}

fn aci(user: &str) -> Result<Aci> {
    Ok(Aci::from_uuid_bytes(user_bytes(user)?))
}

fn bad_answer() -> Error {
    Error::Protocol("a group answer")
}

/// The test hubs' public params (`api::TEST_ZK_SEED`), which test clients start with.
pub(crate) fn test_zk() -> Option<ServerPublicParams> {
    #[cfg(feature = "test-support")]
    {
        static PARAMS: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        let bytes = PARAMS.get_or_init(|| {
            zkgroup::serialize(&zkgroup::ServerSecretParams::generate(api::TEST_ZK_SEED).get_public_params())
        });
        zkgroup::deserialize(bytes).ok()
    }
    #[cfg(not(feature = "test-support"))]
    None
}

/// What a change is to do, made again on the group as the hub has it after a `409`.
enum Intent {
    Add(Vec<String>),
    Remove(Vec<String>),
    Rename(String),
    /// This account's own profile key into its entry (a yes to a request).
    Key,
    /// Entries no member can open out (`Group::unreadable`).
    Purge,
}

impl Client {
    /// Pins the hub's zkgroup `ServerPublicParams` (spec 3.4); without them no group is made, changed or fetched.
    pub fn set_zk_params(&mut self, bytes: &[u8]) -> Result<()> {
        self.zk = Some(zkgroup::deserialize(bytes).map_err(|_| Error::Protocol("not ServerPublicParams"))?);
        Ok(())
    }

    fn zk(&self) -> Result<ServerPublicParams> {
        self.zk.clone().ok_or(Error::Protocol("no group params pinned for this hub"))
    }

    /// The groups this account is or was in (fetched at least once); with requests off (spec 3.3) none a stranger
    /// added it to and it said no yes to.
    pub fn groups(&self) -> Vec<GroupView> {
        self.state
            .groups
            .iter()
            .filter(|(_, g)| g.revision > 0 && (g.accepted || self.state.requests_open))
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

    /// Makes a group of this account and `members` (user ids) at the hub and tells them; returns its id.
    pub fn create_group(&mut self, name: &str, members: &[String]) -> Result<(String, Vec<Event>)> {
        let result = self.create_group_inner(name, members);
        self.finish_with(result)
    }

    fn create_group_inner(&mut self, name: &str, members: &[String]) -> Result<String> {
        let zk = self.zk()?;
        let me = self.state.user_id.clone();
        let mut others: Vec<String> = members.iter().map(|m| m.to_ascii_lowercase()).filter(|m| *m != me).collect();
        others.sort();
        others.dedup();
        for user in &others {
            user_bytes(user)?;
        }
        if others.len() + 1 > MAX_GROUP {
            return Err(Error::Protocol("a group has 20 members at most"));
        }
        if self.state.groups.len() >= MAX_GROUPS {
            return Err(Error::Protocol("too many groups"));
        }
        // The hub has this account's commitment (with its access key): its own entry goes in with its key.
        match self.profile_keys() {
            Err(error) if !retryable(&error) => return Err(error),
            _ => {}
        }
        let key: GroupKey = crate::identity::random();
        let secret = secret(&key);
        let id = secret.get_group_identifier();
        let auth = self.auth_presentation(&zk, &secret)?;
        let mut new = vec![self.new_member(&zk, &secret, &me)?];
        for user in &others {
            new.push(self.new_member(&zk, &secret, user)?);
        }
        let body = encode(&api::NewGroup {
            public_params: zkgroup::serialize(&secret.get_public_params()).into(),
            auth: auth.into(),
            title: title(&secret, name)?.into(),
            members: new,
        });
        let response = expect(self.zk_request("PUT", &api::group_path(&id), &body)?, status::OK)?;
        let answer: GroupAnswer = decode(&response.body).map_err(|_| bad_answer())?;
        let id = hex(&id);
        self.state.groups.insert(id.clone(), Group { key: hex(&key), accepted: true, ..Default::default() });
        self.take_answer(&id, answer)?;
        let members = self.state.groups[&id].members.clone();
        self.tell(&id, &members)?;
        Ok(id)
    }

    pub fn rename_group(&mut self, id: &str, name: &str) -> Result<Vec<Event>> {
        let result = self.change(id, Intent::Rename(name.to_owned()));
        self.finish(result)
    }

    pub fn add_to_group(&mut self, id: &str, users: &[String]) -> Result<Vec<Event>> {
        let users = users.iter().map(|u| u.to_ascii_lowercase()).collect();
        let result = self.change(id, Intent::Add(users));
        self.finish(result)
    }

    pub fn remove_from_group(&mut self, id: &str, users: &[String]) -> Result<Vec<Event>> {
        let users = users.iter().map(|u| u.to_ascii_lowercase()).collect();
        let result = self.change(id, Intent::Remove(users));
        self.finish(result)
    }

    /// Leaves a group: the others are told, this account gets nothing of it any more.
    pub fn leave_group(&mut self, id: &str) -> Result<Vec<Event>> {
        let me = self.state.user_id.clone();
        let result = self.change(id, Intent::Remove(vec![me]));
        self.finish(result)
    }

    /// Says yes to a group a stranger added this account to: its profile key goes to the hub entry and the members.
    pub fn accept_group(&mut self, id: &str) -> Result<Vec<Event>> {
        let result = (|| {
            let group = self.state.groups.get_mut(id).ok_or(Error::Protocol("no such group"))?;
            group.accepted = true;
            let members = group.members.clone();
            self.profile_key_due(&members);
            // Joined in the account's storage too, saved with it.
            self.store_group(id);
            self.persist()?;
            // The hub has the commitment first, so the own entry's key can go in, at once.
            self.profile_keys()?;
            self.key_pause = (0, 0);
            self.key_groups()?;
            self.deliver()
        })();
        self.finish(result)
    }

    /// A Yo (with a photo or video or not) to a group this account is in.
    pub fn send_group(&mut self, id: &str, content: Content) -> Result<Vec<Event>> {
        let result = self.send_group_inner(id, content);
        self.finish_sent(result)
    }

    /// `send_group` split around its network step, as `send_begin`: with the group's endorsements fresh and a session
    /// with every member, the Yo is queued and saved and the deliveries are handed out. `None`, with nothing changed:
    /// `send_group` instead.
    pub fn send_group_begin(&mut self, id: &str, content: Content) -> Option<crate::Deliveries> {
        if self.replaced.get() {
            return None;
        }
        if !shown(&content) || !matches!(content, Content::Yo | Content::Photo(_) | Content::Video { .. }) {
            return None;
        }
        let group = self.group(id).ok()?;
        if group.endorsed_until < self.now_s() + ENDORSED_MARGIN_S || self.not_empty(&group).is_err() {
            return None;
        }
        let me = self.state.user_id.clone();
        let mut others = Vec::new();
        for member in group.members.iter().filter(|m| **m != me) {
            match self.state.contacts.get(member) {
                Some(contact) if contact.gone => continue,
                Some(contact) if !contact.devices.is_empty() && !self.no_session(member) => {}
                // A session to start first (`reach_all`, which asks the hub): `send_group`.
                _ => return None,
            }
            others.push(member.clone());
        }
        let tag = GroupTag { key: key_of(&group).ok()?, revision: group.revision };
        self.atomic(|c| {
            c.send_profiles();
            c.queue(&others, Some(tag), content)?;
            for user in &others {
                c.encrypt_waiting(user); // its envelope in the same save
            }
            c.persist()
        })
        .ok()?;
        Some(self.deliveries().unwrap_or_else(|_| self.no_deliveries()))
    }

    /// Fetches a group photo or video is uploaded for: one per intended device (`MAX_BLOB_FETCHES` at most).
    pub fn group_media_fetches(&self, id: &str) -> Option<u32> {
        let group = self.state.groups.get(id).filter(|g| !g.left && g.revision > 0)?;
        let others: usize = group
            .members
            .iter()
            .filter(|m| **m != self.state.user_id)
            .filter(|m| !self.state.contacts.get(*m).is_some_and(|c| c.gone))
            .map(|m| self.devices_of(m).len().max(1))
            .sum();
        Some((others as u32).clamp(1, MAX_BLOB_FETCHES))
    }

    /// Opens the sessions a group's photo or video needs and says for how many fetches to upload it.
    pub fn open_group(&mut self, id: &str) -> Result<(u32, Vec<Event>)> {
        let result = self.group(id).and_then(|g| {
            self.not_empty(&g)?;
            self.reach_all(&g.members)?;
            Ok(self.group_media_fetches(id).unwrap_or(1))
        });
        self.finish_with(result)
    }

    /// For tests: how many deliveries the hub took on a group send endorsement alone (no profile key).
    pub fn endorsed_deliveries(&self) -> u64 {
        self.endorsed_sent
    }

    // ---- inside ----

    fn group(&self, id: &str) -> Result<Group> {
        self.state
            .groups
            .get(id)
            .filter(|g| !g.left && g.revision > 0)
            .cloned()
            .ok_or(Error::Protocol("not in this group"))
    }

    fn send_group_inner(&mut self, id: &str, content: Content) -> Result<()> {
        if self.replaced.get() {
            return Err(Error::Replaced);
        }
        if !shown(&content) || !matches!(content, Content::Yo | Content::Photo(_) | Content::Video { .. }) {
            return Err(Error::Protocol("not content for a group"));
        }
        let mut group = self.group(id)?;
        // Endorsements for the members, fresh: who has no profile key of theirs here still gets it sealed.
        if group.endorsed_until < self.now_s() + ENDORSED_MARGIN_S {
            match self.fetch_group(id) {
                Err(error) if not_now(&error) => return Err(error),
                Err(error) => self.note(format!("group: {error}")),
                Ok(()) => {}
            }
            group = self.group(id)?;
        }
        self.not_empty(&group)?;
        let others = self.reach_all(&group.members)?;
        self.send_profiles();
        let tag = GroupTag { key: key_of(&group)?, revision: group.revision };
        self.atomic(|c| {
            c.queue(&others, Some(tag), content)?;
            c.persist()
        })?;
        self.deliver_now()
    }

    /// A group of this account alone takes no Yo (`GroupEmpty`): the app offers to leave it.
    fn not_empty(&self, group: &Group) -> Result<()> {
        match group.members.iter().any(|m| *m != self.state.user_id) {
            true => Ok(()),
            false => Err(Error::GroupEmpty),
        }
    }

    fn now_s(&self) -> u64 {
        self.wall() / 1000
    }

    /// A group request (spec 3.4): unsigned, on the connection that names no account (`api::anonymous`); its zkgroup
    /// proof is all that authorises it, so the hub never learns which account is in which group.
    fn zk_request(&self, method: &str, path: &str, body: &[u8]) -> Result<crate::Response> {
        debug_assert!(api::anonymous(method, path));
        self.unsigned(method, path, &[], body)
    }

    /// An auth credential presentation for this group (today's credential, fetched once a day).
    fn auth_presentation(&mut self, zk: &ServerPublicParams, secret: &GroupSecretParams) -> Result<Vec<u8>> {
        let now = self.now_s();
        let fresh = self.group_auth.as_ref().is_some_and(|(day, _)| now >= *day && now < day + SECONDS_PER_DAY);
        if !fresh {
            let response = expect(self.signed("GET", api::GROUP_AUTH, &[])?, status::OK)?;
            let answer: api::GroupAuthCredential = decode(&response.body).map_err(|_| bad_answer())?;
            let issued: AuthCredentialWithPniZkcResponse =
                zkgroup::deserialize(answer.credential.as_slice()).map_err(|_| bad_answer())?;
            let user = user_bytes(&self.state.user_id)?;
            let credential: AuthCredentialWithPniZkc = issued
                .receive(
                    Aci::from_uuid_bytes(user),
                    Pni::from_uuid_bytes(user),
                    Timestamp::from_epoch_seconds(answer.redemption_s),
                    zk,
                )
                .map_err(|_| Error::Protocol("an auth credential that does not verify"))?;
            self.group_auth = Some((answer.redemption_s, credential));
        }
        let (_, credential) = self.group_auth.clone().expect("fetched above");
        Ok(zkgroup::serialize(&AuthCredentialWithPni::Zkc(credential).present(zk, secret, crate::identity::random())))
    }

    /// A member to add: with a profile key credential presentation if their key is held here, else invited by id.
    fn new_member(&mut self, zk: &ServerPublicParams, secret: &GroupSecretParams, user: &str) -> Result<NewMember> {
        let user_aci = aci(user)?;
        let key = match user == self.state.user_id {
            true => self.state.profile_key,
            false => self.state.contacts.get(user).and_then(|c| c.profile_key),
        };
        let invited = || NewMember {
            user: zkgroup::serialize(&secret.encrypt_service_id(user_aci.into())).into(),
            presentation: Vec::new().into(),
        };
        let Some(key) = key else { return Ok(invited()) };
        match self.presentation(zk, secret, user, key) {
            Ok(Some(presentation)) => Ok(NewMember { user: Vec::new().into(), presentation: presentation.into() }),
            // No commitment told, or another key than the one held here: invited.
            Ok(None) | Err(Error::Refused(status::NOT_FOUND | status::FORBIDDEN)) => Ok(invited()),
            Err(error) => Err(error),
        }
    }

    /// A profile key credential presentation for `user` holding `key` (spec 3.4): `None` one that does not verify;
    /// `Refused(404)` the hub has no commitment of theirs, `Refused(403)` another key's.
    fn presentation(
        &mut self,
        zk: &ServerPublicParams,
        secret: &GroupSecretParams,
        user: &str,
        key: [u8; 32],
    ) -> Result<Option<Vec<u8>>> {
        let context = zk.create_profile_key_credential_request_context(
            crate::identity::random(),
            aci(user)?,
            ProfileKey::create(key),
        );
        let body = encode(&api::ProfileCredentialRequest {
            user_id: user_bytes(user)?,
            request: zkgroup::serialize(&context.get_request()).into(),
        });
        let response = expect(self.zk_request("POST", api::GROUP_CREDENTIAL, &body)?, status::OK)?;
        let issued: ExpiringProfileKeyCredentialResponse =
            zkgroup::deserialize(&response.body).map_err(|_| bad_answer())?;
        let now = Timestamp::from_epoch_seconds(self.now_s());
        let Ok(credential) = zk.receive_expiring_profile_key_credential(&context, &issued, now) else {
            self.note("a profile key credential that does not verify".into());
            return Ok(None);
        };
        let presentation =
            zk.create_expiring_profile_key_credential_presentation(crate::identity::random(), *secret, credential);
        Ok(Some(zkgroup::serialize(&presentation)))
    }

    /// The group as the hub answered it, taken in: members and title decrypted, endorsements checked.
    fn take_answer(&mut self, id: &str, answer: GroupAnswer) -> Result<()> {
        let zk = self.zk()?;
        let me = self.state.user_id.clone();
        let old = self.state.groups.get(id).cloned().ok_or(Error::Internal("a group"))?;
        let secret = secret(&key_of(&old)?);
        let decrypt = |bytes: &[u8]| -> Option<String> {
            let ciphertext: UuidCiphertext = zkgroup::deserialize(bytes).ok()?;
            let service = secret.decrypt_service_id(ciphertext).ok()?;
            let aci = Aci::try_from(service).ok()?;
            Some(hex(&aci.service_id_binary()))
        };
        // Each entry's user; one no member can open (a member added another group's ciphertext, the hub cannot tell)
        // is skipped, kept by its bytes to be removed (`purge`), and the group stays usable for everyone.
        let users: Vec<Option<String>> = answer.state.members.iter().map(|m| decrypt(m.user.as_slice())).collect();
        let mut members = Vec::new();
        let mut unreadable = Vec::new();
        let (mut keyed, mut added_by) = (false, String::new());
        for (member, user) in answer.state.members.iter().zip(&users) {
            let Some(user) = user.clone() else {
                unreadable.push(hex(member.user.as_slice()));
                continue;
            };
            if user == me {
                keyed = !member.profile_key.as_slice().is_empty();
                added_by = decrypt(member.added_by.as_slice()).unwrap_or_default();
            }
            members.push(user);
        }
        if !unreadable.is_empty() {
            self.note(format!("group: {} entries no member can open", unreadable.len()));
        }
        // Its name as a name may be (spec 3.4: 1 to 32 bytes, no control characters), else none.
        let name = secret
            .decrypt_blob_with_padding(answer.state.title.as_slice())
            .ok()
            .and_then(|t| String::from_utf8(t).ok())
            .filter(|name| name.len() <= MAX_NAME_LEN && !name.chars().any(char::is_control))
            .unwrap_or_default();
        // Endorsements (spec 3.4): for the members with their key in, as Signal's full members; a hub of before
        // endorsed every member. Each is checked against the entries it is for, and kept by their user.
        let mut endorsements = std::collections::BTreeMap::new();
        let mut endorsed_until = 0;
        if !answer.endorsements.as_slice().is_empty() {
            let entries: Vec<(Option<String>, UuidCiphertext, bool)> = answer
                .state
                .members
                .iter()
                .zip(&users)
                .filter_map(|(member, user)| {
                    let ciphertext = zkgroup::deserialize(member.user.as_slice()).ok()?;
                    Some((user.clone(), ciphertext, !member.profile_key.as_slice().is_empty()))
                })
                .collect();
            let now = Timestamp::from_epoch_seconds(self.now_s());
            let received = zkgroup::deserialize::<GroupSendEndorsementsResponse>(answer.endorsements.as_slice())
                .ok()
                .and_then(|response| {
                    let until = response.expiration().epoch_seconds();
                    let receive = |keyed_only: bool| {
                        let users: Vec<&(Option<String>, UuidCiphertext, bool)> =
                            entries.iter().filter(|(_, _, keyed)| *keyed || !keyed_only).collect();
                        let received = response
                            .clone()
                            .receive_with_ciphertexts(users.iter().map(|(_, c, _)| *c), now, &zk)
                            .ok()?;
                        Some(users.into_iter().map(|(user, _, _)| user.clone()).zip(received).collect::<Vec<_>>())
                    };
                    receive(true).or_else(|| receive(false)).map(|r| (until, r))
                });
            match received {
                Some((until, received)) => {
                    endorsed_until = until;
                    for (member, endorsement) in received {
                        if let Some(member) = member {
                            endorsements.insert(member, hex(&zkgroup::serialize(&endorsement.decompressed)));
                        }
                    }
                }
                None => self.note("group send endorsements that do not verify".into()),
            }
        }
        members.sort();
        let left = !members.contains(&me);
        // Joined (spec 3.3): once said yes to, until this account is out (leaving says no; added back by a stranger it
        // is a request again), or with its key in its entry, or added by itself or an accepted contact.
        let accepted = (old.accepted && !old.left && !left)
            || keyed
            || added_by == me
            || self.state.contacts.get(&added_by).is_some_and(|c| c.accepted && !c.blocked);
        let revision = answer.state.revision;
        let changed = old.revision != revision || old.members != members || old.name != name || old.left != left;
        let joined = accepted && !left;
        let group = Group {
            key: old.key,
            name,
            members: members.clone(),
            revision,
            left,
            accepted,
            keyed,
            // What the hub has now: a tag naming more than there is never makes the group fetched again and again.
            named: revision,
            unreadable: if left { Vec::new() } else { unreadable },
            endorsed_until: if left { 0 } else { endorsed_until },
            endorsements: if left { Default::default() } else { endorsements },
        };
        self.state.groups.insert(id.to_owned(), group);
        // Its key into the account's storage, as Signal's: a new phone of this account fetches it at once.
        self.store_group(id);
        // Its members get this account's profile key, so their Yos to the group come as a contact's (spec 3.3).
        if joined {
            self.profile_key_due(&members);
        }
        // A group request with requests off is not shown (`groups`): nothing to tell the app of either.
        if changed && (accepted || self.state.requests_open) {
            self.push_event(Event::GroupChanged { group: id.to_owned() });
        }
        self.release_held(id);
        self.persist()
    }

    /// Yos held for this group: shown if the fetched group has their sender in it, else dropped; never a blocked one's,
    /// nor with requests off a stranger's in a group not joined (spec 3.3).
    fn release_held(&mut self, id: &str) {
        let (theirs, rest): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.state.group_held).into_iter().partition(|(g, _)| g == id);
        self.state.group_held = rest;
        let Some(group) = self.state.groups.get(id).cloned() else { return };
        for (_, event) in theirs {
            let shown = !group.left
                && match &event {
                    Event::GroupMessage { user, .. } => {
                        group.members.contains(user)
                            && !self.blocked(user)
                            && (group.accepted || self.state.requests_open || !self.stranger(user))
                    }
                    _ => false,
                };
            if shown {
                self.push_event(event);
            }
        }
    }

    /// Fetches a group from the hub. No longer a member, or gone: left (one never fetched is forgotten).
    fn fetch_group(&mut self, id: &str) -> Result<()> {
        let zk = self.zk()?;
        let group = self.state.groups.get(id).cloned().ok_or(Error::Internal("a group"))?;
        let secret = secret(&key_of(&group)?);
        let auth = self.auth_presentation(&zk, &secret)?;
        let body = encode(&api::GroupRequest { auth: auth.into(), revision: 0, actions: Vec::new() });
        let response = self.zk_request("POST", &api::group_path(&secret.get_group_identifier()), &body)?;
        match response.status {
            status::OK => self.take_answer(id, decode(&response.body).map_err(|_| bad_answer())?),
            status::FORBIDDEN | status::NOT_FOUND => {
                self.out_of(id);
                self.unstore_group(id);
                self.persist()
            }
            _ => Err(refused(&response)),
        }
    }

    /// This account is not in the group (any more).
    fn out_of(&mut self, id: &str) {
        let Some(group) = self.state.groups.get_mut(id) else { return };
        if group.revision == 0 {
            self.state.groups.remove(id);
        } else {
            group.named = group.revision;
            group.accepted = false;
            group.endorsed_until = 0;
            group.endorsements.clear();
            group.unreadable.clear();
            if !std::mem::replace(&mut group.left, true) {
                self.push_event(Event::GroupChanged { group: id.to_owned() });
            }
        }
        self.state.group_held.retain(|(g, _)| g != id);
    }

    /// Every group a message named a newer revision of (or never fetched) is fetched now. Not now: later.
    pub(crate) fn refresh_groups(&mut self) -> Result<()> {
        // No group params pinned (yet): nothing can be fetched, nothing is given up (a restored group waits for them).
        if self.zk.is_none() {
            return Ok(());
        }
        let due: Vec<String> = (self.state.groups.iter())
            .filter(|(_, g)| g.revision == 0 || g.named > g.revision)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            match self.fetch_group(&id) {
                Ok(()) => {}
                Err(error) if not_now(&error) => return Err(error),
                Err(error) => {
                    self.note(format!("group: {error}"));
                    self.out_of(&id);
                }
            }
        }
        self.purge_groups()?;
        self.key_groups()
    }

    /// The own entry's profile key into each group joined and not keyed yet, once the hub has the commitment: tried
    /// again after each read until the hub's answer has it keyed (also on a new phone, the group from the storage).
    /// The hub without the commitment (`404`: told before hubs kept them, or lost) is told the access key with it
    /// again, and the key tried once more; refused still (or `403`, another key's), it waits a pause that doubles up
    /// to an hour, not the next read.
    fn key_groups(&mut self) -> Result<()> {
        if !self.state.access_key_told || self.monotonic() < self.key_pause.0 {
            return Ok(());
        }
        let due: Vec<String> = (self.state.groups.iter())
            .filter(|(_, g)| g.accepted && !g.keyed && !g.left && g.revision > 0)
            .map(|(id, _)| id.clone())
            .collect();
        let mut told_again = false;
        for id in &due {
            let mut result = self.change(id, Intent::Key);
            if matches!(result, Err(Error::Refused(status::NOT_FOUND))) && !told_again {
                told_again = true;
                self.note("group key: the hub has no commitment, told again".into());
                self.state.access_key_told = false;
                self.persist()?;
                // Its keys queued for contacts first (`deliver` false): then the hub is told in a later round.
                self.profile_keys_with(false)?;
                if !self.state.access_key_told {
                    return Ok(());
                }
                result = self.change(id, Intent::Key);
            }
            match result {
                Err(error) if not_now(&error) => return Err(error),
                // The own credential: the same for every group, none of the others gets it either.
                Err(error @ (Error::Refused(status::NOT_FOUND | status::FORBIDDEN) | UNVERIFIED)) => {
                    self.note(format!("group key: {error}"));
                    let pause = (self.key_pause.1 * 2).clamp(KEY_PAUSE_FIRST_MS, KEY_PAUSE_MAX_MS);
                    self.key_pause = (self.monotonic() + pause, pause);
                    return Ok(());
                }
                Err(error) => self.note(format!("group key: {error}")),
                Ok(()) => {}
            }
        }
        self.key_pause = (0, 0);
        Ok(())
    }

    /// Entries no member can open, removed from each group joined that has them (by their bytes: any member may
    /// remove any, spec 3.4); tried again after each read while they are there.
    fn purge_groups(&mut self) -> Result<()> {
        let due: Vec<String> = (self.state.groups.iter())
            .filter(|(_, g)| g.accepted && !g.left && g.revision > 0 && !g.unreadable.is_empty())
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            match self.change(&id, Intent::Purge) {
                Err(error) if not_now(&error) => return Err(error),
                Err(error) => self.note(format!("group purge: {error}")),
                Ok(()) => {}
            }
        }
        Ok(())
    }

    /// `refresh_groups`, whatever comes of it: noted, tried again after the next read.
    pub(crate) fn refresh_groups_quietly(&mut self) {
        if let Err(error) = self.refresh_groups() {
            self.note(format!("groups later: {error}"));
        }
    }

    /// A group a message from `from` names: known from now on, its revision noted (fetched after the inbox,
    /// `refresh_groups`). One not a member as fetched names at most the next revision (a member added since may write
    /// first): whatever it claims, it makes the group fetched once.
    pub(crate) fn group_named(&mut self, tag: GroupTag, from: &str) -> Option<String> {
        let id = group_id_of(&tag.key);
        let full = self.state.groups.len() >= MAX_GROUPS;
        match self.state.groups.get_mut(&id) {
            Some(group) => {
                let member = group.revision == 0 || group.members.iter().any(|m| m == from);
                let named = if member { tag.revision } else { tag.revision.min(group.revision.saturating_add(1)) };
                group.named = group.named.max(named);
            }
            // A sender cannot fill this device with groups.
            None if full => {
                self.note("a group past the most kept".into());
                return None;
            }
            None => {
                let group = Group { key: hex(&tag.key), named: tag.revision, ..Default::default() };
                self.state.groups.insert(id.clone(), group);
            }
        }
        Some(id)
    }

    /// A Yo from `from` to a group: shown if they are a member; held while the group is fetched.
    pub(crate) fn group_yo(&mut self, from: &str, tag: GroupTag, sent_at_ms: u64, content: Content) {
        let Some(id) = self.group_named(tag, from) else { return };
        let group = self.state.groups[&id].clone();
        let current = group.revision > 0 && group.revision >= tag.revision;
        if current && (group.left || !group.members.iter().any(|m| m == from)) {
            self.note("a Yo to a group from no member".into());
            return;
        }
        // A receipt only in a group joined, or to a contact: a request gets nothing back (spec 3.3).
        if let Some(contact) = self.state.contacts.get_mut(from).filter(|c| group.accepted || c.accepted) {
            contact.receipt_due = true;
        }
        let content = self.forget_keys(content);
        let event = Event::GroupMessage { group: id.clone(), user: from.to_owned(), sent_at_ms, content };
        self.show_or_hold(&id, current, event);
    }

    fn show_or_hold(&mut self, id: &str, current: bool, event: Event) {
        if current {
            self.push_event(event);
        } else if self.state.group_held.len() < MAX_HELD {
            self.state.group_held.push((id.to_owned(), event));
        } else {
            self.note("a group's Yo while its group is fetched, too many held".into());
        }
    }

    /// A change at the hub as one revision on; on a `409`, made again on the hub's group. Everyone in it before and
    /// after is told (type 8).
    fn change(&mut self, id: &str, intent: Intent) -> Result<()> {
        let zk = self.zk()?;
        let me = self.state.user_id.clone();
        if self.state.groups.get(id).is_some_and(|g| g.named > g.revision) {
            self.fetch_group(id)?;
        }
        for _ in 0..CHANGE_TRIES {
            let group = self.group(id)?;
            let secret = secret(&key_of(&group)?);
            let mut actions = Vec::new();
            let mut after = group.members.clone();
            // Entries no member can open go with any change of a member who has the group joined.
            if group.accepted {
                for bytes in group.unreadable.iter().filter_map(|b| unhex(b)) {
                    actions.push(GroupAction { kind: api::ACTION_REMOVE, data: bytes.into() });
                }
            }
            match &intent {
                Intent::Purge => {}
                Intent::Add(users) => {
                    for user in users.iter().filter(|u| !group.members.contains(u)) {
                        let member = self.new_member(&zk, &secret, user)?;
                        actions.push(GroupAction { kind: api::ACTION_ADD, data: encode(&member).into() });
                        after.push(user.clone());
                    }
                }
                Intent::Remove(users) => {
                    for user in users.iter().filter(|u| group.members.contains(u)) {
                        let ciphertext = secret.encrypt_service_id(aci(user)?.into());
                        actions.push(GroupAction {
                            kind: api::ACTION_REMOVE,
                            data: zkgroup::serialize(&ciphertext).into(),
                        });
                        after.retain(|m| m != user);
                    }
                }
                Intent::Rename(name) if *name != group.name => {
                    actions.push(GroupAction { kind: api::ACTION_TITLE, data: title(&secret, name)?.into() });
                }
                Intent::Rename(_) => {}
                // Refused (`404`, `403`): the caller's to handle (`key_groups`).
                Intent::Key if !group.keyed => {
                    let Some(key) = self.state.profile_key else { return Ok(()) };
                    let Some(presentation) = self.presentation(&zk, &secret, &me, key)? else { return Err(UNVERIFIED) };
                    actions.push(GroupAction { kind: api::ACTION_PROFILE_KEY, data: presentation.into() });
                }
                Intent::Key => {}
            }
            if actions.is_empty() {
                return Ok(());
            }
            if after.len() > MAX_GROUP {
                return Err(Error::Protocol("a group has 20 members at most"));
            }
            let auth = self.auth_presentation(&zk, &secret)?;
            let revision = group.revision.checked_add(1).ok_or(Error::Protocol("a group's last revision"))?;
            let body = encode(&api::GroupRequest { auth: auth.into(), revision, actions });
            let response = self.zk_request("POST", &api::group_path(&secret.get_group_identifier()), &body)?;
            match response.status {
                status::OK => {
                    self.take_answer(id, decode(&response.body).map_err(|_| bad_answer())?)?;
                    let mut told: Vec<String> = group.members.iter().chain(&after).cloned().collect();
                    told.sort();
                    told.dedup();
                    return self.tell(id, &told);
                }
                // Another change first: the group as it is now, and this one made again on it.
                status::CONFLICT => self.take_answer(id, decode(&response.body).map_err(|_| bad_answer())?)?,
                status::FORBIDDEN | status::NOT_FOUND => {
                    self.out_of(id);
                    self.unstore_group(id);
                    self.persist()?;
                    return Err(Error::Protocol("not in this group"));
                }
                _ => return Err(refused(&response)),
            }
        }
        Err(Error::Protocol("the group changed too often meanwhile"))
    }

    /// Tells `users` (all but this account) that the group is at its revision now.
    fn tell(&mut self, id: &str, users: &[String]) -> Result<()> {
        let group = self.state.groups.get(id).cloned().ok_or(Error::Internal("a group"))?;
        let tag = GroupTag { key: key_of(&group)?, revision: group.revision };
        let reached = self.reach_all(users)?;
        if !group.left {
            self.send_profiles();
        }
        self.atomic(|c| {
            c.queue(&reached, None, Content::Group(tag))?;
            c.persist()
        })?;
        match self.deliver() {
            Ok(()) | Err(Error::Network(_)) => Ok(()),
            Err(error) if retryable(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// A group send token for `user` from a shared group (spec 3.4), base64url: sealed without their profile key.
    pub(crate) fn group_send_token(&self, user: &str) -> Option<String> {
        let now = self.now_s();
        let group = self
            .state
            .groups
            .values()
            .find(|g| !g.left && g.endorsed_until > now + 60 && g.endorsements.contains_key(user))?;
        let compressed = unhex(group.endorsements.get(user)?)?;
        let endorsement: GroupSendEndorsement = zkgroup::deserialize(&compressed).ok()?;
        let token = endorsement.to_token(secret(&key_of(group).ok()?));
        let full = token.into_full_token(Timestamp::from_epoch_seconds(group.endorsed_until));
        Some(yoip::b64(&zkgroup::serialize(&full)))
    }

    /// The hub refused a delivery's group send token: the endorsements for `user` are fetched anew before the next.
    pub(crate) fn endorsement_refused(&mut self, user: &str) {
        for group in self.state.groups.values_mut().filter(|g| g.endorsements.contains_key(user)) {
            group.endorsed_until = 0;
        }
    }

    /// A session with each other member this device can reach (spec 3.1), without a chat of their own. One gone or
    /// without a device is left out (noted); one unreachable now fails the call.
    fn reach_all(&mut self, members: &[String]) -> Result<Vec<String>> {
        let mut reached = Vec::new();
        let me = self.state.user_id.clone();
        for member in members.iter().filter(|m| **m != me) {
            if self.state.contacts.get(member).is_some_and(|c| c.gone) {
                continue;
            }
            if !self.has_chat(member) {
                let chat = self.state.contacts.get(member).is_some_and(|c| c.chat);
                match self.start_chat(&yoip::net::handle::Handle::user(&user_bytes(member)?)) {
                    Ok(_) => {}
                    Err(error) if not_now(&error) => return Err(error),
                    // A verified member's new key, not confirmed yet: theirs waits in the outbox until it is.
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

    /// Queues the same content for each recipient.
    pub(crate) fn queue(&mut self, users: &[String], group: Option<GroupTag>, content: Content) -> Result<()> {
        // Media already sent to some members (a lost answer, sent again) goes only to the rest (spec 3.2, 3.2).
        let users: Vec<String> = match &content {
            Content::Photo(media) | Content::Video { media, .. } => {
                let in_group = group.as_ref().map(|g| &g.key);
                users.iter().filter(|user| !self.sent_media(user, &media.id, in_group)).cloned().collect()
            }
            _ => users.to_vec(),
        };
        if users.is_empty() {
            return Ok(());
        }
        let sent_at_ms = self.next_sent_at();
        let keep = shown(&content);
        // A Yo to accepted contacts or joined members carries the profile key (spec 3.3); never to a request.
        let accepted = users.iter().all(|user| {
            self.in_joined_group(user) || self.state.contacts.get(user).is_some_and(|c| c.accepted && !c.blocked)
        });
        let key = self.state.profile_key.filter(|_| accepted).map(|key| (key, self.state.profile_key_gen.max(1)));
        let message = yoip::protocol::content::Message { sent_at_ms, group, content }.encode_with_key(key)?;
        for user in &users {
            self.enqueue(user, encode(&yoip::protocol::envelope::Plain { to: None, content: message.clone().into() }));
            if keep {
                if let Some(contact) = self.state.contacts.get_mut(user) {
                    contact.sent.push_back(message.clone());
                    contact.sent_devices.insert(sent_at_ms, Vec::new());
                    while contact.sent.len() > crate::KEPT_FOR_RESEND {
                        if let Some(old) =
                            contact.sent.pop_front().and_then(|m| yoip::protocol::content::Message::decode(&m).ok())
                        {
                            contact.sent_devices.remove(&old.sent_at_ms);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether this account and `user` are both in the group a message names (one of no group: yes).
    pub(crate) fn both_in(&self, tag: Option<&GroupTag>, user: &str) -> bool {
        tag.is_none_or(|tag| {
            self.state
                .groups
                .get(&group_id_of(&tag.key))
                .is_some_and(|g| !g.left && (user == self.state.user_id || g.members.iter().any(|m| m == user)))
        })
    }
}

/// A group's name, encrypted under its key, padded to the longest a name may be.
fn title(secret: &GroupSecretParams, name: &str) -> Result<Vec<u8>> {
    if name.is_empty() || name.len() > MAX_NAME_LEN || name.chars().any(char::is_control) {
        return Err(Error::Protocol("a group's name: 1 to 32 bytes"));
    }
    Ok(secret.encrypt_blob_with_padding(crate::identity::random(), name.as_bytes(), (MAX_NAME_LEN - name.len()) as u32))
}

impl Client {
    /// This account's profile key is to go to these users (all but itself), as members of a group it is in.
    pub(crate) fn profile_key_due(&mut self, users: &[String]) {
        for user in users.iter().filter(|u| **u != self.state.user_id) {
            let contact = self.state.contacts.entry(user.clone()).or_default();
            // Once per key: a group's every change need not send it again.
            if !contact.key_given {
                contact.key_due = true;
            }
        }
    }
}
