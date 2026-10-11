//! Integer points and rectangles in sprite pixel coordinates.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// A rectangle of whole pixels. It is empty when its width or height is zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const EMPTY: Self = Self::new(0, 0, 0, 0);

    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// A rectangle at the origin.
    pub const fn from_size(w: u32, h: u32) -> Self {
        Self::new(0, 0, w, h)
    }

    /// The rectangle covering columns `left..right` and rows `top..bottom`.
    pub fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        if right <= left || bottom <= top {
            return Self::EMPTY;
        }
        Self::new(left, top, (right - left) as u32, (bottom - top) as u32)
    }

    pub const fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// One past the last column.
    pub const fn right(self) -> i32 {
        self.x + self.w as i32
    }

    /// One past the last row.
    pub const fn bottom(self) -> i32 {
        self.y + self.h as i32
    }

    pub const fn origin(self) -> Point {
        Point::new(self.x, self.y)
    }

    pub const fn area(self) -> usize {
        self.w as usize * self.h as usize
    }

    pub const fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    /// The part both rectangles cover; [`Rect::EMPTY`] if they do not overlap.
    pub fn intersect(self, other: Self) -> Self {
        if self.is_empty() || other.is_empty() {
            return Self::EMPTY;
        }
        Self::from_edges(
            self.x.max(other.x),
            self.y.max(other.y),
            self.right().min(other.right()),
            self.bottom().min(other.bottom()),
        )
    }

    /// The smallest rectangle covering both. An empty rectangle contributes nothing.
    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        Self::from_edges(
            self.x.min(other.x),
            self.y.min(other.y),
            self.right().max(other.right()),
            self.bottom().max(other.bottom()),
        )
    }

    /// The same rectangle moved by `(dx, dy)`.
    pub const fn translated(self, dx: i32, dy: i32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.w, self.h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_and_containment() {
        let r = Rect::new(2, 3, 4, 5);
        assert_eq!((r.right(), r.bottom()), (6, 8));
        assert!(r.contains(Point::new(2, 3)));
        assert!(r.contains(Point::new(5, 7)));
        assert!(!r.contains(Point::new(6, 7)));
        assert!(!r.contains(Point::new(1, 3)));
        assert_eq!(r.area(), 20);
    }

    #[test]
    fn intersect() {
        let a = Rect::new(0, 0, 10, 10);
        assert_eq!(a.intersect(Rect::new(5, 5, 10, 10)), Rect::new(5, 5, 5, 5));
        assert_eq!(a.intersect(Rect::new(-5, -5, 7, 7)), Rect::new(0, 0, 2, 2));
        assert_eq!(a.intersect(Rect::new(10, 0, 5, 5)), Rect::EMPTY);
        assert_eq!(a.intersect(Rect::EMPTY), Rect::EMPTY);
    }

    #[test]
    fn union_ignores_empty_rectangles() {
        let a = Rect::new(0, 0, 2, 2);
        let b = Rect::new(5, -1, 1, 1);
        assert_eq!(a.union(b), Rect::new(0, -1, 6, 3));
        assert_eq!(a.union(Rect::new(100, 100, 0, 5)), a);
        assert_eq!(Rect::EMPTY.union(b), b);
    }

    #[test]
    fn from_edges_collapses_inverted_rectangles() {
        assert_eq!(Rect::from_edges(3, 3, 3, 9), Rect::EMPTY);
        assert_eq!(Rect::from_edges(5, 0, 2, 9), Rect::EMPTY);
        assert_eq!(Rect::from_edges(-2, -2, 1, 0), Rect::new(-2, -2, 3, 2));
    }
}
