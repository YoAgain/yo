//! The gatekeeper: what a service may (its TOML), what a script may ask for (actions), how often (limits). Pure
//! rules, no I/O: nothing reaches Yo past it.

pub mod action;
pub mod header;
pub mod limits;

pub use action::{Action, Asked};
pub use header::{Header, Trigger};
pub use limits::Limits;
