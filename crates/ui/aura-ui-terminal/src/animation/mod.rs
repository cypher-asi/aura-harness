//! Animation utilities for the terminal.

mod aura;
mod shimmer;
mod spinner;

pub use aura::{aura_ring, AURA_WIDTH};
pub use shimmer::shimmer_spans;
pub use spinner::{Spinner, SpinnerStyle};
