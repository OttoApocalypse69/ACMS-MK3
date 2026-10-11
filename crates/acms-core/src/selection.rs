//! The selection: a mask saying which canvas pixels tools may change.

use crate::color::Rgba8;
use crate::geom::{Point, Rect};
use crate::image::Image;
use crate::stroke::line;

/// How a new selection combines with the one that is already there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Combine {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}

/// One bit per canvas pixel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    width: u32,
    height: u32,
    bits: Vec<bool>,
    /// The smallest rectangle containing every selected pixel.
    bounds: Rect,
}

impl Mask {
    /// A mask of the given canvas size selecting the pixels for which `selected` is true.
    pub fn from_fn(width: u32, height: u32, mut selected: impl FnMut(i32, i32) -> bool) -> Self {
        let mut bits = Vec::with_capacity(width as usize * height as usize);
        let mut bounds = Rect::EMPTY;
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                let bit = selected(x, y);
                if bit {
                    bounds = bounds.union(Rect::new(x, y, 1, 1));
                }
                bits.push(bit);
            }
        }
        Self {
            width,
            height,
            bits,
            bounds,
        }
    }

    pub fn empty(width: u32, height: u32) -> Self {
        Self::from_fn(width, height, |_, _| false)
    }

    pub fn full(width: u32, height: u32) -> Self {
        Self::from_fn(width, height, |_, _| true)
    }

    /// The part of `rect` that lies on the canvas.
    pub fn from_rect(width: u32, height: u32, rect: Rect) -> Self {
        Self::from_fn(width, height, |x, y| rect.contains(Point::new(x, y)))
    }

    /// The pixels inside a closed outline, plus the outline itself, so that even a thin
    /// lasso selects what it was drawn over.
    pub fn from_polygon(width: u32, height: u32, points: &[Point]) -> Self {
        let mut mask = Self::from_fn(width, height, |x, y| {
            // Even-odd rule, tested at the pixel's centre.
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut inside = false;
            for (index, a) in points.iter().enumerate() {
                let b = points[(index + 1) % points.len()];
                let (ax, ay, bx, by) = (
                    a.x as f32 + 0.5,
                    a.y as f32 + 0.5,
                    b.x as f32 + 0.5,
                    b.y as f32 + 0.5,
                );
                if (ay > py) != (by > py) && px < (bx - ax) * (py - ay) / (by - ay) + ax {
                    inside = !inside;
                }
            }
            inside
        });
        for (index, &a) in points.iter().enumerate() {
            let b = points[(index + 1) % points.len()];
            for p in line(a, b) {
                mask.set(p.x, p.y, true);
            }
        }
        mask
    }

    /// The pixels of `image` (canvas-sized) that look like the one at `start`: no channel
    /// differs by more than `tolerance`. With `contiguous`, only those connected to `start`.
    pub fn similar(image: &Image, start: Point, tolerance: u8, contiguous: bool) -> Self {
        let (width, height) = (image.width(), image.height());
        if !image.bounds().contains(start) {
            return Self::empty(width, height);
        }
        let target = image.get(start.x, start.y);
        let close = |color: Rgba8| {
            let diff = |a: u8, b: u8| a.abs_diff(b) <= tolerance;
            diff(color.r, target.r)
                && diff(color.g, target.g)
                && diff(color.b, target.b)
                && diff(color.a, target.a)
        };
        if !contiguous {
            return Self::from_fn(width, height, |x, y| close(image.get(x, y)));
        }

        let mut mask = Self::empty(width, height);
        let mut stack = vec![start];
        while let Some(p) = stack.pop() {
            if !image.bounds().contains(p) || mask.contains(p.x, p.y) || !close(image.get(p.x, p.y))
            {
                continue;
            }
            mask.set(p.x, p.y, true);
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                stack.push(Point::new(p.x + dx, p.y + dy));
            }
        }
        mask
    }

    /// The selected pixels are the visible pixels of a canvas-sized image.
    pub fn from_image(image: &Image) -> Self {
        Self::from_fn(image.width(), image.height(), |x, y| image.get(x, y).a != 0)
    }

    /// A canvas-sized image that is opaque where the mask is set.
    pub fn to_image(&self) -> Image {
        let mut image = Image::new(self.width, self.height);
        for p in self.pixels() {
            image.set(p.x, p.y, Rgba8::WHITE);
        }
        image
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        let inside = x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height;
        inside.then(|| y as usize * self.width as usize + x as usize)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.index(x, y).is_some_and(|index| self.bits[index])
    }

    fn set(&mut self, x: i32, y: i32, bit: bool) {
        if let Some(index) = self.index(x, y) {
            self.bits[index] = bit;
            if bit {
                self.bounds = self.bounds.union(Rect::new(x, y, 1, 1));
            }
        }
    }

    /// The smallest rectangle containing every selected pixel; empty if there is none.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }

    /// Every selected pixel, row by row.
    pub fn pixels(&self) -> impl Iterator<Item = Point> + '_ {
        let b = self.bounds;
        (b.y..b.bottom())
            .flat_map(move |y| (b.x..b.right()).map(move |x| Point::new(x, y)))
            .filter(|p| self.contains(p.x, p.y))
    }

    /// This selection changed by a new one. Both must be for the same canvas size.
    pub fn combined(&self, new: &Self, how: Combine) -> Self {
        Self::from_fn(self.width, self.height, |x, y| {
            let (old, new) = (self.contains(x, y), new.contains(x, y));
            match how {
                Combine::Replace => new,
                Combine::Add => old || new,
                Combine::Subtract => old && !new,
                Combine::Intersect => old && new,
            }
        })
    }

    pub fn inverted(&self) -> Self {
        Self::from_fn(self.width, self.height, |x, y| !self.contains(x, y))
    }

    /// The same shape moved by `(dx, dy)`; what leaves the canvas is lost.
    pub fn translated(&self, dx: i32, dy: i32) -> Self {
        Self::from_fn(self.width, self.height, |x, y| {
            self.contains(x - dx, y - dy)
        })
    }

    /// The border of the selection as line segments between pixel corners: horizontal ones
    /// as `(x0, y)..(x1, y)`, vertical ones as `(x, y0)..(x, y1)`. Runs are joined.
    pub fn outline(&self) -> Vec<(Point, Point)> {
        let mut segments = Vec::new();
        let b = self.bounds;
        for y in b.y..=b.bottom() {
            let mut run: Option<i32> = None;
            for x in b.x..=b.right() {
                let edge = x < b.right() && self.contains(x, y - 1) != self.contains(x, y);
                match (edge, run) {
                    (true, None) => run = Some(x),
                    (false, Some(start)) => {
                        segments.push((Point::new(start, y), Point::new(x, y)));
                        run = None;
                    }
                    _ => {}
                }
            }
        }
        for x in b.x..=b.right() {
            let mut run: Option<i32> = None;
            for y in b.y..=b.bottom() {
                let edge = y < b.bottom() && self.contains(x - 1, y) != self.contains(x, y);
                match (edge, run) {
                    (true, None) => run = Some(y),
                    (false, Some(start)) => {
                        segments.push((Point::new(x, start), Point::new(x, y)));
                        run = None;
                    }
                    _ => {}
                }
            }
        }
        segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(mask: &Mask) -> Vec<String> {
        (0..mask.height() as i32)
            .map(|y| {
                (0..mask.width() as i32)
                    .map(|x| if mask.contains(x, y) { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn rectangles_are_clipped_to_the_canvas() {
        let mask = Mask::from_rect(4, 3, Rect::new(2, -1, 5, 3));
        assert_eq!(rows(&mask), ["..##", "..##", "...."]);
        assert_eq!(mask.bounds(), Rect::new(2, 0, 2, 2));
        assert!(!mask.contains(9, 9));
        assert!(Mask::from_rect(4, 3, Rect::new(10, 10, 2, 2)).is_empty());
        assert_eq!(Mask::full(2, 2).pixels().count(), 4);
    }

    #[test]
    fn combining() {
        let a = Mask::from_rect(4, 1, Rect::new(0, 0, 3, 1));
        let b = Mask::from_rect(4, 1, Rect::new(2, 0, 2, 1));
        assert_eq!(rows(&a.combined(&b, Combine::Replace)), ["..##"]);
        assert_eq!(rows(&a.combined(&b, Combine::Add)), ["####"]);
        assert_eq!(rows(&a.combined(&b, Combine::Subtract)), ["##.."]);
        assert_eq!(rows(&a.combined(&b, Combine::Intersect)), ["..#."]);
        assert_eq!(rows(&a.inverted()), ["...#"]);
        assert_eq!(a.inverted().inverted(), a);
    }

    #[test]
    fn translating_loses_what_leaves_the_canvas() {
        let mask = Mask::from_rect(4, 2, Rect::new(0, 0, 2, 2));
        assert_eq!(rows(&mask.translated(3, 1)), ["....", "...#"]);
        assert!(mask.translated(9, 0).is_empty());
    }

    #[test]
    fn polygons_fill_their_inside_and_include_their_outline() {
        let triangle = [Point::new(0, 0), Point::new(6, 0), Point::new(0, 6)];
        let mask = Mask::from_polygon(8, 8, &triangle);
        assert!(mask.contains(1, 1) && mask.contains(6, 0) && mask.contains(0, 6));
        assert!(mask.contains(3, 3) && !mask.contains(5, 5) && !mask.contains(7, 0));

        // A lasso that is just a line still selects the pixels it passed over.
        let stroke = [Point::new(1, 1), Point::new(4, 1)];
        assert_eq!(rows(&Mask::from_polygon(6, 3, &stroke))[1], ".####.");
        assert!(Mask::from_polygon(4, 4, &[]).is_empty());
    }

    #[test]
    fn similar_pixels_contiguous_or_everywhere() {
        let red = Rgba8::rgb(200, 0, 0);
        let nearly = Rgba8::rgb(205, 4, 0);
        let mut image = Image::new(5, 1);
        image.set(0, 0, red);
        image.set(1, 0, nearly);
        image.set(4, 0, red);

        let at = Point::new(0, 0);
        assert_eq!(rows(&Mask::similar(&image, at, 0, true)), ["#...."]);
        assert_eq!(rows(&Mask::similar(&image, at, 5, true)), ["##..."]);
        assert_eq!(rows(&Mask::similar(&image, at, 5, false)), ["##..#"]);
        // Starting on an empty pixel selects the empty area.
        assert_eq!(
            rows(&Mask::similar(&image, Point::new(2, 0), 0, true)),
            ["..##."]
        );
        assert!(Mask::similar(&image, Point::new(9, 9), 0, true).is_empty());
    }

    #[test]
    fn outline_joins_runs_into_long_segments() {
        let mask = Mask::from_rect(8, 8, Rect::new(2, 3, 4, 2));
        let mut outline = mask.outline();
        outline.sort_by_key(|(a, b)| (a.x, a.y, b.x, b.y));
        let p = Point::new;
        assert_eq!(
            outline,
            [
                (p(2, 3), p(2, 5)),
                (p(2, 3), p(6, 3)),
                (p(2, 5), p(6, 5)),
                (p(6, 3), p(6, 5)),
            ]
        );
        assert!(Mask::empty(4, 4).outline().is_empty());
        // A selection touching the canvas edge still has a border there.
        assert_eq!(Mask::full(2, 2).outline().len(), 4);
    }

    #[test]
    fn images_and_masks_convert_both_ways() {
        let mask = Mask::from_rect(4, 4, Rect::new(1, 1, 2, 1));
        assert_eq!(Mask::from_image(&mask.to_image()), mask);
    }
}
