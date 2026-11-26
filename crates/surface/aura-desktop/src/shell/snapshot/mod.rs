//! Whole screens of the window, drawn offscreen by the CPU rasterizer,
//! so the terminal UI, the painter and the input path are exercised
//! together without a display or a GPU; and the scenes the benchmarks
//! in [`super::bench`] time.
//!
//! Set `AURA_DESKTOP_SNAPSHOTS=<dir>` to also write each screen as a PNG
//! with its text beside it.
//!
//! Set `AURA_DESKTOP_GPU=1` on a machine with a graphics adapter to draw
//! each screen with [`crate::gpu`] as well, and check it against the CPU
//! rasterizer's picture of the same quads.

mod fixtures;
mod flows;
mod graph;
mod screens;
mod support;
mod tabs;

pub use fixtures::{agent, seed, steps};
pub use flows::flow;
pub use support::report;
