//! Groups at the hub (spec 4.9): Signal's Groups v2 with zkgroup, a small part of it. Per group the hub keeps its
//! `GroupPublicParams`, its revision and its state encrypted under the group's key (members as `UuidCiphertext`s with
//! their `ProfileKeyCiphertext`s, the title): it checks every request by zero-knowledge proof (an auth credential
//! presentation names a member, a profile key credential presentation a member added), never learns who is in a group
//! or what it is called. Every member may change it (no admins); a change names the revision it makes, the hub's +1:
//! of two at once, one wins and the other is answered `409` with the group as it is (its maker builds on that).

use crate::error::{refuse, Refusal, Res};
use crate::{now_ms, Hub};
use axum::http::StatusCode;
use libsignal_protocol::{Aci, Pni, ServiceId};
use yo_wire::api::{self, GroupAction, GroupAnswer, GroupMember, GroupState, NewGroup, NewMember};
use yo_wire::content::MAX_GROUP;
use yo_wire::{UserId, VLBytes};
use zkgroup::auth::{AnyAuthCredentialPresentation, AuthCredentialWithPniZkcResponse};
use zkgroup::groups::{GroupPublicParams, GroupSendDerivedKeyPair, GroupSendEndorsementsResponse, UuidCiphertext};
use zkgroup::profiles::{AnyProfileKeyCredentialPresentation, ProfileKeyCommitment, ProfileKeyCredentialRequest};
use zkgroup::{ServerPublicParams, ServerSecretParams, Timestamp, SECONDS_PER_DAY};

/// The longest encrypted title the hub keeps (a 32-byte name, padded and sealed, is 65).
pub const MAX_TITLE: usize = 256;
/// Profile key credentials a device may ask for a day.
pub const CREDENTIALS_PER_DAY: u32 = 1000;

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
pub fn test_public() -> ServerPublicParams {
    ServerSecretParams::generate(api::TEST_ZK_SEED).get_public_params()
}

fn random() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("randomness");
    bytes
}

fn aci(user: &UserId) -> Aci {
    Aci::from_uuid_bytes(*user)
}

fn forbidden() -> Refusal {
    refuse(StatusCode::FORBIDDEN)
}

fn bad() -> Refusal {
    refuse(StatusCode::BAD_REQUEST)
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

    /// A profile key credential for `request.user_id` against the commitment its account told (`404` none), if the
    /// request proves the key behind it (`403` otherwise), good for six to seven days.
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

    /// The group and endorsements for its members, today's, if `to` is one of them.
    fn answer(&self, state: GroupState, to: &[u8]) -> Res<GroupAnswer> {
        let endorsements = match state.members.iter().any(|m| m.user.as_slice() == to) {
            true => {
                let users = state
                    .members
                    .iter()
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
        if !self.db.create_group(id, request.public_params.as_slice(), &yo_wire::encode(&state), now_ms())? {
            return Err(refuse(StatusCode::CONFLICT));
        }
        self.answer(state, &maker)
    }

    /// A member's request: the group as it is, or changed as one revision on (`409` with the group as it is when the
    /// revision is not the next). A member removing itself is answered the group without endorsements; the last one
    /// out ends it.
    pub(crate) fn group_request(&self, id: &[u8; api::GROUP_ID_LEN], request: &api::GroupRequest) -> Res<GroupAnswer> {
        let (params, state) = self.db.group(id)?.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
        let params: GroupPublicParams =
            zkgroup::deserialize(&params).map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
        let mut state: GroupState = yo_wire::decode(&state).map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
        let me = self.presenter(&params, request.auth.as_slice())?;
        if !state.members.iter().any(|m| m.user.as_slice() == me) {
            return Err(forbidden());
        }
        if request.actions.is_empty() {
            return self.answer(state, &me);
        }
        if request.revision != state.revision.wrapping_add(1) {
            let answer = self.answer(state, &me)?;
            return Err(Refusal(StatusCode::CONFLICT, yo_wire::encode(&answer)));
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
            true => self.db.end_group(id, before)?,
            false => self.db.update_group(id, before, &yo_wire::encode(&state), now_ms())?,
        };
        if !stored {
            // Another change came in between: as if it had come first.
            let (_, now) = self.db.group(id)?.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
            let now: GroupState = yo_wire::decode(&now).map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
            return Err(Refusal(StatusCode::CONFLICT, yo_wire::encode(&self.answer(now, &me)?)));
        }
        self.answer(state, &me)
    }

    fn apply(&self, params: &GroupPublicParams, state: &mut GroupState, action: &GroupAction, me: &[u8]) -> Res<()> {
        let data = action.data.as_slice();
        match action.kind {
            api::ACTION_ADD => {
                let member = self.new_member(params, &yo_wire::decode::<NewMember>(data)?, me)?;
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

    /// Whether a delivery to `user` carries a good group send token for them (spec 4.9): a contact's then.
    pub(crate) fn endorsed(&self, user: &UserId, token: &str) -> bool {
        let Some(token) = yo_wire::unb64(token)
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
        // A stranger's presentation is no member's.
        let fetch = |who: &UserId| api::GroupRequest { auth: auth(&hub, who, &secret), revision: 0, actions: vec![] };
        assert_eq!(hub.group_request(&id, &fetch(&cleo)).unwrap_err().0, StatusCode::FORBIDDEN);
        // Endorsements for Ann and Ben: a token for Ben holds for Ben only.
        let response: GroupSendEndorsementsResponse = zkgroup::deserialize(made.endorsements.as_slice()).unwrap();
        let expiration = response.expiration();
        let received = response
            .receive_with_service_ids_single_threaded(
                [aci(&ann).into(), aci(&ben).into()],
                hub.zk_now(),
                &secret,
                test_public(),
            )
            .unwrap();
        let token = received[1].decompressed.to_token(secret).into_full_token(expiration);
        let token = yo_wire::b64(&zkgroup::serialize(&token));
        assert!(hub.endorsed(&ben, &token));
        assert!(!hub.endorsed(&cleo, &token));
        // Two changes at revision 2: the first stands, the second is answered the group as it is.
        let add = |who: &UserId| api::GroupRequest {
            auth: auth(&hub, &ann, &secret),
            revision: 2,
            actions: vec![GroupAction { kind: api::ACTION_ADD, data: yo_wire::encode(&invited(who, &secret)).into() }],
        };
        assert_eq!(hub.group_request(&id, &add(&cleo)).unwrap().state.members.len(), 3);
        let refused = hub.group_request(&id, &add(&[4; 16])).unwrap_err();
        assert_eq!(refused.0, StatusCode::CONFLICT);
        assert_eq!(yo_wire::decode::<GroupAnswer>(&refused.1).unwrap().state.revision, 2);
        // Ben leaves: answered without endorsements; the others go too, and the group ends.
        for (who, revision) in [(ben, 3), (cleo, 4), (ann, 5)] {
            let me = secret.encrypt_service_id(aci(&who).into());
            let leave = api::GroupRequest {
                auth: auth(&hub, &who, &secret),
                revision,
                actions: vec![GroupAction { kind: api::ACTION_REMOVE, data: zkgroup::serialize(&me).into() }],
            };
            assert!(hub.group_request(&id, &leave).unwrap().endorsements.as_slice().is_empty());
        }
        assert_eq!(hub.group_request(&id, &fetch(&ann)).unwrap_err().0, StatusCode::NOT_FOUND, "ended");
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
}
