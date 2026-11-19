//! Hooks for tests and the fuzz targets (feature `test-support`); the apps never call them.

use crate::*;
use std::sync::Mutex;

/// A state as saved: a restart (`Client::restore`) reads it through the database tables.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Saved {
    pub state: State,
    pub records: signal::Entries,
}

impl Saved {
    pub fn of(state: &State, records: &signal::Entries) -> Saved {
        Saved { state: state.clone(), records: records.clone() }
    }

    /// Written to the tables and read back, as a restart reads it.
    fn reread(&self) -> Result<(State, signal::Entries)> {
        statedb::round_trip(&self.state, &self.records)
    }
}

/// A store keeping the last state saved, in memory.
#[derive(Default)]
pub struct Memory(Mutex<Option<Saved>>);

impl Memory {
    pub fn last(&self) -> Option<Saved> {
        self.0.lock().unwrap().clone()
    }
}

impl Store for Memory {
    fn save_state(&self, state: &State, records: signal::Entries) -> std::result::Result<(), String> {
        *self.0.lock().unwrap() = Some(Saved::of(state, &records));
        Ok(())
    }
}

impl Client {
    /// The state as saved now.
    pub fn saved(&self) -> Saved {
        Saved::of(&self.state, &self.records.export())
    }

    /// The client a restart opens from `saved`.
    pub fn restore(transport: Arc<dyn Transport>, saved: &Saved) -> Result<Client> {
        let (state, records) = saved.reread()?;
        Ok(Client::from_snapshot(transport, state, records, false)?.0)
    }

    /// `restore` as another process of the device (an extension) opens it.
    pub fn restore_guest(transport: Arc<dyn Transport>, saved: &Saved) -> Result<Client> {
        let (state, records) = saved.reread()?;
        Ok(Client::from_snapshot(transport, state, records, true)?.0)
    }

    /// Takes in what another process saved meanwhile.
    pub fn reload(&mut self, saved: &Saved) -> Result<()> {
        let (state, records) = saved.reread()?;
        self.reload_snapshot(state, records)
    }

    /// The clock that decides when prekeys rotate and receipts are due.
    pub fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
    }

    /// The phone's clock `ms` off (spec 6.2: signed requests are within 5 minutes of the hub's).
    pub fn set_clock_skew(&mut self, ms: i64) {
        self.skew_ms = ms;
    }

    /// Every event handed out so far acknowledged.
    pub fn acknowledge_events(&mut self) {
        self.acknowledge_only(|_| true);
    }

    pub fn sender_certificate_for_tests(&self) -> Option<Vec<u8>> {
        self.state.sender_certificate.as_ref().map(|(bytes, _)| bytes.clone())
    }

    /// The sender certificate replaced (one from another root, an old one), as if fetched now.
    pub fn set_sender_certificate_for_tests(&mut self, bytes: Option<Vec<u8>>) {
        self.state.sender_certificate = bytes.map(|b| (b, (self.clock)()));
        self.state.certificate_clock = (self.clock)();
        self.certificate_asked_at = 0;
    }

    /// Any request, signed by this device, as only a broken client would send it.
    pub fn signed_for_tests(&mut self, method: &str, path: &str, body: &[u8]) -> Result<Response> {
        self.signed(method, path, body)
    }

    /// This account's access key as a delivery shows it (`Yo-Access`), once its profile key is made.
    pub fn access_key_for_tests(&self) -> Option<String> {
        self.state.profile_key.map(|key| yo_wire::b64(&access_key(&key)))
    }

    pub fn reset_recovery_limits_for_tests(&mut self) {
        self.state.roster_asked.clear();
        self.state.recovery_lookups = (0, 0);
    }

    /// One sealed envelope's payload processed as if it came from the inbox.
    pub fn receive_for_fuzzing(&mut self, payload: &[u8]) -> Result<()> {
        let opened = [&[0u8; 16][..], payload].concat();
        self.receive(&opened, &mut BTreeSet::new())
    }
}
