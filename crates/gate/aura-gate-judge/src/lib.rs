//! Layer: gate
//!
//! Gate's requests to other models, and reading their replies.
//!
//! - [`classify`]: the request that labels a prompt with its category and how hard its work looks
//! - [`criteria`]: the acceptance criteria a routed code turn is held to, and what its review found
//! - [`turn`]: the judge of a failed turn, and the reviews of unverified and of standing work
//! - [`blind`]: the blind judge of `/compare`, which picks the better of two runs
//! - `request`: what the requests share, from building one to reading its reply
//!
//! Gate never calls a model itself: it builds these requests, and the
//! calls go through the agent's kernel as usual.

pub mod blind;
pub mod classify;
pub mod criteria;
mod request;
pub mod turn;
