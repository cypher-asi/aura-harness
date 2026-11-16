//! Layer: agent
//!
//! Gate, the router behind the `gate` model id.
//!
//! - [`router`]: the candidates the user's sign-ins allow and the model a
//!   turn goes to
//!
//! Gate never calls a model itself: it only chooses which model a turn
//! runs on, and the turn's calls go through the agent's kernel as usual.

pub mod router;

pub use router::{candidates, fixed_pick, pick_order, Candidate};
