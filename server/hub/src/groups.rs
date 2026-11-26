//! Groups at the hub (spec 3.4): Signal's Groups v2 with zkgroup; every request checked by proof, nothing learned.

use crate::error::{refuse, Refusal, Res};
use crate::request::{bad, path_id, Auth, Hubs, Peer};
use crate::{now_ms, Hub};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use bytes::Bytes;
use libsignal_protocol::{Aci, Pni, ServiceId};
use yoip::net::api::{self, GroupAction, GroupAnswer, GroupMember, GroupState, NewGroup, NewMember};
use yoip::protocol::content::MAX_GROUP;
use yoip::{decode, encode, UserId, VLBytes};
use zkgroup::auth::{AnyAuthCredentialPresentation, AuthCredentialWithPniZkcResponse};
use zkgroup::groups::{GroupPublicParams, GroupSendDerivedKeyPair, GroupSendEndorsementsResponse, UuidCiphertext};
use zkgroup::profiles::{AnyProfileKeyCredentialPresentation, ProfileKeyCommitment, ProfileKeyCredentialRequest};
use zkgroup::{ServerSecretParams, Timestamp, SECONDS_PER_DAY};

/// The longest encrypted title the hub keeps (a 32-byte name, padded and sealed, is 65).
pub const MAX_TITLE: usize = 256;

/// The hub's zkgroup keys (`YO_ZK_PARAMS_FILE`, `yo-hub zk new`): its `ServerSecretParams`, and their public half.
pub struct Zk {
    secret: ServerSecretParams,
    public: Vec<u8>,
}

impl Zk {
    pub fn from_bytes(bytes: &[u8]) -> Result<Zk, String> {
        let secret: ServerSecretParams = zkgroup::deserialize(bytes).map_err(|_| "not ServerSecretParams")?;
        let public = zkgroup::serialize(&secret.get_public_params());
        Ok(Zk { secret, public })
    }

    /// The public params, as apps pin them.
    pub fn public(&self) -> &[u8] {
        &self.public
    }
}

/// New `ServerSecretParams`, as the file the hub reads, and their public half: for the apps.
pub fn new_params() -> (Vec<u8>, Vec<u8>) {
    let secret = ServerSecretParams::generate(random());
    (zkgroup::serialize(&secret), zkgroup::serialize(&secret.get_public_params()))
}

/// The test hubs' params (`api::TEST_ZK_SEED`).
pub fn test_params() -> Vec<u8> {
    zkgroup::serialize(&ServerSecretParams::generate(api::TEST_ZK_SEED))
}

/// The test hubs' public params, as test clients pin them.
#[cfg(test)]
fn test_public() -> zkgroup::ServerPublicParams {
    ServerSecretParams::generate(api::TEST_ZK_SEED).get_public_params()
}

fn random() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("randomness");
    bytes
}

fn aci(user: &UserId) -> Aci {
    Aci::from_uuid_bytes(*user)
}

fn forbidden() -> Refusal {
    refuse(StatusCode::FORBIDDEN)
}

impl Hub {
    fn zk_now(&self) -> Timestamp {
        Timestamp::from_epoch_seconds(self.clock_ms() / 1000)
    }

    /// Today's auth credential for `user` (its ACI; Yo has no PNI: the same UUID as one).
    pub(crate) fn group_auth(&self, user: &UserId) -> api::GroupAuthCredential {
        let now = self.zk_now().epoch_seconds();
        let day = now - now % SECONDS_PER_DAY;
        let response = AuthCredentialWithPniZkcResponse::issue_credential(
            aci(user),
            Pni::from_uuid_bytes(*user),
            Timestamp::from_epoch_seconds(day),
            &self.zk.secret,
            random(),
        );
        api::GroupAuthCredential { redemption_s: day, credential: zkgroup::serialize(&response).into() }
    }

    /// A profile key credential for `request.user_id` (`404` no commitment, `403` the key not proved).
    pub(crate) fn profile_credential(&self, request: &api::ProfileCredentialRequest) -> Res<Vec<u8>> {
        let commitment = self.db.commitment(&request.user_id)?.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
        let commitment: ProfileKeyCommitment = zkgroup::deserialize(&commitment).map_err(|_| forbidden())?;
        let asked: ProfileKeyCredentialRequest = zkgroup::deserialize(request.request.as_slice()).map_err(|_| bad())?;
        let now = self.zk_now().epoch_seconds();
        let expiration = Timestamp::from_epoch_seconds(now - now % SECONDS_PER_DAY + 7 * SECONDS_PER_DAY);
        let response = self
            .zk
            .secret
            .issue_expiring_profile_key_credential(random(), &asked, aci(&request.user_id), commitment, expiration)
            .map_err(|_| forbidden())?;
        Ok(zkgroup::serialize(&response))
    }

