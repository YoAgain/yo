use super::*;

impl Client {
    /// Makes `fresh` one-time prekeys of each kind and, with `rotate`, a new signed prekey and last resort (spec 3.1).
    pub(super) fn make_prekeys(&mut self, fresh: u32, rotate: bool) -> Result<api::Prekeys> {
        let now = (self.clock)();
        let mut next = || {
            self.state.prekeys.next_id += 1;
            self.state.prekeys.next_id
        };
        let ids: Vec<u32> = (0..fresh * 2 + 2).map(|_| next()).collect();
        let mut prekeys = api::Prekeys { signed: None, last_resort: None, one_time: Vec::new(), kyber: Vec::new() };
        if rotate {
            prekeys.signed = Some(self.records.new_signed(ids[0], now)?);
            prekeys.last_resort = Some(self.records.new_kyber(ids[1], true)?);
            self.state.prekeys.signed_at = now;
        }
        for pair in ids[2..].chunks(2) {
            prekeys.one_time.push(self.records.new_prekey(pair[0])?);
            prekeys.kyber.push(self.records.new_kyber(pair[1], false)?);
        }
        Ok(prekeys)
    }

    /// Keeps the hub's prekeys (spec 3.1): one-time ones refilled, signed and last resort replaced weekly. Saved before
    /// they leave and sent again as they are, so a lost answer costs no private keys.
    pub(super) fn refill(&mut self) -> Result<()> {
        let (body, fresh) = match self.state.prekeys.upload.clone() {
            Some(upload) => upload,
            None => {
                let left = self.state.prekeys.left;
                let rotate = (self.clock)().saturating_sub(self.state.prekeys.signed_at) >= ROTATE_EVERY_MS;
                if left >= PREKEY_REFILL_BELOW && !rotate {
                    // Nothing to upload; retired keys still go when their month is over (spec 3.1).
                    if self.forget_retired() {
                        self.persist()?;
                    }
                    return Ok(());
                }
                let fresh = if left < PREKEY_REFILL_BELOW { PREKEY_TARGET - left } else { 0 };
                // Saved in the same transaction: a save that fails leaves no upload to send unsaved.
                self.atomic(|c| {
                    let prekeys = c.make_prekeys(fresh, rotate)?;
                    let upload = (encode(&prekeys), fresh);
                    c.state.prekeys.upload = Some(upload.clone());
                    c.persist()?;
                    Ok(upload)
                })?
            }
        };
        let response = self.signed("PUT", api::PREKEYS, &body)?;
        // `409` to the same upload again: the hub took it before and its answer was lost. Either way it is done.
        if response.status != status::NO_CONTENT && response.status != status::CONFLICT {
            if !retryable(&refused(&response)) {
                // Made anew next time, with a new signed prekey and last resort too: a rotation it carried is still
                // due.
                self.state.prekeys.upload = None;
                self.state.prekeys.signed_at = 0;
            }
            return Err(refused(&response));
        }
        self.state.prekeys.upload = None;
        self.state.prekeys.left = self.state.prekeys.left.saturating_add(fresh);
        self.prekeys_known_ms = self.last_request_ms;
        // The hub hands out the newest signed prekey and last resort from now on: the ones before are retired.
        let now = self.wall();
        for kind in ["sp", "kl"] {
            let ids = self.records.ids(kind);
            for id in ids.iter().take(ids.len().saturating_sub(1)) {
                if !self.state.prekeys.retired.iter().any(|(k, i, _)| k == kind && i == id) {
                    self.state.prekeys.retired.push((kind.to_owned(), *id, now));
                }
            }
        }
        let _ = self.forget_retired();
        self.records.keep_newest_one_time(ONE_TIME_KEPT);
        Ok(())
    }

    /// Forgets retired prekeys older than `RETIRED_KEPT_MS`; whether any went.
    pub(super) fn forget_retired(&mut self) -> bool {
        let now = self.wall();
        let (old, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.state.prekeys.retired)
            .into_iter()
            .partition(|(_, _, at)| now.saturating_sub(*at) > RETIRED_KEPT_MS);
        let any = !old.is_empty();
        for (kind, id, _) in old {
            self.records.forget(&kind, id);
        }
        self.state.prekeys.retired = kept;
        any
    }
}
