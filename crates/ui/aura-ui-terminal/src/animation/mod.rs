//! Animation utilities for the terminal.

mod aura;
mod fade;
mod shimmer;
mod spinner;

pub use aura::{aura_ring, AURA_WIDTH};
pub(crate) use fade::{fade_area, fade_in};
pub use shimmer::shimmer_spans;
pub(crate) use shimmer::{mix, rgb};
pub use spinner::{Spinner, SpinnerStyle};