    /// The member an auth presentation names, if it holds for these params now: its `UuidCiphertext`'s bytes.
    fn presenter(&self, params: &GroupPublicParams, auth: &[u8]) -> Res<Vec<u8>> {
        let presentation = AnyAuthCredentialPresentation::new(auth).map_err(|_| bad())?;
        self.zk
            .secret
            .verify_auth_credential_presentation(*params, &presentation, self.zk_now())
            .map_err(|_| forbidden())?;
        Ok(zkgroup::serialize(&presentation.get_aci_ciphertext()))
    }

    /// A member added: with a presentation, the user and profile key it proves; without, an invited user.
    fn new_member(&self, params: &GroupPublicParams, member: &NewMember, by: &[u8]) -> Res<GroupMember> {
        let (user, profile_key) = if member.presentation.as_slice().is_empty() {
            let user: UuidCiphertext = zkgroup::deserialize(member.user.as_slice()).map_err(|_| bad())?;
            (zkgroup::serialize(&user), Vec::new())
        } else {
            let presentation =
                AnyProfileKeyCredentialPresentation::new(member.presentation.as_slice()).map_err(|_| bad())?;
            self.zk
                .secret
                .verify_profile_key_credential_presentation(*params, &presentation, self.zk_now())
                .map_err(|_| forbidden())?;
            (
                zkgroup::serialize(&presentation.get_uuid_ciphertext()),
                zkgroup::serialize(&presentation.get_profile_key_ciphertext()),
            )
        };
        Ok(GroupMember { user: user.into(), profile_key: profile_key.into(), added_by: by.to_vec().into() })
    }

