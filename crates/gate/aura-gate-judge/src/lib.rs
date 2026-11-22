//! Layer: gate
//!
//! Gate's requests to other models, and reading their replies.
//!
//! - [`classify`]: the request that labels a prompt with its category
//!   and how hard its work looks
//! - [`criteria`]: the acceptance criteria a routed code turn is written
//!   before its work starts, and what the review found of each
//! - [`turn`]: the judge of a failed turn, which says whether the model
//!   was at fault, and its reviews of unverified work and of work about
//!   to stand
//! - [`blind`]: the blind judge of `/compare`, which picks the better of
//!   two runs
//!
//! Gate never calls a model itself: it builds these requests, and the
//! calls go through the agent's kernel as usual.

pub mod blind;
pub mod classify;
pub mod criteria;
pub mod turn;
