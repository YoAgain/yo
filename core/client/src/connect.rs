use super::*;

impl Client {
    /// Users who connected this service, with their codes; never app events or synced content.
    pub fn take_connect_requests(&mut self) -> Vec<(String, [u8; 32])> {
        std::mem::take(&mut self.connect_requests)
    }

    /// Connects `service` with another one: a contact from now on, told the code of its connect link.
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
        let user = self.send_target(&handle)?;
        self.check_sendable(user.as_deref())?;
        let user = match user.filter(|u| self.has_chat(u)) {
            Some(user) => user,
            None => self.start_chat(&handle)?,
        };
        self.atomic(|c| {
            c.accept_writing(&user);
            c.keep_friend(&user);
            c.queue_key(&user)?;
            c.send_message(&user, Content::Connect(code))?;
            c.encrypt_waiting(&user);
            c.persist()
        })?;
        self.deliver_now()
    }
}
