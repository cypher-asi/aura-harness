//! Points, sizes, rectangles and colors of the window, in pixels.
//!
//! The bare renderer takes these from here rather than from iced, so
//! this file is the one place that changes when iced goes.

pub use iced::{Color, Point, Rectangle, Size};

/// `color` as RGBA bytes, not premultiplied.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn rgba(color: Color) -> [u8; 4] {
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    [byte(color.r), byte(color.g), byte(color.b), byte(color.a)]
}
