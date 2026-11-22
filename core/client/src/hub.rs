use super::*;

impl Client {
    /// The headers that sign a request, and its time.
    pub(super) fn sign(&mut self, method: &str, path: &str, body: &[u8]) -> (Vec<(String, String)>, u64) {
        self.sign_hash(method, path, &sha256(body))
    }

    /// `sign` by the body's hash.
    pub(super) fn sign_hash(&mut self, method: &str, path: &str, body_hash: &[u8; 32]) -> (Vec<(String, String)>, u64) {
        // Each signature once (the hub's replay cache): strictly increasing times across processes and restarts
        // (`signed_until` is saved). A floor far ahead of this clock is dropped so a clock set back does not stall
        // every request; a time the hub saw already is answered `401`, and `signed` signs again.
        let wall = self.wall();
        let floor = self.last_request_ms.max(self.state.signed_until);
        let time = if floor >= wall + SIGN_AHEAD_MS { wall } else { wall.max(floor + 1) };
        self.last_request_ms = time;
        self.state.signed_until = time;
        let secret = self.state.auth_secret;
        let mut headers = vec![(HEADER_DEVICE.to_owned(), yo_wire::b64(&self.state.device_id))];
        headers.extend(signature_headers(|m| identity::sign_with(&secret, m), method, path, time, body_hash));
        (headers, time)
    }

    /// A signed request; a `401` is retried once, signed anew (the same millisecond from another process makes the same
    /// signature, which the hub takes once; a refused request was not processed), by the hub's time if it told it.
    pub(super) fn signed(&mut self, method: &str, path: &str, body: &[u8]) -> Result<Response> {
        let (headers, _) = self.sign(method, path, body);
        let response = self.unsigned(method, path, &headers, body)?;
        if response.status != status::UNAUTHORIZED {
            return Ok(response);
        }
        let (headers, _) = self.sign(method, path, body);
        self.signed_again(self.unsigned(method, path, &headers, body)?)
    }

    /// The answer to a request signed anew after a `401`: the time refused again though corrected by the hub's, the
    /// phone's clock is off beyond what this can mend (`Error::Clock`).
    pub(super) fn signed_again(&self, response: Response) -> Result<Response> {
        match response.status == status::UNAUTHORIZED && yo_wire::request::clock_of(&response.body).is_some() {
            true => Err(Error::Clock),
            false => Ok(response),
        }
    }

    pub(super) fn unsigned(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<Response> {
        let response = self.transport.request(method, path, headers, body).map_err(Error::Network)?;
        if headers.iter().any(|(name, _)| name.eq_ignore_ascii_case(HEADER_DEVICE)) {
            self.hub_told_time(&response);
        }
        if response.status == status::GONE && headers.iter().any(|(name, _)| name.eq_ignore_ascii_case(HEADER_DEVICE)) {
            self.replaced.set(true);
        }
        Ok(response)
    }

    pub fn set_push(&mut self, push: Push) -> Result<()> {
        let response = self.signed("PUT", api::PUSH, &encode(&push))?;
        // Its signing time kept (spec 6.3: one clock for every process of the device).
        self.persist()?;
        expect(response, status::NO_CONTENT).map(drop)
    }

    /// A lookup (spec 6.3): who a handle is and its username; with `LOOKUP_PREKEYS`, a bundle unless in `except`.
    pub(super) fn lookup(
        &mut self,
        handle: &Handle,
        flags: u8,
        except: Vec<Key>,
    ) -> Result<(api::LookupResult, Option<String>)> {
        let body = encode(&api::Lookup { handle: handle.clone(), flags: flags | api::LOOKUP_USERNAME, except });
        let nonce = self.hub_nonce();
        let (headers, _) = self.sign("POST", api::LOOKUP, &body);
        let response = self.unsigned("POST", api::LOOKUP, &with_nonce(headers, nonce), &body)?;
        let response = match response.status {
            status::UNAUTHORIZED => {
                let (headers, _) = self.sign("POST", api::LOOKUP, &body);
                self.signed_again(self.unsigned("POST", api::LOOKUP, &with_nonce(headers, nonce), &body)?)?
            }
            _ => response,
        };
        match response.status {
            status::OK => {}
            status::NOT_FOUND => return Err(Error::NotOnYo),
            _ => return Err(refused(&response)),
        }
        let answer = hub_signed(&self.hub_keys, nonce.as_ref(), api::LOOKUP, &body, &response.body)?;
        let named: api::LookupNamed = decode(answer)?;
        crate::one_device(&named.found.devices)?;
        Ok((named.found, spelled(named.username)))
    }
}
