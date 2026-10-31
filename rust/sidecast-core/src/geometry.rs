//! Display-local geometry in points (origin = top-left of the streamed display).

/// A position in display points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }

    pub fn distance(self, other: Point) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    pub fn midpoint(self, other: Point) -> Point {
        Point::new((self.x + other.x) / 2.0, (self.y + other.y) / 2.0)
    }
}

/// A size in display points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const ZERO: Size = Size { width: 0.0, height: 0.0 };

    pub const fn new(width: f64, height: f64) -> Self {
        Size { width, height }
    }

    /// Both dimensions are positive (and finite).
    pub fn is_usable(self) -> bool {
        self.width > 0.0 && self.height > 0.0 && self.width.is_finite() && self.height.is_finite()
    }
}
