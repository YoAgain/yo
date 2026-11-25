use super::*;
use crate::storage::Connection;
use yo_wire::content::{CONNECT, CONNECTED, CONNECT_END, CONNECT_ENDED, CONNECT_REFUSED};

/// How long a connect link's code is good (the service's own time for it).
const WAIT_MS: u64 = 10 * 60 * 1000;

/// A service connected with another one, or on the way there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// The service's user id (hex).
    pub user: String,
    pub name: String,
    pub connected: bool,
    /// Allowed, not connected yet.
    pub waiting: bool,
    /// The last one was refused.
    pub refused: bool,
    /// Asked to end, not confirmed yet.
    pub ending: bool,
}

/// A connection as it came to a service: the user, `CONNECT` or `CONNECT_END`, the code.
pub type ConnectRequest = (String, u8, [u8; 32]);

impl Client {
    /// What users asked of this service's connections, in order; never app events or synced content.
    pub fn take_connect_requests(&mut self) -> Vec<ConnectRequest> {
        std::mem::take(&mut self.connect_requests)
    }

    /// The services connected, waiting, refused or ending, by name; kept in the storage.
    pub fn connections(&self) -> Vec<ConnectionInfo> {
        let now = crate::wall_ms();
        let mut list: Vec<ConnectionInfo> = self
            .state
            .storage
            .connections
            .iter()
            .map(|(user, field)| {
                let c = &field.value;
                ConnectionInfo {
                    user: user.clone(),
                    name: c.name.clone(),
                    connected: c.connected,
                    waiting: c.waiting.is_some() && now < field.at.saturating_add(WAIT_MS),
                    refused: c.refused,
                    ending: c.ending.is_some(),
                }
            })
            .filter(|c| c.connected || c.waiting || c.refused || c.ending)
            .collect();
        list.sort_by(|a, b| (&a.name, &a.user).cmp(&(&b.name, &b.user)));
        list
    }

    /// Connects `service`, a bot, with another one: a contact from now on, told the code of its connect link.
    pub fn connect_service(&mut self, service: &str, code: [u8; 32]) -> Result<Vec<Event>> {
        let result = self.connect_inner(service, code);
        self.finish_sent(result)
    }

    fn connect_inner(&mut self, service: &str, code: [u8; 32]) -> Result<()> {
        if self.replaced.get() {
            return Err(Error::Replaced);
        }
        let handle = Handle::parse(service)?;
        if self.is_me(&handle) {
            return Err(Error::Protocol("sending to oneself"));
        }
        // Who it is and that it is a bot, as the hub says now.
        let (found, username) = self.lookup(&handle, 0, Vec::new())?;
        let user = hex(&found.user_id);
        if user == self.state.user_id {
            return Err(Error::Protocol("sending to oneself"));
        }
        if !self.state.bots.contains(&user) {
            return Err(Error::Protocol("not a service"));
        }
        if self.state.contacts.contains_key(&user) && handle.kind != KIND_USER {
            self.take_handle(&user, username.clone().unwrap_or_else(|| handle.display()));
        }
        self.check_sendable(Some(&user))?;
        if !self.has_chat(&user) {
            let id = Handle::user(&found.user_id);
            self.start_chat(&id)?;
        }
        let name = username.unwrap_or_else(|| handle.display());
        self.atomic(|c| {
            c.keep_friend(&user);
            c.set_connection(&user, |connection| {
                connection.name = name;
                connection.waiting = Some(yo_wire::b64(&code));
                connection.refused = false;
                connection.ending = None;
            });
            c.queue_connect(&user, CONNECT, code)
        })?;
        self.deliver_now()
    }

    /// Ends the connection of a service (user id): it forgets this user. Gone at once when the account is.
    pub fn disconnect_service(&mut self, user: &str) -> Result<Vec<Event>> {
        let result = self.disconnect_inner(user);
        self.finish_sent(result)
    }

    fn disconnect_inner(&mut self, user: &str) -> Result<()> {
        if self.replaced.get() {
            return Err(Error::Replaced);
        }
        if !self.state.storage.connections.contains_key(user) {
            return Err(Error::Protocol("not connected"));
        }
        let code: [u8; 32] = identity::random();
        let end = |connection: &mut Connection, ending: Option<String>| {
            connection.connected = false;
            connection.waiting = None;
            connection.refused = false;
            connection.ending = ending;
        };
        match self.check_sendable(Some(user)) {
            Err(Error::NotOnYo) => {
                self.set_connection(user, |connection| end(connection, None));
                return self.persist();
            }
            other => other?,
        }
        if !self.has_chat(user) {
            self.start_chat(&Handle::parse(user)?)?;
        }
        self.atomic(|c| {
            c.set_connection(user, |connection| end(connection, Some(yo_wire::b64(&code))));
            c.queue_connect(user, CONNECT_END, code)
        })?;
        self.deliver_now()
    }

    /// A service's answer to `user`: `CONNECTED`, `CONNECT_REFUSED` or `CONNECT_ENDED`, with the code it was asked by.
    pub fn answer_connection(&mut self, user: &str, kind: u8, code: [u8; 32]) -> Result<Vec<Event>> {
        let result = self.atomic(|c| c.queue_connect(user, kind, code)).and_then(|()| self.deliver_now());
        self.finish_sent(result)
    }

    fn queue_connect(&mut self, user: &str, kind: u8, code: [u8; 32]) -> Result<()> {
        self.accept_writing(user);
        self.queue_key(user)?;
        self.send_message(user, Content::Connect { kind, code })?;
        self.encrypt_waiting(user);
        self.persist()
    }

    /// A connection message of `user`: a service's answer counts only with the code it was asked by.
    pub(crate) fn connection_received(&mut self, user: &str, kind: u8, code: [u8; 32]) {
        if matches!(kind, CONNECT | CONNECT_END) {
            if !self.blocked(user) && self.connect_requests.len() < 256 {
                self.connect_requests.push((user.to_owned(), kind, code));
            }
            return;
        }
        let Some(connection) = self.state.storage.connections.get(user).map(|field| &field.value) else { return };
        let code = Some(yo_wire::b64(&code));
        let answers = match kind {
            CONNECTED | CONNECT_REFUSED => connection.waiting == code,
            CONNECT_ENDED => connection.ending == code,
            _ => false,
        };
        if !answers {
            return;
        }
        self.set_connection(user, |connection| match kind {
            CONNECTED => {
                connection.connected = true;
                connection.waiting = None;
            }
            CONNECT_REFUSED => {
                connection.refused = true;
                connection.waiting = None;
            }
            _ => connection.ending = None,
        });
        self.push_event(Event::ConnectionChanged { user: user.to_owned() });
    }
}
