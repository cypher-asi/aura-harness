//! Points, sizes, rectangles and colors of the window, in pixels.

/// A place in the window: `x` pixels right and `y` down from its
/// top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// A rectangle by its top-left corner and its size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rectangle {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rectangle {
    pub const fn new(top_left: Point, size: Size) -> Self {
        Self {
            x: top_left.x,
            y: top_left.y,
            width: size.width,
            height: size.height,
        }
    }

    pub fn center_x(&self) -> f32 {
        self.x + self.width / 2.0
    }

    pub fn center_y(&self) -> f32 {
        self.y + self.height / 2.0
    }

    /// Whether `point` is inside: the left and top edges are, the right
    /// and bottom ones are the next rectangle's.
    pub fn contains(&self, point: Point) -> bool {
        self.x <= point.x
            && point.x < self.x + self.width
            && self.y <= point.y
            && point.y < self.y + self.height
    }
}

/// A color, each channel from 0 to 1, not premultiplied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    /// An opaque color from its bytes.
    pub fn from_rgb8(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: 1.0,
        }
    }

    /// The color `t` of the way from this one to `other`, as opaque as
    /// this one.
    #[must_use]
    pub fn mix(self, other: Self, t: f32) -> Self {
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a,
        }
    }
}

/// `color` as RGBA bytes, not premultiplied.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn rgba(color: Color) -> [u8; 4] {
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    [byte(color.r), byte(color.g), byte(color.b), byte(color.a)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rectangle_holds_its_near_edges_and_not_its_far_ones() {
        let rect = Rectangle::new(Point::new(10.0, 20.0), Size::new(30.0, 40.0));
        assert!(rect.contains(Point::new(10.0, 20.0)));
        assert!(rect.contains(Point::new(39.9, 59.9)));
        assert!(!rect.contains(Point::new(40.0, 30.0)));
        assert!(!rect.contains(Point::new(20.0, 60.0)));
        assert!(!rect.contains(Point::new(9.9, 30.0)));
        assert_eq!((rect.center_x(), rect.center_y()), (25.0, 40.0));
    }

    #[test]
    fn colors_are_bytes_and_mix_between_each_other() {
        let teal = Color::from_rgb8(1, 244, 203);
        assert_eq!(rgba(teal), [1, 244, 203, 255]);
        let (black, white) = (Color::from_rgb8(0, 0, 0), Color::from_rgb8(255, 255, 255));
        assert_eq!(rgba(black.mix(white, 0.5)), [128, 128, 128, 255]);
        assert_eq!(white.mix(black, 0.0), white);
    }
}
