#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The hub could not be reached; nothing was lost, try again.
    #[error("network: {0}")]
    Network(String),
    /// The hub refused this device's time though corrected by the hub's (spec 4.1), or the phone cannot reach the
    /// hub for its date (TLS): the user should check the phone's date and time. Nothing was lost, try again.
    #[error("the phone's clock is off")]
    Clock,
    /// The hub refused with this status.
    #[error("refused: {0}")]
    Refused(u16),
    /// No one on Yo has this handle.
    #[error("not on Yo")]
    NotOnYo,
    #[error("blocked")]
    Blocked,
    /// A Yo to a group with no other member left: nobody to send it to (leave it instead).
    #[error("the group is empty")]
    GroupEmpty,
    /// The contact's app is too old for sessions: the message waits until it is updated (spec 3.1).
    #[error("the contact must update Yo")]
    Outdated,
    /// The contact was verified and its account key changed: confirm the new key first (spec 2.3).
    #[error("identity not confirmed")]
    Unconfirmed,
    /// This device was replaced by another registration of the account.
    #[error("this device was replaced")]
    Replaced,
    /// Something the peer or the hub sent breaks the protocol.
    #[error("protocol: {0}")]
    Protocol(&'static str),
    /// libsignal refused: the message does not fit the session, or a bundle is bad.
    #[error("session: {0}")]
    Session(String),
    /// The stored state cannot be opened with this key, or is damaged.
    #[error("state")]
    State,
    /// The state could not be saved (`Store::save`): nothing that needed it left (spec 3.2).
    #[error("save: {0}")]
    Save(String),
    #[error("internal: {0}")]
    Internal(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<yoip::Error> for Error {
    fn from(_: yoip::Error) -> Self {
        Error::Protocol("format")
    }
}