    /// The group, and endorsements for its members with their profile key, today's, if `to` is one of them with its
    /// own (Signal's full members, spec 3.4). An invited member (no key in its entry: no yes yet) is endorsed for no
    /// one and to no one: a delivery to it is a request (3.3), so whoever adds anyone to a group reaches them no
    /// further than by writing to them.
    fn answer(&self, state: GroupState, to: &[u8]) -> Res<GroupAnswer> {
        let keyed = |m: &&GroupMember| !m.profile_key.as_slice().is_empty();
        let endorsements = match state.members.iter().filter(keyed).any(|m| m.user.as_slice() == to) {
            true => {
                let users = state
                    .members
                    .iter()
                    .filter(keyed)
                    .map(|m| zkgroup::deserialize::<UuidCiphertext>(m.user.as_slice()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
                let expiration = GroupSendEndorsementsResponse::default_expiration(self.zk_now());
                let key = GroupSendDerivedKeyPair::for_expiration(expiration, &self.zk.secret);
                zkgroup::serialize(&GroupSendEndorsementsResponse::issue(users, &key, random()))
            }
            false => Vec::new(),
        };
        Ok(GroupAnswer { state, endorsements: endorsements.into() })
    }

    /// Makes a group (`409` if its id is taken).
    pub(crate) fn create_group(&self, id: &[u8; api::GROUP_ID_LEN], request: &NewGroup) -> Res<GroupAnswer> {
        let params: GroupPublicParams = zkgroup::deserialize(request.public_params.as_slice()).map_err(|_| bad())?;
        if params.get_group_identifier() != *id || request.title.as_slice().len() > MAX_TITLE {
            return Err(bad());
        }
        let maker = self.presenter(&params, request.auth.as_slice())?;
        if request.members.is_empty() || request.members.len() > MAX_GROUP {
            return Err(bad());
        }
        let members =
            request.members.iter().map(|m| self.new_member(&params, m, &maker)).collect::<Res<Vec<GroupMember>>>()?;
        if !members.iter().any(|m| m.user.as_slice() == maker) || duplicates(&members) {
            return Err(bad());
        }
        let state = GroupState { revision: 1, title: request.title.clone(), members };
        if !self.db.create_group(id, request.public_params.as_slice(), &encode(&state), now_ms())? {
            return Err(refuse(StatusCode::CONFLICT));
        }
        self.count(crate::Event::GroupCreate, 1);
        self.answer(state, &maker)
    }

    /// The group, or changed one revision on (`409` with the group as it is); the last member out ends it.
    pub(crate) fn group_request(&self, id: &[u8; api::GROUP_ID_LEN], request: &api::GroupRequest) -> Res<GroupAnswer> {
        // No change needs more (every member out and others in, a title, a key): refused before any proof is checked.
        if request.actions.len() > 2 * MAX_GROUP + 2 {
            return Err(bad());
        }
        let (params, mut state) = self.group(id)?;
        let me = self.presenter(&params, request.auth.as_slice())?;
        if !state.members.iter().any(|m| m.user.as_slice() == me) {
            return Err(forbidden());
        }
        if request.actions.is_empty() {
            return self.answer(state, &me);
        }
        self.db_room()?;
        if request.revision != state.revision.wrapping_add(1) {
            return Err(Refusal(StatusCode::CONFLICT, encode(&self.answer(state, &me)?)));
        }
        let before = state.revision;
        for action in &request.actions {
            self.apply(&params, &mut state, action, &me)?;
        }
        if state.members.len() > MAX_GROUP || duplicates(&state.members) {
            return Err(bad());
        }
        state.revision = request.revision;
        let stored = match state.members.is_empty() {
            true => self.db.end_group(id, before, now_ms())?,
            false => self.db.update_group(id, before, &encode(&state), now_ms())?,
        };
        if !stored {
            // Another change came in between: as if it had come first.
            return Err(Refusal(StatusCode::CONFLICT, encode(&self.answer(self.group(id)?.1, &me)?)));
        }
        self.answer(state, &me)
    }

    /// A group as stored: its params and state (`404` none).
    fn group(&self, id: &[u8]) -> Res<(GroupPublicParams, GroupState)> {
        let (params, state) = self.db.group(id)?.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
        let broken = || refuse(StatusCode::INTERNAL_SERVER_ERROR);
        Ok((zkgroup::deserialize(&params).map_err(|_| broken())?, decode(&state).map_err(|_| broken())?))
    }

    fn apply(&self, params: &GroupPublicParams, state: &mut GroupState, action: &GroupAction, me: &[u8]) -> Res<()> {
        let data = action.data.as_slice();
        match action.kind {
            api::ACTION_ADD => {
                let member = self.new_member(params, &decode::<NewMember>(data)?, me)?;
                state.members.push(member);
            }
            api::ACTION_REMOVE => {
                let at = state.members.iter().position(|m| m.user.as_slice() == data).ok_or_else(bad)?;
                state.members.remove(at);
            }
            api::ACTION_TITLE if data.len() <= MAX_TITLE => state.title = VLBytes::from(data.to_vec()),
            api::ACTION_PROFILE_KEY => {
                // Its own key only: from invited to a member with its key (a yes to a request).
                let member = self.new_member(
                    params,
                    &NewMember { user: VLBytes::from(vec![]), presentation: data.to_vec().into() },
                    me,
                )?;
                if member.user.as_slice() != me {
                    return Err(forbidden());
                }
                let entry = state.members.iter_mut().find(|m| m.user.as_slice() == me).ok_or_else(forbidden)?;
                entry.profile_key = member.profile_key;
            }
            _ => return Err(bad()),
        }
        Ok(())
    }

    /// Whether a delivery to `user` carries a good group send token for them (spec 3.4): a contact's then.
    pub(crate) fn endorsed(&self, user: &UserId, token: &str) -> bool {
        let Some(token) = yoip::unb64(token)
            .ok()
            .and_then(|bytes| zkgroup::deserialize::<zkgroup::groups::GroupSendFullToken>(&bytes).ok())
        else {
            return false;
        };
        let now = self.zk_now();
        let expiration = token.expiration();
        // Never one longer than the hub issues them (`default_expiration`: two to three days).
        if !expiration.is_day_aligned() || expiration.epoch_seconds() > now.epoch_seconds() + 4 * SECONDS_PER_DAY {
            return false;
        }
        let key = GroupSendDerivedKeyPair::for_expiration(expiration, &self.zk.secret);
        token.verify([ServiceId::from(aci(user))], now, &key).is_ok()
    }
}

/// `GET /v1/group-auth`: today's auth credential for the signing device's account.
pub(crate) async fn get_group_auth(State(hub): Hubs, Auth(device, _): Auth) -> Res<Vec<u8>> {
    Ok(encode(&hub.group_auth(&device.user_id)))
}

/// A group request as it came (spec 3.4): anonymous, on the connection that names no account, the zkgroup proof in
/// its body all that authorises it, limited by its address; or signed, as apps before 2026-11-24 send it (limited
/// by its device then). A signature that is there must hold.
pub(crate) struct GroupAsk {
    body: Bytes,
}

impl axum::extract::FromRequest<std::sync::Arc<Hub>> for GroupAsk {
    type Rejection = Refusal;

    async fn from_request(request: axum::extract::Request, hub: &std::sync::Arc<Hub>) -> Res<Self> {
        let peer = request
            .extensions()
            .get::<Peer>()
            .map(|axum::extract::ConnectInfo(peer)| *peer)
            .ok_or_else(|| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
        let (parts, body) = request.into_parts();
        let body = axum::body::to_bytes(body, yoip::net::limits::MAX_BODY)
            .await
            .map_err(|_| refuse(StatusCode::PAYLOAD_TOO_LARGE))?;
        match parts.headers.contains_key(yoip::net::request::HEADER_DEVICE) {
            true => hub.authenticate(&parts.method, &parts.uri, &parts.headers, &body).map(drop)?,
            // Anonymous: limited by its address, as every unsigned request (spec 4.1).
            false => hub.limit_anonymous(peer, &parts.headers)?,
        }
        Ok(GroupAsk { body })
    }
}

/// `POST /v1/group-credential`: a profile key credential for a user whose key the asker holds.
pub(crate) async fn post_group_credential(State(hub): Hubs, ask: GroupAsk) -> Res<Vec<u8>> {
    hub.profile_credential(&decode(&ask.body)?)
}

/// `PUT /v1/groups/{id}`: a new group, while the database's disk has room.
pub(crate) async fn put_group(State(hub): Hubs, Path(id): Path<String>, ask: GroupAsk) -> Res<Vec<u8>> {
    hub.db_room()?;
    Ok(encode(&hub.create_group(&path_id(&id)?, &decode(&ask.body)?)?))
}

/// `POST /v1/groups/{id}`: a member's request (`GroupRequest`).
pub(crate) async fn post_group(State(hub): Hubs, Path(id): Path<String>, ask: GroupAsk) -> Res<Vec<u8>> {
    Ok(encode(&hub.group_request(&path_id(&id)?, &decode(&ask.body)?)?))
}

fn duplicates(members: &[GroupMember]) -> bool {
    members.iter().enumerate().any(|(i, m)| members[..i].iter().any(|o| o.user == m.user))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::sync::Arc;
    use zkgroup::auth::AuthCredentialWithPni;
    use zkgroup::groups::{GroupMasterKey, GroupSecretParams};
    use zkgroup::profiles::{ExpiringProfileKeyCredentialResponse, ProfileKey};

    fn hub() -> Arc<Hub> {
        Hub::new(Config::for_tests(), Arc::new(crate::push::Recorder::default())).unwrap()
    }

    /// A member's auth presentation for the group, as a client makes it.
    fn auth(hub: &Hub, user: &UserId, secret: &GroupSecretParams) -> VLBytes {
        let public = test_public();
        let issued = hub.group_auth(user);
        let response: AuthCredentialWithPniZkcResponse = zkgroup::deserialize(issued.credential.as_slice()).unwrap();
        let credential = response
            .receive(
                aci(user),
                Pni::from_uuid_bytes(*user),
                Timestamp::from_epoch_seconds(issued.redemption_s),
                &public,
            )
            .unwrap();
        zkgroup::serialize(&AuthCredentialWithPni::Zkc(credential).present(&public, secret, random())).into()
    }

    /// A member added with its profile key: the hub issues the credential against the commitment it told.
    fn keyed(hub: &Hub, user: &UserId, key: [u8; 32], secret: &GroupSecretParams) -> NewMember {
        let public = test_public();
        let commitment = ProfileKey::create(key).get_commitment(aci(user));
        hub.db.set_commitment_for_tests(user, &zkgroup::serialize(&commitment));
        let context =
            public.create_profile_key_credential_request_context(random(), aci(user), ProfileKey::create(key));
        let request = api::ProfileCredentialRequest {
            user_id: *user,
            request: zkgroup::serialize(&context.get_request()).into(),
        };
        let response: ExpiringProfileKeyCredentialResponse =
            zkgroup::deserialize(&hub.profile_credential(&request).unwrap()).unwrap();
        let credential = public.receive_expiring_profile_key_credential(&context, &response, hub.zk_now()).unwrap();
        let presentation = public.create_expiring_profile_key_credential_presentation(random(), *secret, credential);
        NewMember { user: Vec::new().into(), presentation: zkgroup::serialize(&presentation).into() }
    }

    fn invited(user: &UserId, secret: &GroupSecretParams) -> NewMember {
        let user = zkgroup::serialize(&secret.encrypt_service_id(aci(user).into()));
        NewMember { user: user.into(), presentation: Vec::new().into() }
    }

    #[test]
    fn a_group_is_made_changed_by_revision_and_endorses_its_members() {
        let hub = hub();
        let (ann, ben, cleo) = ([1u8; 16], [2u8; 16], [3u8; 16]);
        let secret = GroupSecretParams::derive_from_master_key(GroupMasterKey::new([7; 32]));
        let id = secret.get_group_identifier();
        let new = NewGroup {
            public_params: zkgroup::serialize(&secret.get_public_params()).into(),
            auth: auth(&hub, &ann, &secret),
            title: secret.encrypt_blob_with_padding(random(), b"T", 31).into(),
            members: vec![keyed(&hub, &ann, [9; 32], &secret), invited(&ben, &secret)],
        };
        let made = hub.create_group(&id, &new).unwrap();
        assert_eq!(made.state.revision, 1);
        assert!(!made.state.members[0].profile_key.as_slice().is_empty(), "with its key");
        assert!(made.state.members[1].profile_key.as_slice().is_empty(), "invited");
        assert_eq!(hub.create_group(&id, &new).unwrap_err().0, StatusCode::CONFLICT, "taken");
        // Counted once, with nothing of which group; the group knows when it was made.
        hub.flush_events(120_000).unwrap();
        let counted: Vec<(String, i64, i64)> = hub
            .db
            .conn()
            .prepare("SELECT kind, minute, n FROM stat_minute")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(counted, [("group-create".to_owned(), 2, 1)]);
        let made_at: Option<i64> = hub.db.conn().query_row("SELECT created_at FROM grp", [], |r| r.get(0)).unwrap();
        assert!(made_at.is_some_and(|at| at > 0));
        // A stranger's presentation is no member's.
        let fetch = |who: &UserId| api::GroupRequest { auth: auth(&hub, who, &secret), revision: 0, actions: vec![] };
        assert_eq!(hub.group_request(&id, &fetch(&cleo)).unwrap_err().0, StatusCode::FORBIDDEN);
        // Endorsements for Ann alone, who has her key in: Ben is only invited (spec 3.4), and none is for him.
        let response: GroupSendEndorsementsResponse = zkgroup::deserialize(made.endorsements.as_slice()).unwrap();
        let both = [aci(&ann).into(), aci(&ben).into()];
        assert!(response
            .clone()
            .receive_with_service_ids_single_threaded(both, hub.zk_now(), &secret, test_public())
            .is_err());
        let received = response
            .receive_with_service_ids_single_threaded([aci(&ann).into()], hub.zk_now(), &secret, test_public())
            .unwrap();
        assert_eq!(received.len(), 1);
        // Invited, Ben gets the group (to say yes or no) and no endorsement.
        let invited_answer = hub.group_request(&id, &fetch(&ben)).unwrap();
        assert!(invited_answer.endorsements.as_slice().is_empty(), "nothing for one not keyed");
        // His yes (his key into his entry): from then on both are endorsed, and a token for Ben holds for Ben only.
        let yes = api::GroupRequest {
            auth: auth(&hub, &ben, &secret),
            revision: 2,
            actions: vec![GroupAction {
                kind: api::ACTION_PROFILE_KEY,
                data: keyed(&hub, &ben, [8; 32], &secret).presentation,
            }],
        };
        let joined = hub.group_request(&id, &yes).unwrap();
        let response: GroupSendEndorsementsResponse = zkgroup::deserialize(joined.endorsements.as_slice()).unwrap();
        let expiration = response.expiration();
        let received =
            response.receive_with_service_ids_single_threaded(both, hub.zk_now(), &secret, test_public()).unwrap();
        let token = received[1].decompressed.to_token(secret).into_full_token(expiration);
        let token = yoip::b64(&zkgroup::serialize(&token));
        assert!(hub.endorsed(&ben, &token));
        assert!(!hub.endorsed(&cleo, &token));
        // Two changes at revision 3: the first stands, the second is answered the group as it is.
        let add = |who: &UserId| api::GroupRequest {
            auth: auth(&hub, &ann, &secret),
            revision: 3,
            actions: vec![GroupAction { kind: api::ACTION_ADD, data: yoip::encode(&invited(who, &secret)).into() }],
        };
        assert_eq!(hub.group_request(&id, &add(&cleo)).unwrap().state.members.len(), 3);
        // More actions than any change needs: refused before a proof is checked (each costs the hub milliseconds).
        let mut many = add(&[5; 16]);
        many.revision = 4;
        many.actions = vec![many.actions[0].clone(); 2 * MAX_GROUP + 3];
        assert_eq!(hub.group_request(&id, &many).unwrap_err().0, StatusCode::BAD_REQUEST);
        let refused = hub.group_request(&id, &add(&[4; 16])).unwrap_err();
        assert_eq!(refused.0, StatusCode::CONFLICT);
        assert_eq!(yoip::decode::<GroupAnswer>(&refused.1).unwrap().state.revision, 3);
        // Ben leaves: answered without endorsements; the others go too, and the group ends.
        for (who, revision) in [(ben, 4), (cleo, 5), (ann, 6)] {
            let me = secret.encrypt_service_id(aci(&who).into());
            let leave = api::GroupRequest {
                auth: auth(&hub, &who, &secret),
                revision,
                actions: vec![GroupAction { kind: api::ACTION_REMOVE, data: zkgroup::serialize(&me).into() }],
            };
            assert!(hub.group_request(&id, &leave).unwrap().endorsements.as_slice().is_empty());
        }
        assert_eq!(hub.group_request(&id, &fetch(&ann)).unwrap_err().0, StatusCode::NOT_FOUND, "ended");
        // Audit I5: an ended group's id is gone for good; whoever has its old key makes it no more.
        assert_eq!(hub.create_group(&id, &new).unwrap_err().0, StatusCode::CONFLICT, "its id gone");
        let other = GroupSecretParams::derive_from_master_key(GroupMasterKey::new([0x44; 32]));
        let again = NewGroup {
            public_params: zkgroup::serialize(&other.get_public_params()).into(),
            auth: auth(&hub, &ann, &other),
            title: other.encrypt_blob_with_padding(random(), b"T", 31).into(),
            members: vec![keyed(&hub, &ann, [9; 32], &other)],
        };
        assert!(hub.create_group(&other.get_group_identifier(), &again).is_ok(), "another id: made");
    }

    /// Audit G1: a stranger names any user as an invited member (no profile key, no consent): the hub endorses no one
    /// for it, so no token admits a sealed delivery to that user; what the stranger sends there is a request (3.3).
    #[test]
    fn a_stranger_gets_no_send_token_for_anyone_it_invites() {
        let hub = hub();
        let (mallory, victim) = ([0x4d; 16], [0x56; 16]);
        let secret = GroupSecretParams::derive_from_master_key(GroupMasterKey::new([0x33; 32]));
        let id = secret.get_group_identifier();
        let new = NewGroup {
            public_params: zkgroup::serialize(&secret.get_public_params()).into(),
            auth: auth(&hub, &mallory, &secret),
            title: secret.encrypt_blob_with_padding(random(), b"X", 31).into(),
            members: vec![keyed(&hub, &mallory, [9; 32], &secret), invited(&victim, &secret)],
        };
        let made = hub.create_group(&id, &new).unwrap();
        let response: GroupSendEndorsementsResponse = zkgroup::deserialize(made.endorsements.as_slice()).unwrap();
        let with_victim = [aci(&mallory).into(), aci(&victim).into()];
        let now = hub.zk_now();
        assert!(response
            .clone()
            .receive_with_service_ids_single_threaded(with_victim, now, &secret, test_public())
            .is_err());
        let received = response
            .receive_with_service_ids_single_threaded([aci(&mallory).into()], now, &secret, test_public())
            .unwrap();
        assert_eq!(received.len(), 1, "Mallory's own, no other");
        // Her own endorsement made into a token for the victim holds for no one but her.
        let expiration = GroupSendEndorsementsResponse::default_expiration(now);
        let token = received[0].decompressed.to_token(secret).into_full_token(expiration);
        assert!(!hub.endorsed(&victim, &yoip::b64(&zkgroup::serialize(&token))));
        // A group of invited members only but its maker: the same with more of them, and with a change.
        let more = api::GroupRequest {
            auth: auth(&hub, &mallory, &secret),
            revision: 2,
            actions: vec![GroupAction {
                kind: api::ACTION_ADD,
                data: yoip::encode(&invited(&[0x57; 16], &secret)).into(),
            }],
        };
        let changed = hub.group_request(&id, &more).unwrap();
        let response: GroupSendEndorsementsResponse = zkgroup::deserialize(changed.endorsements.as_slice()).unwrap();
        assert!(response
            .receive_with_service_ids_single_threaded([aci(&mallory).into()], now, &secret, test_public())
            .is_ok());
    }

    /// Audit G3: a member adds a `UuidCiphertext` that is no member of this group (one of another group's). The hub
    /// cannot tell (it never decrypts), keeps it, and endorses it as no one's; any member removes it by its bytes.
    #[test]
    fn an_entry_no_client_can_open_is_removed_by_its_bytes() {
        let hub = hub();
        let (ann, ben) = ([1u8; 16], [2u8; 16]);
        let secret = GroupSecretParams::derive_from_master_key(GroupMasterKey::new([7; 32]));
        let other = GroupSecretParams::derive_from_master_key(GroupMasterKey::new([8; 32]));
        let id = secret.get_group_identifier();
        let new = NewGroup {
            public_params: zkgroup::serialize(&secret.get_public_params()).into(),
            auth: auth(&hub, &ann, &secret),
            title: secret.encrypt_blob_with_padding(random(), b"T", 31).into(),
            members: vec![keyed(&hub, &ann, [9; 32], &secret), invited(&ben, &secret)],
        };
        hub.create_group(&id, &new).unwrap();
        let foreign = invited(&ben, &other).user;
        let add = api::GroupRequest {
            auth: auth(&hub, &ann, &secret),
            revision: 2,
            actions: vec![GroupAction { kind: api::ACTION_ADD, data: yoip::encode(&invited(&ben, &other)).into() }],
        };
        let answer = hub.group_request(&id, &add).expect("the hub keeps it");
        let unreadable: UuidCiphertext = zkgroup::deserialize(answer.state.members[2].user.as_slice()).unwrap();
        assert!(secret.decrypt_service_id(unreadable).is_err(), "no member's client can open it");
        let remove = api::GroupRequest {
            auth: auth(&hub, &ann, &secret),
            revision: 3,
            actions: vec![GroupAction { kind: api::ACTION_REMOVE, data: foreign }],
        };
        assert_eq!(hub.group_request(&id, &remove).unwrap().state.members.len(), 2, "gone again");
    }

    #[test]
    fn a_profile_key_credential_needs_the_key_behind_the_commitment() {
        let hub = hub();
        let ann = [1u8; 16];
        let public = test_public();
        let request = |key| {
            let context =
                public.create_profile_key_credential_request_context(random(), aci(&ann), ProfileKey::create(key));
            api::ProfileCredentialRequest { user_id: ann, request: zkgroup::serialize(&context.get_request()).into() }
        };
        assert_eq!(hub.profile_credential(&request([5; 32])).unwrap_err().0, StatusCode::NOT_FOUND, "none told");
        let commitment = ProfileKey::create([5; 32]).get_commitment(aci(&ann));
        hub.db.set_commitment_for_tests(&ann, &zkgroup::serialize(&commitment));
        assert!(hub.profile_credential(&request([5; 32])).is_ok());
        assert_eq!(hub.profile_credential(&request([6; 32])).unwrap_err().0, StatusCode::FORBIDDEN, "another key");
    }

    /// Audit M1: group requests go anonymous (the zkgroup proof all that authorises them, spec 3.4), counted by
    /// address in the one anonymous bucket (spec 4.1); a signed one, as apps before send it, is still taken and
    /// counted by its device, and a signature that is there must hold.
    #[tokio::test]
    async fn group_requests_go_anonymous_and_anonymous_ones_count_by_address() {
        use axum::{body::Body, extract::ConnectInfo, http::Request};
        use std::net::SocketAddr;
        use tower::ServiceExt;
        let limits = crate::config::Limits { anonymous_per_ip_minute: 3, ..crate::config::Limits::unlimited() };
        let hub =
            Hub::new(Config { limits, ..Config::for_tests() }, Arc::new(crate::push::Recorder::default())).unwrap();
        // Ann's device, registered, for the signed requests of apps before.
        let pair = libsignal_protocol::KeyPair::generate(&mut rand::rng());
        let auth_key: [u8; 32] = pair.public_key.public_key_bytes().try_into().unwrap();
        let ann = [1u8; 16];
        let device = crate::db::NewDevice {
            device_id: [1; 16],
            device_key: [1; 32],
            auth_key,
            registration_id: 1,
            push_platform: 0,
            push_token: Vec::new(),
            app: None,
            version: None,
        };
        let prekeys = api::Prekeys { signed: None, last_resort: None, one_time: Vec::new(), kyber: Vec::new() };
        hub.db.register(b"ANN", "ANN", &ann, &[1; 32], &device, &prekeys, 10, 0).unwrap();
        let send = |method: &str, path: String, body: Vec<u8>, ip: [u8; 4], signed: bool| {
            let mut request = Request::builder().method(method).uri(&path);
            if signed {
                let time = crate::now_ms();
                let message = yoip::net::request::to_be_signed(method, &path, time, &body);
                let signature = pair.private_key.calculate_signature(&message, &mut rand::rng()).unwrap();
                request = request
                    .header(yoip::net::request::HEADER_DEVICE, yoip::b64(&[1; 16]))
                    .header(yoip::net::request::HEADER_TIME, time.to_string())
                    .header(yoip::net::request::HEADER_SIGNATURE, yoip::b64(&signature));
            }
            let mut request = request.body(Body::from(body)).unwrap();
            request.extensions_mut().insert(ConnectInfo(SocketAddr::from((ip, 1))));
            let router = hub.router();
            async move { router.oneshot(request).await.unwrap().status() }
        };
        let made = |key: u8| {
            let secret = GroupSecretParams::derive_from_master_key(GroupMasterKey::new([key; 32]));
            let new = NewGroup {
                public_params: zkgroup::serialize(&secret.get_public_params()).into(),
                auth: auth(&hub, &ann, &secret),
                title: secret.encrypt_blob_with_padding(random(), b"T", 31).into(),
                members: vec![keyed(&hub, &ann, [9; 32], &secret)],
            };
            (secret, yoip::encode(&new))
        };
        let (secret, body) = made(1);
        let path = api::group_path(&secret.get_group_identifier());
        let fetch =
            || yoip::encode(&api::GroupRequest { auth: auth(&hub, &ann, &secret), revision: 0, actions: vec![] });
        let ip = [192, 0, 2, 1];
        assert_eq!(send("PUT", path.clone(), body, ip, false).await, StatusCode::OK, "made, anonymously");
        for _ in 0..2 {
            assert_eq!(send("POST", path.clone(), fetch(), ip, false).await, StatusCode::OK);
        }
        assert_eq!(send("POST", path.clone(), fetch(), ip, false).await, StatusCode::TOO_MANY_REQUESTS, "by address");
        let (other, body) = made(2);
        let other_path = api::group_path(&other.get_group_identifier());
        assert_eq!(send("PUT", other_path.clone(), body.clone(), ip, false).await, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(send("PUT", other_path, body, ip, true).await, StatusCode::OK, "signed: by the device");
        assert_eq!(send("POST", path.clone(), fetch(), [192, 0, 2, 2], false).await, StatusCode::OK, "another address");
        assert_eq!(send("POST", path.clone(), fetch(), ip, true).await, StatusCode::OK, "signed: not by address");
        // A signature there that does not hold: refused, not taken as anonymous.
        let mut forged = Request::post(&path)
            .header(yoip::net::request::HEADER_DEVICE, yoip::b64(&[1; 16]))
            .header(yoip::net::request::HEADER_TIME, crate::now_ms().to_string())
            .header(yoip::net::request::HEADER_SIGNATURE, yoip::b64(&[0u8; 64]))
            .body(Body::from(fetch()))
            .unwrap();
        forged.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 3], 1))));
        assert_eq!(hub.router().oneshot(forged).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        // Not a member: the proof decides, as signed.
        let stranger = api::GroupRequest { auth: auth(&hub, &[2; 16], &secret), revision: 0, actions: vec![] };
        let status = send("POST", path.clone(), yoip::encode(&stranger), [192, 0, 2, 4], false).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // Credentials: anonymous, counted by address.
        let context = test_public().create_profile_key_credential_request_context(
            random(),
            aci(&ann),
            ProfileKey::create([9; 32]),
        );
        let asked = yoip::encode(&api::ProfileCredentialRequest {
            user_id: ann,
            request: zkgroup::serialize(&context.get_request()).into(),
        });
        let ip = [192, 0, 2, 5];
        for _ in 0..3 {
            assert_eq!(send("POST", api::GROUP_CREDENTIAL.into(), asked.clone(), ip, false).await, StatusCode::OK);
        }
        let status = send("POST", api::GROUP_CREDENTIAL.into(), asked.clone(), ip, false).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(send("POST", api::GROUP_CREDENTIAL.into(), asked, ip, true).await, StatusCode::OK, "signed");
        hub.closing.send_replace(true);
    }
}
