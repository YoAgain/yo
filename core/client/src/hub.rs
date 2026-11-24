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

    /// Signs a request to send without the client; the signing clock is saved first (spec 6.2).
    pub(super) fn request(&mut self, method: &'static str, path: &'static str, body: Vec<u8>) -> Result<Request> {
        let (headers, _) = self.sign(method, path, &body);
        self.persist()?;
        Ok(Request { transport: self.transport.clone(), method, path, headers, body })
    }

    /// Whether to attest now (spec 6.11): registered, not attested yet, its wait over.
    pub fn attest_due(&self, now_ms: u64) -> bool {
        let schedule = &self.state.attestation;
        !self.state.user_id.is_empty() && !schedule.attested && now_ms >= schedule.next_try_ms
    }

    /// The signed challenge request, to send without the client.
    pub fn attest_challenge_begin(&mut self) -> Result<Request> {
        self.request("GET", api::ATTEST_CHALLENGE, Vec::new())
    }

    /// The signed attestation, to send without the client.
    pub fn attest_send_begin(&mut self, platform: u8, payload: Vec<u8>) -> Result<Request> {
        self.request("POST", api::ATTEST, encode(&api::Attest { platform, payload: payload.into() }))
    }

    /// The hub's challenge; `None`: attestation is off at the hub (`204`, or `404` from a hub before it).
    pub fn attest_challenge(&mut self) -> Result<Option<Vec<u8>>> {
        self.attest_challenge_begin()?.attest_challenge()
    }

    /// The hub's verdict on an attestation.
    pub fn attest_send(&mut self, platform: u8, payload: Vec<u8>) -> Result<u8> {
        self.attest_send_begin(platform, payload)?.attest_send()
    }

    /// The hub has attestation off: look again in a day, no attempt counted.
    pub fn attest_off(&mut self) -> Result<()> {
        self.state.attestation.next_try_ms = wall_ms().saturating_add(24 * HOUR_MS);
        self.persist()
    }

    /// Attested for good, or (failed, unjudged, not sent) the next wait.
    pub fn attest_outcome(&mut self, verdict: Option<u8>) -> Result<()> {
        if verdict == Some(api::ATTEST_ATTESTED) {
            self.state.attestation.attested = true;
            return self.persist();
        }
        self.attest_failed(wall_ms())
    }

    /// A failed attempt: waits 1 h, doubling, at most 7 days.
    pub fn attest_failed(&mut self, now_ms: u64) -> Result<()> {
        let schedule = &mut self.state.attestation;
        let hours = (1u64 << schedule.attempts.min(8)).min(7 * 24);
        schedule.next_try_ms = now_ms.saturating_add(hours * HOUR_MS);
        schedule.attempts = schedule.attempts.saturating_add(1);
        self.persist()
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
        crate::validate_roster(&named.found.devices)?;
        if handle.kind == yo_wire::handle::KIND_USER && handle.value.as_slice() != named.found.user_id {
            return Err(Error::Protocol("the hub answered for another user"));
        }
        Ok((named.found, spelled(named.username)))
    }
}

impl Request {
    pub fn attest_challenge(&self) -> Result<Option<Vec<u8>>> {
        let response = self.response()?;
        if matches!(response.status, status::NO_CONTENT | status::NOT_FOUND) {
            return Ok(None);
        }
        let challenge: api::AttestChallenge = decode(&expect(response, status::OK)?.body)?;
        if challenge.challenge.as_slice().len() != api::ATTEST_CHALLENGE_LEN {
            return Err(Error::Protocol("invalid attestation challenge"));
        }
        Ok(Some(challenge.challenge.into()))
    }

    pub fn attest_send(&self) -> Result<u8> {
        let result: api::AttestResult = decode(&expect(self.response()?, status::OK)?.body)?;
        Ok(result.verdict)
    }
}
