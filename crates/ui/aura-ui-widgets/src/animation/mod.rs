//! Animation utilities for the terminal.

mod aura;
mod fade;
mod shimmer;
mod spinner;

pub use aura::{aura_ring, AURA_WIDTH};
pub use fade::{fade_area, fade_in};
pub(crate) use shimmer::{mix, rgb};
pub use shimmer::{shimmer_pass_frames, shimmer_spans};
pub use spinner::{Spinner, SpinnerStyle};
