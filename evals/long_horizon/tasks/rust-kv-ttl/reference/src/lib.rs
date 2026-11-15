//! A tiny in-memory key-value store with a Redis-like text protocol.

pub mod clock;
pub mod command;
pub mod session;
pub mod store;

pub use clock::{Clock, ManualClock, SystemClock};
pub use command::{execute, parse, Command, ParseError, Response};
pub use session::Session;
pub use store::Store;
