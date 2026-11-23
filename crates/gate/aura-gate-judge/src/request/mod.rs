//! What Gate's requests to other models share: how one is built, how
//! its prompt shows the user's text, and how its reply is read.
//!
//! - `ask`: the request itself, apart from the agent's own calls
//! - `cut`: long parts of a prompt cut to their start, their end or both
//! - `fence`: the user's prompt normalized and fenced as text to read
//! - `reply`: a reply's tool calls, and its text after a local model's thinking

mod ask;
mod cut;
mod fence;
mod reply;
#[cfg(test)]
mod tests;

pub(crate) use ask::{answers_in_text, meanings, Ask};
pub(crate) use cut::{cut, or_blank, tail};
pub(crate) use fence::fenced;
pub(crate) use reply::{after_thinking, answered, calls, Answered};
