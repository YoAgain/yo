//! What the app shows under Settings › Encryption as live checks: the keys this device keeps at the hub, its delivery
//! tokens, and how its sessions and contacts stand. Facts only, read from the state; the app decides the colours.

use crate::Client;

/// A device's security as it stands now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecurityStatus {
    /// One-time prekeys (of the scarcer kind, classic or Kyber) the hub still has to hand out for this device.
    pub prekeys_at_hub: u32,
    /// A refill made and not yet taken by the hub (sent again with the next sync).
    pub refill_pending: bool,
    /// Days since the signed prekey and the Kyber last resort were last replaced (weekly, spec 4.2).
    pub signed_prekey_days: u64,
    /// Privacy Pass tokens in stock: deliveries that count against no address (spec 6.10).
    pub delivery_tokens: u32,
    /// Devices of contacts (not blocked, not gone) this device writes to.
    pub devices: u32,
    /// Of them, those with a session good to send on that was made with PQXDH (Kyber): libsignal's own check.
    pub pq_sessions: u32,
    /// Contacts (not blocked, not gone).
    pub contacts: u32,
    /// Of them, those whose safety number was compared (verified).
    pub verified: u32,
}

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

impl SecurityStatus {
    /// Whole days from `then` to `now` (ms); 0 for a time not set or in the future.
    pub fn days(then: u64, now: u64) -> u64 {
        if then == 0 {
            return 0;
        }
        now.saturating_sub(then) / DAY_MS
    }
}

impl Client {
    pub fn security_status(&self) -> SecurityStatus {
        let mut status = SecurityStatus {
            prekeys_at_hub: self.state.prekeys.left,
            refill_pending: self.state.prekeys.upload.is_some(),
            signed_prekey_days: SecurityStatus::days(self.state.prekeys.signed_at, (self.clock)()),
            delivery_tokens: self.state.tokens.len() as u32,
            ..SecurityStatus::default()
        };
        for (user, contact) in self.state.contacts.iter().filter(|(_, c)| !c.blocked && !c.gone && c.chat) {
            status.contacts += 1;
            status.verified += contact.verified as u32;
            for device in &contact.devices {
                status.devices += 1;
                let usable = self.address(user, device.slot).is_ok_and(|a| self.records.usable(&a));
                status.pq_sessions += usable as u32;
            }
        }
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_since_a_replacement() {
        assert_eq!(SecurityStatus::days(0, 10 * DAY_MS), 0, "never: nothing to count");
        assert_eq!(SecurityStatus::days(DAY_MS, DAY_MS + DAY_MS - 1), 0);
        assert_eq!(SecurityStatus::days(DAY_MS, 8 * DAY_MS), 7);
        assert_eq!(SecurityStatus::days(5 * DAY_MS, DAY_MS), 0, "a clock gone back: none");
    }
}
