//! The sign-up name field (spec 2.2, 4.2), one state machine for every app. No I/O: the app (or the bindings)
//! runs the device check and the hub's name checks and feeds the answers in.
//!
//! - Empty: nothing shows.
//! - Checking: only a spinner, the hint stays empty. A result shows only once the answer for exactly the current
//!   text is back; answers for an older text are dropped, so the field never flips between states.
//! - Forbidden characters are judged here, at once, without asking the hub; they stay typed.
//! - The device check (attestation) starts early (welcome screen). Until it is back the name check waits; any
//!   failure, error or `DEVICE_TIMEOUT_MS` counts as not verifiable: the hub's minimum is then 12. It never
//!   blocks sign-up.
//! - Sign Up while checking is pending; the arriving answer decides (go, or shake).

use crate::Attested;
use unicode_segmentation::UnicodeSegmentation;
use yoip::net::api::status;
use yoip::net::handle;

/// How long the device check may take before the name is checked as not verifiable.
pub const DEVICE_TIMEOUT_MS: u64 = 3000;
/// The minimum assumed when the hub names none (a hub from before `Yo-Name-Min`): the longer one.
pub const FALLBACK_MIN: u32 = 12;

/// Where the device check stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Device {
    /// Started at this time (ms).
    Checking(u64),
    Attested(Attested),
    NotVerifiable,
}

/// What the field shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Empty,
    /// Spinner only.
    Checking,
    /// Free: the checkmark; `Minimum N characters` under it.
    Ok,
    /// Shorter than the hub's minimum.
    TooShort,
    /// Taken or reserved (the hub tells them apart to no one).
    Taken,
    /// Characters a name cannot have (`bad`, as typed, each once), or a form the hub refuses (no letter, `_`
    /// first, too long) with `bad` empty.
    Invalid,
    /// No answer from the hub, or asked too often.
    Offline,
}

/// The field's state for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub phase: Phase,
    /// The hub's minimum for this device, once known (shown as the hint with `Ok`, `TooShort`, `Taken`).
    pub min: Option<u32>,
    pub bad: Vec<String>,
    /// Sign Up was tapped while checking: the button shows a spinner.
    pub signup_pending: bool,
}

/// A name check to run at the hub: its number, the text, the attested challenge to name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub seq: u64,
    pub text: String,
    pub challenge: Option<Vec<u8>>,
}

/// What a tap on Sign Up does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignUp {
    Go,
    /// Checking: the answer decides (`answered` returns it).
    Wait,
    Shake,
}

#[derive(Debug, Clone)]
pub struct NameField {
    text: String,
    seq: u64,
    device: Device,
    /// The answered phase for `text`, with the minimum; `None` while checking.
    result: Option<(Phase, Option<u32>)>,
    bad: Vec<String>,
    pending: bool,
    /// How long the device check took (ms), once done.
    pub device_ms: Option<u64>,
}

