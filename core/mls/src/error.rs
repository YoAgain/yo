#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The hub could not be reached; nothing was lost, try again.
    #[error("network: {0}")]
    Network(String),
    /// The hub refused with this status.
    #[error("refused: {0}")]
    Refused(u16),
    #[error("not a phone number")]
    Phone,
    /// The number is not on Yo.
    #[error("not on Yo")]
    NotOnYo,
    #[error("blocked")]
    Blocked,
    /// The contact was verified and its account key changed: confirm the new key first (spec 3.4).
    #[error("identity not confirmed")]
    Unconfirmed,
    /// This device was replaced by another registration of the number.
    #[error("this device was replaced")]
    Replaced,
    /// Something the peer or the hub sent breaks the protocol.
    #[error("protocol: {0}")]
    Protocol(&'static str),
    /// OpenMLS refused: the message or state does not fit the group.
    #[error("mls: {0}")]
    Mls(String),
    /// The stored state cannot be opened with this key, or is damaged.
    #[error("state")]
    State,
    #[error("internal: {0}")]
    Internal(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<yo_wire::Error> for Error {
    fn from(error: yo_wire::Error) -> Self {
        match error {
            yo_wire::Error::Phone => Error::Phone,
            _ => Error::Protocol("format"),
        }
    }
}
