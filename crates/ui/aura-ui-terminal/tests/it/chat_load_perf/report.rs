//! The report lines a load test prints, and the units they are in.

use std::time::Duration;

pub(crate) fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

pub(crate) fn print_row(stage: &str, d: Duration) {
    println!("{stage:<46} {:>10.2} ms", ms(d));
}

/// `bytes` in kilobytes, for the report lines.
#[allow(clippy::cast_precision_loss)] // a chat is far below 2^52 bytes
pub(crate) fn kb(bytes: usize) -> f64 {
    bytes as f64 / 1024.0
}

/// `a / b`, for the report lines.
#[allow(clippy::cast_precision_loss)] // both counts are small
pub(crate) fn ratio(a: usize, b: usize) -> f64 {
    a as f64 / b.max(1) as f64
}

/// How many threads the chat lays blocks out on: `MOST_LAYOUT_THREADS` (8)
/// at most.
#[allow(clippy::cast_possible_truncation)] // clamped to 8 just below
pub(crate) fn threads() -> u32 {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(8) as u32
}
