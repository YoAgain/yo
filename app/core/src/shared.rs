//! Steps on a client that others share behind a lock (the bindings' turn, the CLI's mutex): what travels to the hub goes
//! without holding it, so receiving goes on meanwhile. The same for every app and the CLI.

use crate::media::Media;
use crate::{Client, Content, Error, Event, Sent};

/// What `group:` and a group's id name; anything else is a user id or a name.
pub const GROUP_PREFIX: &str = "group:";

/// Where a message goes: a user (by id, or a typed name) or a group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    User(String),
    Group(String),
}

impl Target {
    /// A target as the apps and the CLI keep it: a user id, a name, or `group:` and the group's id.
    pub fn parse(text: &str) -> Target {
        match text.strip_prefix(GROUP_PREFIX) {
            Some(group) => Target::Group(group.to_owned()),
            None => Target::User(text.to_owned()),
        }
    }
}

/// A client behind a lock: `step` runs one step on it, the lock held for that step only.
pub trait Locked {
    type Error: From<Error>;
    fn step<T>(&self, step: impl FnOnce(&mut Client) -> T) -> Result<T, Self::Error>;

    /// Deliveries whose answers no step took (the lock refused): their users may go again, else nothing more to them
    /// would leave in this process. Without the lock: what is on its way is the process's own, not the state's.
    fn forget(&self, _deliveries: &crate::Deliveries) {}
}

/// The user a name is now, asked without the client (no prekey spent); a user id as it is.
pub fn user_for<L: Locked>(locked: &L, to: &str) -> Result<String, L::Error> {
    if Client::is_user_id(to) {
        return Ok(to.to_owned());
    }
    let whois = locked.step(|c| c.whois_begin(to))??;
    let (user, username, bot) = whois.send()?;
    locked.step(|c| c.note_bot(&user, bot).and_then(|()| c.whois_end(to, &user, username)))??;
    Ok(user)
}

/// Posts deliveries, and what may go after them, without the client, until done.
pub fn deliver<L: Locked>(locked: &L, mut deliveries: crate::Deliveries) -> Result<Vec<Event>, L::Error> {
    loop {
        let answers = deliveries.send();
        match locked.step(|c| c.send_end(&deliveries, answers)) {
            Ok(Sent::More(more)) => deliveries = more,
            Ok(Sent::Done(result)) => return Ok(result?),
            Err(error) => {
                // They wait in the saved outbox and go again; a copy the hub took already is known by its plaintext.
                locked.forget(&deliveries);
                return Err(error);
            }
        }
    }
}

/// A Yo, with a photo or video or not. Where sessions are in place its way to the hub does not hold the client.
pub fn send<L: Locked>(locked: &L, target: &Target, content: Content) -> Result<Vec<Event>, L::Error> {
    match target {
        Target::User(to) => {
            let to = user_for(locked, to)?;
            match locked.step(|c| c.send_begin(&to, content.clone()))? {
                Some(deliveries) => deliver(locked, deliveries),
                None => Ok(locked.step(|c| c.send(&to, content))??),
            }
        }
        Target::Group(id) => match locked.step(|c| c.send_group_begin(id, content.clone()))? {
            Some(deliveries) => deliver(locked, deliveries),
            None => Ok(locked.step(|c| c.send_group(id, content))??),
        },
    }
}

/// Uploads a sealed photo or video for the target before its Yo: the chat (or the group's sessions) opened first if
/// needed, for as many fetches as it has, the upload itself without the client. What opening received.
pub fn upload<L: Locked>(
    locked: &L,
    target: &Target,
    media: &Media,
    ciphertext: &[u8],
) -> Result<Vec<Event>, L::Error> {
    let (fetches, received) = locked.step(|c| match target {
        Target::User(to) => match c.media_fetches(to) {
            Some(fetches) => Ok((fetches, Vec::new())),
            None => c.open_chat(to),
        },
        Target::Group(id) => c.open_group(id),
    })??;
    let uploaded = locked.step(|c| c.upload(media, fetches)).and_then(|upload| Ok(upload?.send(ciphertext)?));
    if let Err(error) = uploaded {
        // What opening received is not lost with the upload: the next call hands it out again.
        _ = locked.step(|c| c.hand_back(&received));
        return Err(error);
    }
    Ok(received)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_is_a_group_only_by_its_prefix() {
        assert_eq!(Target::parse("group:abc"), Target::Group("abc".into()));
        assert_eq!(Target::parse("YoGPT"), Target::User("YoGPT".into()));
        assert_eq!(Target::parse("groupie"), Target::User("groupie".into()));
    }
}
