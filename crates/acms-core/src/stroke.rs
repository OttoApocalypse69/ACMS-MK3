//! The stroke pipeline (see `PLAN.md`, section 4.4).
//!
//! ```text
//! pointer positions
//!   → interpolate     [`line`]: no gaps between two samples
//!   → brush stamp     [`Brush::footprint`]: the pixels one sample covers
//!   → ink             [`Ink::apply`]: what happens to each covered pixel
//!   → mask            only the canvas, for now
//!   → preview buffer  [`StrokeBuffer`]: shown live, nothing in the sprite changes
//!   → commit          [`Stroke::commit`]: one transaction, one undo step
//! ```

use crate::color::Rgba8;
use crate::document::Document;
use crate::geom::{Point, Rect};
use crate::image::Image;
use crate::sprite::{LayerId, Sprite, SpriteError};

pub const MAX_BRUSH_SIZE: u32 = 64;

/// Every pixel on the straight line from `from` to `to`, both ends included (Bresenham).
/// Consecutive pixels always touch, at least diagonally.
pub fn line(from: Point, to: Point) -> impl Iterator<Item = Point> {
    let dx = (to.x - from.x).abs();
    let dy = -(to.y - from.y).abs();
    let step_x = if from.x < to.x { 1 } else { -1 };
    let step_y = if from.y < to.y { 1 } else { -1 };

    let mut error = dx + dy;
    let mut next = Some(from);
    std::iter::from_fn(move || {
        let current = next?;
        next = (current != to).then(|| {
            let mut p = current;
            let doubled = 2 * error;
            if doubled >= dy {
                error += dy;
                p.x += step_x;
            }
            if doubled <= dx {
                error += dx;
                p.y += step_y;
            }
            p
        });
        Some(current)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushShape {
    Square,
    Round,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Brush {
    pub shape: BrushShape,
    /// Diameter in pixels, `1..=MAX_BRUSH_SIZE`.
    size: u32,
}

impl Default for Brush {
    fn default() -> Self {
        Self::new(BrushShape::Round, 1)
    }
}

impl Brush {
    /// `size` is clamped to `1..=MAX_BRUSH_SIZE`.
    pub fn new(shape: BrushShape, size: u32) -> Self {
        Self {
            shape,
            size: size.clamp(1, MAX_BRUSH_SIZE),
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn set_size(&mut self, size: u32) {
        self.size = size.clamp(1, MAX_BRUSH_SIZE);
    }

    /// The square the brush fits in when stamped at `center`.
    pub fn bounds_at(&self, center: Point) -> Rect {
        let half = (self.size / 2) as i32;
        Rect::new(center.x - half, center.y - half, self.size, self.size)
    }

    /// The pixels the brush covers, as offsets from the pixel it is stamped on.
    pub fn footprint(&self) -> Vec<Point> {
        let size = self.size as i32;
        let half = size / 2;
        let radius = self.size as f32 / 2.0;
        // Shaving half a pixel off the squared radius keeps small circles round: size 3 is a
        // plus, not a block.
        let limit = (radius * radius - 0.5).max(0.0);

        let mut offsets = Vec::with_capacity((size * size) as usize);
        for y in 0..size {
            for x in 0..size {
                let inside = match self.shape {
                    BrushShape::Square => true,
                    BrushShape::Round => {
                        let dx = x as f32 + 0.5 - radius;
                        let dy = y as f32 + 0.5 - radius;
                        dx * dx + dy * dy <= limit
                    }
                };
                if inside {
                    offsets.push(Point::new(x - half, y - half));
                }
            }
        }
        offsets
    }
}

/// How a covered pixel changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    /// Sets the pixel to a colour.
    Simple(Rgba8),
    /// Makes the pixel transparent.
    Erase,
}

impl Ink {
    /// The new value of a pixel, given what the sprite held before the stroke began.
    pub fn apply(self, _original: Rgba8) -> Rgba8 {
        match self {
            Self::Simple(color) => color.normalized(),
            Self::Erase => Rgba8::TRANSPARENT,
        }
    }
}

/// The preview buffer: the pixels a stroke has changed so far, without touching the sprite.
///
/// It covers the whole canvas so that it can be reused from one stroke to the next.
#[derive(Clone, Debug, Default)]
pub struct StrokeBuffer {
    canvas: Rect,
    pixels: Vec<Rgba8>,
    touched: Vec<bool>,
    /// The smallest rectangle containing every touched pixel.
    bounds: Rect,
}

impl StrokeBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Empties the buffer and sizes it for a canvas.
    pub fn reset(&mut self, canvas: Rect) {
        if canvas == self.canvas {
            // Same canvas as last time: wipe only what the last stroke used.
            for y in self.bounds.y..self.bounds.bottom() {
                let row = (y - canvas.y) as usize * canvas.w as usize;
                let from = row + (self.bounds.x - canvas.x) as usize;
                self.touched[from..from + self.bounds.w as usize].fill(false);
            }
        } else {
            self.canvas = canvas;
            self.pixels = vec![Rgba8::TRANSPARENT; canvas.area()];
            self.touched = vec![false; canvas.area()];
        }
        self.bounds = Rect::EMPTY;
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        self.canvas.contains(Point::new(x, y)).then(|| {
            (y - self.canvas.y) as usize * self.canvas.w as usize + (x - self.canvas.x) as usize
        })
    }

    /// The stroke's value for a canvas pixel, or `None` if the stroke has not touched it.
    pub fn get(&self, x: i32, y: i32) -> Option<Rgba8> {
        let index = self.index(x, y)?;
        self.touched[index].then(|| self.pixels[index])
    }

    /// The area the stroke has touched so far.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Whether this pixel can still be written: it is on the canvas and not yet touched.
    fn is_free(&self, x: i32, y: i32) -> bool {
        self.index(x, y).is_some_and(|index| !self.touched[index])
    }

    fn put(&mut self, x: i32, y: i32, color: Rgba8) {
        if let Some(index) = self.index(x, y) {
            self.pixels[index] = color;
            self.touched[index] = true;
            self.bounds = self.bounds.union(Rect::new(x, y, 1, 1));
        }
    }
}

/// A stroke in progress on one cel.
pub struct Stroke {
    layer: LayerId,
    frame: usize,
    footprint: Vec<Point>,
    brush: Brush,
    ink: Ink,
    last: Option<Point>,
    buffer: StrokeBuffer,
}

impl Stroke {
    /// Starts a stroke. `buffer` is an old preview buffer to reuse, or a new one.
    pub fn begin(
        sprite: &Sprite,
        layer: LayerId,
        frame: usize,
        brush: Brush,
        ink: Ink,
        mut buffer: StrokeBuffer,
    ) -> Result<Self, SpriteError> {
        sprite.check_cel(layer, frame)?;
        buffer.reset(sprite.bounds());
        Ok(Self {
            layer,
            frame,
            footprint: brush.footprint(),
            brush,
            ink,
            last: None,
            buffer,
        })
    }

    pub fn layer(&self) -> LayerId {
        self.layer
    }

    pub fn frame(&self) -> usize {
        self.frame
    }

    /// What the stroke has drawn so far.
    pub fn buffer(&self) -> &StrokeBuffer {
        &self.buffer
    }

    /// Continues the stroke to `point`, filling every pixel on the way from the previous
    /// point. Returns the canvas area whose preview changed.
    pub fn extend_to(&mut self, sprite: &Sprite, point: Point) -> Rect {
        let from = self.last.unwrap_or(point);
        self.last = Some(point);

        let cel = sprite.cel(self.layer, self.frame);
        // The first pixel of a continued segment was stamped by the previous call.
        let skip = usize::from(from != point);
        for center in line(from, point).skip(skip) {
            for offset in &self.footprint {
                let (x, y) = (center.x + offset.x, center.y + offset.y);
                // Each pixel is inked once per stroke, from what the sprite held before it.
                if self.buffer.is_free(x, y) {
                    let original = cel.map_or(Rgba8::TRANSPARENT, |cel| cel.pixel(x, y));
                    self.buffer.put(x, y, self.ink.apply(original));
                }
            }
        }

        self.brush
            .bounds_at(from)
            .union(self.brush.bounds_at(point))
            .intersect(sprite.bounds())
    }

    /// Writes the stroke into the document as one undo step called `name`, and hands the
    /// preview buffer back for the next stroke.
    pub fn commit(self, document: &mut Document, name: &str) -> Result<StrokeBuffer, SpriteError> {
        let Self {
            layer,
            frame,
            buffer,
            ..
        } = self;
        let rect = buffer.bounds();
        if !rect.is_empty() {
            let mut pixels = document.sprite().read_region(layer, frame, rect);
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    if let Some(color) = buffer.get(x, y) {
                        pixels.set(x - rect.x, y - rect.y, color);
                    }
                }
            }
            document.transact(name, |tx| tx.set_pixels(layer, frame, rect, &pixels))?;
        }
        Ok(buffer)
    }

    /// Abandons the stroke. Returns the preview buffer and the canvas area it had covered.
    pub fn cancel(self) -> (StrokeBuffer, Rect) {
        let rect = self.buffer.bounds();
        (self.buffer, rect)
    }
}

/// Renders a brush footprint into a small image, for tests and brush previews.
pub fn footprint_image(brush: Brush, color: Rgba8) -> Image {
    let half = (brush.size() / 2) as i32;
    let mut image = Image::new(brush.size(), brush.size());
    for offset in brush.footprint() {
        image.set(offset.x + half, offset.y + half, color);
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const L1: LayerId = LayerId(1);

    fn points(from: (i32, i32), to: (i32, i32)) -> Vec<(i32, i32)> {
        line(Point::new(from.0, from.1), Point::new(to.0, to.1))
            .map(|p| (p.x, p.y))
            .collect()
    }

    #[test]
    fn line_includes_both_ends() {
        assert_eq!(points((3, 3), (3, 3)), [(3, 3)]);
        assert_eq!(points((0, 0), (3, 0)), [(0, 0), (1, 0), (2, 0), (3, 0)]);
        assert_eq!(points((0, 2), (0, 0)), [(0, 2), (0, 1), (0, 0)]);
        assert_eq!(points((0, 0), (2, 2)), [(0, 0), (1, 1), (2, 2)]);
        assert_eq!(
            points((0, 0), (4, 2)),
            [(0, 0), (1, 1), (2, 1), (3, 2), (4, 2)]
        );
    }

    #[test]
    fn line_never_leaves_a_gap_in_any_direction() {
        for target in [
            (40, 7),
            (-40, 7),
            (7, 40),
            (7, -40),
            (-13, -29),
            (25, -25),
            (0, 31),
        ] {
            let pixels = points((0, 0), target);
            assert_eq!(pixels.first(), Some(&(0, 0)));
            assert_eq!(pixels.last(), Some(&target));
            for pair in pixels.windows(2) {
                let (dx, dy) = (pair[1].0 - pair[0].0, pair[1].1 - pair[0].1);
                assert!(dx.abs() <= 1 && dy.abs() <= 1 && (dx, dy) != (0, 0));
            }
            // One pixel per step along the longer axis: no doubled-up pixels either.
            let longest = target.0.abs().max(target.1.abs()) as usize;
            assert_eq!(pixels.len(), longest + 1);
        }
    }

    fn rows(brush: Brush) -> Vec<String> {
        let image = footprint_image(brush, RED);
        (0..image.height() as i32)
            .map(|y| {
                (0..image.width() as i32)
                    .map(|x| if image.get(x, y) == RED { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn round_brush_shapes() {
        assert_eq!(rows(Brush::new(BrushShape::Round, 1)), ["#"]);
        assert_eq!(rows(Brush::new(BrushShape::Round, 2)), ["##", "##"]);
        assert_eq!(
            rows(Brush::new(BrushShape::Round, 3)),
            [".#.", "###", ".#."]
        );
        assert_eq!(
            rows(Brush::new(BrushShape::Round, 4)),
            [".##.", "####", "####", ".##."]
        );
        assert_eq!(
            rows(Brush::new(BrushShape::Round, 5)),
            [".###.", "#####", "#####", "#####", ".###."]
        );
        assert_eq!(
            rows(Brush::new(BrushShape::Round, 7)),
            [
                "..###..", ".#####.", "#######", "#######", "#######", ".#####.", "..###.."
            ]
        );
    }

    #[test]
    fn square_brush_fills_its_bounds_and_sizes_are_clamped() {
        assert_eq!(
            rows(Brush::new(BrushShape::Square, 3)),
            ["###", "###", "###"]
        );
        assert_eq!(Brush::new(BrushShape::Square, 0).size(), 1);
        assert_eq!(Brush::new(BrushShape::Square, 1000).size(), MAX_BRUSH_SIZE);

        let brush = Brush::new(BrushShape::Square, 4);
        let bounds = brush.bounds_at(Point::new(10, 10));
        assert_eq!(bounds, Rect::new(8, 8, 4, 4));
        for offset in brush.footprint() {
            assert!(bounds.contains(Point::new(10 + offset.x, 10 + offset.y)));
        }
    }

    #[test]
    fn inks() {
        let blue = Rgba8::rgb(0, 0, 255);
        assert_eq!(Ink::Simple(RED).apply(blue), RED);
        assert_eq!(Ink::Simple(RED).apply(Rgba8::TRANSPARENT), RED);
        assert_eq!(Ink::Erase.apply(blue), Rgba8::TRANSPARENT);
        // Painting with an invisible colour is the same as erasing.
        assert_eq!(
            Ink::Simple(Rgba8::new(9, 9, 9, 0)).apply(blue),
            Rgba8::TRANSPARENT
        );
    }

    #[test]
    fn buffer_tracks_touched_pixels_and_resets_cleanly() {
        let mut buffer = StrokeBuffer::new();
        buffer.reset(Rect::from_size(8, 8));
        assert_eq!(buffer.get(2, 2), None);
        assert!(buffer.is_free(2, 2));
        assert!(!buffer.is_free(8, 2));

        buffer.put(2, 2, RED);
        buffer.put(5, 3, Rgba8::TRANSPARENT);
        buffer.put(99, 99, RED);
        assert_eq!(buffer.get(2, 2), Some(RED));
        assert_eq!(buffer.get(5, 3), Some(Rgba8::TRANSPARENT));
        assert_eq!(buffer.bounds(), Rect::new(2, 2, 4, 2));
        assert!(!buffer.is_free(2, 2));

        buffer.reset(Rect::from_size(8, 8));
        assert_eq!(buffer.get(2, 2), None);
        assert_eq!(buffer.get(5, 3), None);
        assert!(buffer.bounds().is_empty());

        buffer.put(1, 1, RED);
        buffer.reset(Rect::from_size(4, 4));
        assert_eq!(buffer.get(1, 1), None);
    }

    fn stroke(doc: &Document, brush: Brush, ink: Ink) -> Stroke {
        Stroke::begin(doc.sprite(), L1, 0, brush, ink, StrokeBuffer::new()).unwrap()
    }

    #[test]
    fn a_stroke_previews_without_changing_the_sprite_then_commits_as_one_step() {
        let mut doc = Document::new(Sprite::new(16, 16).unwrap());
        let mut s = stroke(&doc, Brush::default(), Ink::Simple(RED));

        assert_eq!(
            s.extend_to(doc.sprite(), Point::new(2, 2)),
            Rect::new(2, 2, 1, 1)
        );
        let damage = s.extend_to(doc.sprite(), Point::new(9, 5));
        assert_eq!(damage, Rect::new(2, 2, 8, 4));
        s.extend_to(doc.sprite(), Point::new(9, 12));

        assert_eq!(s.buffer().get(9, 8), Some(RED));
        assert_eq!(doc.sprite().cels().count(), 0);
        assert!(!doc.can_undo());

        s.commit(&mut doc, "Pencil").unwrap();
        assert_eq!(doc.history().collect::<Vec<_>>(), ["Pencil"]);
        assert_eq!(doc.sprite().pixel(L1, 0, 2, 2), RED);
        assert_eq!(doc.sprite().pixel(L1, 0, 9, 12), RED);
        assert_eq!(
            doc.sprite().cel(L1, 0).unwrap().rect(),
            Rect::new(2, 2, 8, 11)
        );

        doc.undo();
        assert_eq!(doc.sprite().cels().count(), 0);
    }

    #[test]
    fn a_fast_stroke_has_no_gaps() {
        let mut doc = Document::new(Sprite::new(64, 64).unwrap());
        let mut s = stroke(&doc, Brush::default(), Ink::Simple(RED));
        // Samples far apart, as from a fast mouse.
        let samples = [(1, 1), (40, 9), (60, 60), (3, 50), (30, 30)];
        for (x, y) in samples {
            s.extend_to(doc.sprite(), Point::new(x, y));
        }
        s.commit(&mut doc, "Pencil").unwrap();

        // Walk the same path: every pixel on it must be painted.
        for pair in samples.windows(2) {
            for p in line(
                Point::new(pair[0].0, pair[0].1),
                Point::new(pair[1].0, pair[1].1),
            ) {
                assert_eq!(doc.sprite().pixel(L1, 0, p.x, p.y), RED, "gap at {p:?}");
            }
        }
    }

    #[test]
    fn strokes_are_clipped_to_the_canvas() {
        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        let mut s = stroke(&doc, Brush::new(BrushShape::Square, 4), Ink::Simple(RED));
        s.extend_to(doc.sprite(), Point::new(-10, 4));
        let damage = s.extend_to(doc.sprite(), Point::new(20, 4));
        assert_eq!(damage, Rect::new(0, 2, 8, 4));
        s.commit(&mut doc, "Pencil").unwrap();
        assert_eq!(
            doc.sprite().cel(L1, 0).unwrap().rect(),
            Rect::new(0, 2, 8, 4)
        );
    }

    #[test]
    fn erasing_removes_pixels_and_an_empty_stroke_leaves_no_undo_step() {
        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        let mut s = stroke(&doc, Brush::new(BrushShape::Square, 8), Ink::Simple(RED));
        s.extend_to(doc.sprite(), Point::new(4, 4));
        let buffer = s.commit(&mut doc, "Fill").unwrap();

        let mut s =
            Stroke::begin(doc.sprite(), L1, 0, Brush::default(), Ink::Erase, buffer).unwrap();
        s.extend_to(doc.sprite(), Point::new(0, 0));
        s.extend_to(doc.sprite(), Point::new(7, 0));
        // The preview shows the erased pixels as transparent, and only those.
        assert_eq!(s.buffer().get(3, 0), Some(Rgba8::TRANSPARENT));
        assert_eq!(s.buffer().get(3, 1), None);
        let buffer = s.commit(&mut doc, "Eraser").unwrap();
        assert_eq!(
            doc.sprite().cel(L1, 0).unwrap().rect(),
            Rect::new(0, 1, 8, 7)
        );
        assert_eq!(doc.history().len(), 2);

        // Erasing where there is nothing changes nothing.
        let mut s =
            Stroke::begin(doc.sprite(), L1, 0, Brush::default(), Ink::Erase, buffer).unwrap();
        s.extend_to(doc.sprite(), Point::new(0, 0));
        s.commit(&mut doc, "Eraser").unwrap();
        assert_eq!(doc.history().len(), 2);
    }

    #[test]
    fn cancelling_a_stroke_changes_nothing() {
        let doc = Document::new(Sprite::new(8, 8).unwrap());
        let mut s = stroke(&doc, Brush::default(), Ink::Simple(RED));
        s.extend_to(doc.sprite(), Point::new(1, 1));
        s.extend_to(doc.sprite(), Point::new(3, 1));
        let (_, covered) = s.cancel();
        assert_eq!(covered, Rect::new(1, 1, 3, 1));
        assert_eq!(doc.sprite().cels().count(), 0);
        assert!(!doc.can_undo());
    }

    #[test]
    fn a_stroke_needs_an_existing_cel_position() {
        let doc = Document::new(Sprite::new(8, 8).unwrap());
        let result = Stroke::begin(
            doc.sprite(),
            LayerId(7),
            0,
            Brush::default(),
            Ink::Erase,
            StrokeBuffer::new(),
        );
        assert!(matches!(result, Err(SpriteError::UnknownLayer(7))));
    }
}
