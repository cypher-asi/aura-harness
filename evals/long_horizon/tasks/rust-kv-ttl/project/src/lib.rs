//! A tiny in-memory key-value store with a Redis-like text protocol.

pub mod command;
pub mod store;

pub use command::{execute, parse, Command, ParseError, Response};
pub use store::Store;
