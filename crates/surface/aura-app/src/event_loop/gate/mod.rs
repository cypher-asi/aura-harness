//! Gate's runtime in the app: what the turns it routes do between their
//! model calls, over [`aura_gate`]'s pure parts.
//!
//! - [`followup`]: what an attempt does when it ends with no check of its own

pub(crate) mod followup;