impl NameField {
    /// A field whose device check starts now.
    pub fn new(now_ms: u64) -> Self {
        NameField {
            text: String::new(),
            seq: 0,
            device: Device::Checking(now_ms),
            result: None,
            bad: Vec::new(),
            pending: false,
            device_ms: None,
        }
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The attestation for `Register`, if the device attested.
    pub fn attestation(&self) -> Option<Attested> {
        match &self.device {
            Device::Attested(a) => Some(a.clone()),
            _ => None,
        }
    }

    /// The device check is done: `None` for any failure. Returns the name check now due, if one waited.
    pub fn device_done(&mut self, attested: Option<Attested>, now_ms: u64) -> Option<Query> {
        let Device::Checking(started) = self.device else { return None };
        self.device_ms = Some(now_ms.saturating_sub(started));
        self.device = match attested {
            Some(a) if !a.payload.is_empty() => Device::Attested(a),
            _ => Device::NotVerifiable,
        };
        self.due()
    }

    /// The clock: a device check past `DEVICE_TIMEOUT_MS` counts as not verifiable.
    pub fn tick(&mut self, now_ms: u64) -> Option<Query> {
        match self.device {
            Device::Checking(started) if now_ms.saturating_sub(started) >= DEVICE_TIMEOUT_MS => {
                self.device_done(None, now_ms)
            }
            _ => None,
        }
    }

    /// The text changed: the name check to run, if one is due now (it waits for the device check otherwise).
    pub fn typed(&mut self, text: &str) -> Option<Query> {
        if text == self.text && (self.result.is_some() || self.seq > 0) {
            return None;
        }
        self.text = text.to_owned();
        self.seq += 1;
        self.result = None;
        self.bad.clear();
        if text.is_empty() {
            self.pending = false;
            return None;
        }
        let bad = forbidden(text);
        if !bad.is_empty() {
            self.bad = bad;
            self.result = Some((Phase::Invalid, None));
            return None;
        }
        self.due()
    }

    /// The current check's number.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The name check due now, if any (the text known, the device check back, no answer yet).
    pub fn due_query(&self) -> Option<Query> {
        self.due()
    }

    fn due(&self) -> Option<Query> {
        if self.text.is_empty() || self.result.is_some() || matches!(self.device, Device::Checking(_)) {
            return None;
        }
        let challenge = self.attestation().map(|a| a.challenge);
        Some(Query { seq: self.seq, text: self.text.clone(), challenge })
    }

    /// The hub's answer to query `seq`: `Err` for no answer. Dropped for an older text. Returns the decision of a
    /// pending Sign Up.
    pub fn answered(&mut self, seq: u64, answer: Result<(u16, Option<u32>), ()>) -> Option<SignUp> {
        if seq != self.seq || self.result.is_some() || self.text.is_empty() {
            return None;
        }
        let min = match &answer {
            Ok((_, Some(min))) => Some(*min),
            Ok((_, None)) => Some(FALLBACK_MIN),
            Err(()) => None,
        };
        let phase = match answer {
            Ok((status::NO_CONTENT, _)) => Phase::Ok,
            Ok((status::CONFLICT, _)) => Phase::Taken,
            Ok((status::BAD_REQUEST, _)) => match handle::check_username_form(&self.text) {
                Ok(()) => Phase::TooShort,
                Err(_) => Phase::Invalid,
            },
            _ => Phase::Offline,
        };
        // A hub before `Yo-Name-Min` that says free: the name is long enough for it.
        self.result = Some((phase, min));
        if std::mem::take(&mut self.pending) {
            return Some(if self.result.as_ref().is_some_and(|(p, _)| *p == Phase::Ok) {
                SignUp::Go
            } else {
                SignUp::Shake
            });
        }
        None
    }

    /// Sign Up tapped.
    pub fn sign_up(&mut self) -> SignUp {
        match &self.result {
            Some((Phase::Ok, _)) => SignUp::Go,
            Some(_) => SignUp::Shake,
            None if self.text.is_empty() => SignUp::Shake,
            None => {
                self.pending = true;
                SignUp::Wait
            }
        }
    }

    pub fn view(&self) -> View {
        let (phase, min) = match (&self.result, self.text.is_empty()) {
            (_, true) => (Phase::Empty, None),
            (None, false) => (Phase::Checking, None),
            (Some((phase, min)), false) => (phase.clone(), *min),
        };
        View { phase, min, bad: self.bad.clone(), signup_pending: self.pending }
    }
}

/// The characters a name cannot have, as typed, each once (`_` first is a form problem, not a character).
pub fn forbidden(text: &str) -> Vec<String> {
    let mut bad: Vec<String> = Vec::new();
    for g in text.graphemes(true) {
        if g != "_" && handle::canonical(g).is_empty() && !bad.iter().any(|b| b == g) {
            bad.push(g.to_owned());
        }
    }
    bad
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attested() -> Attested {
        Attested { platform: 1, payload: vec![1], challenge: vec![9; 32] }
    }

    #[test]
    fn waits_for_the_device_then_checks_with_its_challenge() {
        let mut f = NameField::new(0);
        assert_eq!(f.typed("Anna"), None, "the device check is not back");
        assert_eq!(f.view().phase, Phase::Checking);
        let q = f.device_done(Some(attested()), 400).unwrap();
        assert_eq!((q.text.as_str(), q.challenge), ("Anna", Some(vec![9; 32])));
        assert_eq!(f.device_ms, Some(400));
        f.answered(q.seq, Ok((204, Some(4))));
        assert_eq!(f.view(), View { phase: Phase::Ok, min: Some(4), bad: vec![], signup_pending: false });
        assert_eq!(f.attestation(), Some(attested()));
    }

    #[test]
    fn any_failure_or_timeout_is_not_verifiable() {
        let mut f = NameField::new(0);
        f.typed("Anna");
        assert_eq!(f.tick(DEVICE_TIMEOUT_MS - 1), None);
        let q = f.tick(DEVICE_TIMEOUT_MS).unwrap();
        assert_eq!(q.challenge, None);
        assert_eq!(f.device(), &Device::NotVerifiable);
        assert_eq!(f.device_done(Some(attested()), 5000), None, "too late: stays not verifiable");
        f.answered(q.seq, Ok((400, Some(12))));
        assert_eq!((f.view().phase, f.view().min), (Phase::TooShort, Some(12)));
        let mut g = NameField::new(0);
        g.device_done(Some(Attested { payload: vec![], ..attested() }), 1);
        assert_eq!(g.attestation(), None, "an empty payload is no attestation");
    }

    #[test]
    fn stale_answers_are_dropped() {
        let mut f = NameField::new(0);
        f.device_done(None, 0);
        let first = f.typed("Ann").unwrap();
        let second = f.typed("Anna").unwrap();
        assert_eq!(f.answered(first.seq, Ok((409, Some(12)))), None);
        assert_eq!(f.view().phase, Phase::Checking, "the older text's answer shows nothing");
        f.answered(second.seq, Ok((204, Some(12))));
        assert_eq!(f.view().phase, Phase::Ok);
        f.answered(second.seq, Ok((409, Some(12))));
        assert_eq!(f.view().phase, Phase::Ok, "one answer per text");
    }

    #[test]
    fn forbidden_characters_stay_typed_and_need_no_hub() {
        let mut f = NameField::new(0);
        f.device_done(None, 0);
        assert_eq!(f.typed("An na@x.y@"), None);
        let v = f.view();
        assert_eq!((v.phase, v.bad), (Phase::Invalid, vec![" ".into(), "@".into(), ".".into()]));
        assert_eq!(f.sign_up(), SignUp::Shake);
        assert!(forbidden("Anna_1🦄").is_empty());
        let q = f.typed("_Anna").unwrap();
        f.answered(q.seq, Ok((400, Some(12))));
        assert_eq!((f.view().phase, f.view().bad), (Phase::Invalid, vec![]), "a form problem, no characters");
    }

    #[test]
    fn sign_up_while_checking_is_decided_by_the_answer() {
        let mut f = NameField::new(0);
        assert_eq!(f.sign_up(), SignUp::Shake, "empty");
        f.typed("Annabelle");
        assert_eq!(f.sign_up(), SignUp::Wait);
        assert!(f.view().signup_pending);
        let q = f.device_done(None, 10).unwrap();
        assert_eq!(f.answered(q.seq, Ok((409, Some(12)))), Some(SignUp::Shake));
        assert!(!f.view().signup_pending);
        let q = f.typed("Annabelle1234").unwrap();
        assert_eq!(f.sign_up(), SignUp::Wait);
        assert_eq!(f.answered(q.seq, Ok((204, Some(12)))), Some(SignUp::Go));
        assert_eq!(f.sign_up(), SignUp::Go);
    }

    #[test]
    fn no_answer_is_offline_and_an_old_hub_means_twelve() {
        let mut f = NameField::new(0);
        f.device_done(None, 0);
        let q = f.typed("Anna").unwrap();
        f.answered(q.seq, Err(()));
        assert_eq!((f.view().phase, f.view().min), (Phase::Offline, None));
        let q = f.typed("Annabel").unwrap();
        f.answered(q.seq, Ok((429, None)));
        assert_eq!((f.view().phase, f.view().min), (Phase::Offline, Some(FALLBACK_MIN)));
        assert_eq!(f.typed(""), None);
        assert_eq!(f.view().phase, Phase::Empty);
    }
}
