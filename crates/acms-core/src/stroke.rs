//! The stroke pipeline (see `PLAN.md`, section 4.4).
//!
//! ```text
//! pointer positions
//!   → interpolate     [`line`]: no gaps between two samples
//!   → pixel-perfect   [`is_corner`]: takes back the corner pixel of an L-shaped step
//!   → symmetry        [`Symmetry`]: every sample is repeated in its mirror images
//!   → brush stamp     [`Brush::footprint`]: the pixels one sample covers
//!   → ink             [`Ink::apply`]: what happens to each covered pixel
//!   → mask            the selection, [`lock_alpha`], wrap-around and the canvas edge
//!   → preview buffer  [`StrokeBuffer`]: shown live, nothing in the sprite changes
//!   → commit          [`Stroke::commit`]: one transaction, one undo step
//! ```

use crate::color::Rgba8;
use std::sync::Arc;

use crate::document::{Document, Transaction};
use crate::geom::{Point, Rect};
use crate::image::Image;
use crate::pattern::Pattern;
use crate::selection::Mask;
use crate::sprite::{LayerId, Ramp, Sprite, SpriteError, Symmetry};

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

/// The pixel-perfect filter: whether `b` is the corner of an L-shaped step from `a` to `c`.
/// Without it a one-pixel line still connects, diagonally, and looks cleaner.
pub fn is_corner(a: Point, b: Point, c: Point) -> bool {
    let diagonal = (a.x - c.x).abs() == 1 && (a.y - c.y).abs() == 1;
    diagonal && (b.x == a.x || b.y == a.y) && (b.x == c.x || b.y == c.y) && b != a && b != c
}

fn touching(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() <= 1 && (a.y - b.y).abs() <= 1
}

impl Symmetry {
    /// How many times a stroke is drawn, the original included.
    pub fn copies(self) -> usize {
        match self {
            Self::Off => 1,
            Self::Horizontal | Self::Vertical => 2,
            Self::Both => 4,
            Self::Radial(count) => usize::from(count.max(1)),
        }
    }

    /// Whether every copy is an exact image of the original, pixel for pixel. Turning by
    /// anything but a quarter turn has to round, which can leave gaps to fill in.
    pub fn is_exact(self) -> bool {
        !matches!(self, Self::Radial(count) if !matches!(count, 1 | 2 | 4))
    }

    /// Where copy number `copy` (0 is the original) of a pixel lands on a canvas of the given
    /// size. The mirror lines and the centre of rotation are in the middle of the canvas.
    pub fn image(self, copy: usize, p: Point, width: u32, height: u32) -> Point {
        let (w, h) = (width as i32, height as i32);
        let flip_x = Point::new(w - 1 - p.x, p.y);
        let flip_y = Point::new(p.x, h - 1 - p.y);
        match (self, copy) {
            (Self::Off, _) | (_, 0) => p,
            (Self::Horizontal, _) => flip_x,
            (Self::Vertical, _) => flip_y,
            (Self::Both, 1) => flip_x,
            (Self::Both, 2) => flip_y,
            (Self::Both, _) => Point::new(flip_x.x, flip_y.y),
            (Self::Radial(count), copy) => {
                let count = usize::from(count.max(1));
                // Twice the offset of the pixel's centre from the centre of the canvas, which
                // is a whole number whatever the canvas size.
                let (dx, dy) = (2 * p.x + 1 - w, 2 * p.y + 1 - h);
                let (rx, ry) = if 4 * copy % count == 0 {
                    match 4 * copy / count % 4 {
                        1 => (-dy, dx),
                        2 => (-dx, -dy),
                        3 => (dy, -dx),
                        _ => (dx, dy),
                    }
                } else {
                    let angle = std::f64::consts::TAU * copy as f64 / count as f64;
                    let (sin, cos) = angle.sin_cos();
                    let (dx, dy) = (f64::from(dx), f64::from(dy));
                    (
                        (dx * cos - dy * sin).round() as i32,
                        (dx * sin + dy * cos).round() as i32,
                    )
                };
                Point::new((rx + w - 1).div_euclid(2), (ry + h - 1).div_euclid(2))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushShape {
    Square,
    Round,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brush {
    shape: BrushShape,
    /// Diameter in pixels, `1..=MAX_BRUSH_SIZE`.
    size: u32,
    /// A custom brush: these pixels are stamped instead of a square or circle.
    image: Option<Arc<Image>>,
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
            image: None,
        }
    }

    /// A brush that stamps `image`, cropped to what is visible in it. `None` if nothing is,
    /// or if that is larger than `MAX_BRUSH_SIZE` on a side.
    pub fn custom(image: &Image) -> Option<Self> {
        let content = image.content_bounds();
        let fits = 1..=MAX_BRUSH_SIZE;
        if !fits.contains(&content.w) || !fits.contains(&content.h) {
            return None;
        }
        Some(Self {
            shape: BrushShape::Square,
            size: content.w.max(content.h),
            image: Some(Arc::new(image.crop(content))),
        })
    }

    pub fn shape(&self) -> BrushShape {
        self.shape
    }

    /// Makes this a plain brush of the given shape.
    pub fn set_shape(&mut self, shape: BrushShape) {
        self.shape = shape;
        self.image = None;
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// Changes the size of a plain brush; a custom brush keeps the size of its image.
    pub fn set_size(&mut self, size: u32) {
        if self.image.is_none() {
            self.size = size.clamp(1, MAX_BRUSH_SIZE);
        }
    }

    /// The image of a custom brush.
    pub fn image(&self) -> Option<&Image> {
        self.image.as_deref()
    }

    pub fn is_custom(&self) -> bool {
        self.image.is_some()
    }

    fn extent(&self) -> (u32, u32) {
        match &self.image {
            Some(image) => (image.width(), image.height()),
            None => (self.size, self.size),
        }
    }

    /// The rectangle the brush fits in when stamped at `center`.
    pub fn bounds_at(&self, center: Point) -> Rect {
        let (w, h) = self.extent();
        Rect::new(center.x - (w / 2) as i32, center.y - (h / 2) as i32, w, h)
    }

    /// The pixels the brush covers, as offsets from the pixel it is stamped on.
    pub fn footprint(&self) -> Vec<Point> {
        self.stamp().into_iter().map(|(offset, _)| offset).collect()
    }

    /// The footprint, with the colour each pixel of a custom brush carries.
    fn stamp(&self) -> Vec<(Point, Option<Rgba8>)> {
        if let Some(image) = &self.image {
            let (half_w, half_h) = ((image.width() / 2) as i32, (image.height() / 2) as i32);
            let mut pixels = Vec::new();
            for y in 0..image.height() as i32 {
                for x in 0..image.width() as i32 {
                    let color = image.get(x, y);
                    if color.a != 0 {
                        pixels.push((Point::new(x - half_w, y - half_h), Some(color)));
                    }
                }
            }
            return pixels;
        }

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
                    offsets.push((Point::new(x - half, y - half), None));
                }
            }
        }
        offsets
    }
}

/// How a covered pixel changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ink {
    /// Sets the pixel to a colour.
    Simple(Rgba8),
    /// Makes the pixel transparent.
    Erase,
    /// Sets the pixel to one of two colours, chosen by a pattern.
    Dither {
        first: Rgba8,
        second: Rgba8,
        pattern: Pattern,
        /// The share of pixels that get `first`, in percent.
        density: u8,
    },
    /// Moves the pixel's colour `step` places along the first ramp that contains it. Colours
    /// that are in no ramp are left alone, and the ends of a ramp are as far as it goes.
    Shade { ramps: Arc<[Ramp]>, step: i32 },
}

impl Ink {
    /// The new value of the pixel at a canvas position, given what the sprite held there
    /// before the stroke began.
    pub fn apply(&self, original: Rgba8, x: i32, y: i32) -> Rgba8 {
        match self {
            Self::Simple(color) => color.normalized(),
            Self::Erase => Rgba8::TRANSPARENT,
            Self::Dither {
                first,
                second,
                pattern,
                density,
            } => {
                let picked = if pattern.picks_first(x, y, *density) {
                    first
                } else {
                    second
                };
                picked.normalized()
            }
            Self::Shade { ramps, step } => ramps
                .iter()
                .find_map(|ramp| {
                    let at = ramp.iter().position(|color| *color == original)?;
                    let last = ramp.len() as i64 - 1;
                    let to = (at as i64 + i64::from(*step)).clamp(0, last);
                    ramp.get(to as usize).copied()
                })
                .unwrap_or(original),
        }
    }
}

/// The mask for a layer with alpha lock: drawing recolours the pixels that are already there
/// and never changes how transparent anything is.
pub fn lock_alpha(original: Rgba8, inked: Rgba8) -> Rgba8 {
    if original.a == 0 || inked.a == 0 {
        return original;
    }
    Rgba8::new(inked.r, inked.g, inked.b, original.a)
}

/// The preview buffer: the pixels a stroke has changed so far, without touching the sprite.
///
/// It covers the whole canvas so that it can be reused from one stroke to the next.
#[derive(Clone, Debug, Default)]
pub struct StrokeBuffer {
    canvas: Rect,
    pixels: Vec<Rgba8>,
    touched: Vec<bool>,
    /// A rectangle containing every touched pixel. It only ever grows during a stroke.
    bounds: Rect,
    /// The area whose pixels were written or taken back since [`StrokeBuffer::take_dirty`].
    dirty: Rect,
    /// While `Some`, the index of every pixel that gets written is noted here.
    journal: Option<Vec<usize>>,
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
        self.dirty = Rect::EMPTY;
        self.journal = None;
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

    /// An area containing everything the stroke has touched so far.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// The area that changed since the last call.
    fn take_dirty(&mut self) -> Rect {
        std::mem::take(&mut self.dirty)
    }

    /// Whether this pixel can still be written: it is on the canvas and not yet touched.
    fn is_free(&self, x: i32, y: i32) -> bool {
        self.index(x, y).is_some_and(|index| !self.touched[index])
    }

    fn put(&mut self, x: i32, y: i32, color: Rgba8) {
        if let Some(index) = self.index(x, y) {
            self.pixels[index] = color;
            self.touched[index] = true;
            let pixel = Rect::new(x, y, 1, 1);
            self.bounds = self.bounds.union(pixel);
            self.dirty = self.dirty.union(pixel);
            if let Some(journal) = &mut self.journal {
                journal.push(index);
            }
        }
    }

    /// Makes pixels untouched again, as if the stroke had never reached them.
    fn take_back(&mut self, indices: &[usize]) {
        let width = self.canvas.w as usize;
        for &index in indices {
            self.touched[index] = false;
            let (x, y) = ((index % width) as i32, (index / width) as i32);
            let pixel = Rect::new(self.canvas.x + x, self.canvas.y + y, 1, 1);
            self.dirty = self.dirty.union(pixel);
        }
    }
}

/// Everything about a stroke that is decided when it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrokeSettings {
    pub brush: Brush,
    pub ink: Ink,
    /// Freehand lines lose the corner pixel of every L-shaped step.
    pub pixel_perfect: bool,
    /// The stroke is repeated in the mirror images the sprite's symmetry asks for.
    pub symmetric: bool,
    /// What leaves the canvas on one side comes back in on the other.
    pub tiled: bool,
}

impl StrokeSettings {
    /// A plain stroke: no filter, no symmetry, no wrap-around.
    pub fn new(brush: Brush, ink: Ink) -> Self {
        Self {
            brush,
            ink,
            pixel_perfect: false,
            symmetric: false,
            tiled: false,
        }
    }
}

/// Where a stroke's pixels go, and the masks in their way.
struct Target {
    layer: LayerId,
    frame: usize,
    canvas: Rect,
    alpha_lock: bool,
    tiled: bool,
    /// The selection when the stroke began; pixels outside it are left alone.
    selection: Option<Arc<Mask>>,
}

impl Target {
    /// Runs one pixel through ink and masks into the preview. Each pixel is inked once per
    /// stroke, from what the sprite held before it.
    fn plot(&self, buffer: &mut StrokeBuffer, sprite: &Sprite, x: i32, y: i32, ink: &Ink) {
        let (x, y) = if self.tiled && !self.canvas.is_empty() {
            (
                x.rem_euclid(self.canvas.w as i32),
                y.rem_euclid(self.canvas.h as i32),
            )
        } else {
            (x, y)
        };
        if !buffer.is_free(x, y) {
            return;
        }
        if self
            .selection
            .as_ref()
            .is_some_and(|mask| !mask.contains(x, y))
        {
            return;
        }
        let original = sprite.pixel(self.layer, self.frame, x, y);
        let inked = ink.apply(original, x, y);
        let masked = if self.alpha_lock {
            lock_alpha(original, inked)
        } else {
            inked
        };
        buffer.put(x, y, masked);
    }
}

/// The newest point of a pixel-perfect freehand line, which the next point may still turn
/// into a corner that has to go.
struct Tail {
    point: Point,
    /// The buffer pixels only this point wrote.
    wrote: Vec<usize>,
    /// Where each symmetry copy of the line stood before this point.
    copies_before: Vec<Option<Point>>,
}

/// A stroke in progress on one cel: everything a tool draws between pointer down and up.
///
/// Pencil-like tools call [`Stroke::extend_to`]. Tools that redraw their whole shape while the
/// pointer moves call [`Stroke::reset`] and then [`Stroke::stamp`] or [`Stroke::plot`] again.
pub struct Stroke {
    target: Target,
    /// The brush's pixels, with the colour each carries if it is a custom brush.
    footprint: Vec<(Point, Option<Rgba8>)>,
    brush: Brush,
    ink: Ink,
    pixel_perfect: bool,
    symmetry: Symmetry,
    /// The last pointer position of a freehand line.
    last: Option<Point>,
    /// The freehand point before `tail`.
    before_tail: Option<Point>,
    tail: Option<Tail>,
    /// The last pixel stamped for each symmetry copy of the line being drawn.
    copies: Vec<Option<Point>>,
    buffer: StrokeBuffer,
}

impl Stroke {
    /// Starts a plain stroke on an image layer that is neither locked nor hidden. `buffer` is
    /// an old preview buffer to reuse, or a new one.
    pub fn begin(
        sprite: &Sprite,
        layer: LayerId,
        frame: usize,
        brush: Brush,
        ink: Ink,
        buffer: StrokeBuffer,
    ) -> Result<Self, SpriteError> {
        Self::begin_with(
            sprite,
            layer,
            frame,
            StrokeSettings::new(brush, ink),
            buffer,
        )
    }

    /// Like [`Stroke::begin`], with every stage of the pipeline configurable.
    pub fn begin_with(
        sprite: &Sprite,
        layer: LayerId,
        frame: usize,
        settings: StrokeSettings,
        mut buffer: StrokeBuffer,
    ) -> Result<Self, SpriteError> {
        let alpha_lock = sprite.check_drawable(layer, frame)?.alpha_lock;
        buffer.reset(sprite.bounds());
        let symmetry = if settings.symmetric {
            sprite.symmetry()
        } else {
            Symmetry::Off
        };
        Ok(Self {
            target: Target {
                layer,
                frame,
                canvas: sprite.bounds(),
                alpha_lock,
                tiled: settings.tiled,
                selection: sprite.selection().cloned(),
            },
            footprint: settings.brush.stamp(),
            brush: settings.brush,
            ink: settings.ink,
            pixel_perfect: settings.pixel_perfect,
            symmetry,
            last: None,
            before_tail: None,
            tail: None,
            copies: vec![None; symmetry.copies()],
            buffer,
        })
    }

    pub fn layer(&self) -> LayerId {
        self.target.layer
    }

    pub fn frame(&self) -> usize {
        self.target.frame
    }

    pub fn brush(&self) -> &Brush {
        &self.brush
    }

    /// What the stroke has drawn so far.
    pub fn buffer(&self) -> &StrokeBuffer {
        &self.buffer
    }

    /// Wipes what has been drawn so far, to draw the shape again from scratch. Returns the
    /// canvas area that was covered.
    pub fn reset(&mut self) -> Rect {
        let covered = self.buffer.bounds();
        self.buffer.reset(self.target.canvas);
        self.last = None;
        self.before_tail = None;
        self.tail = None;
        self.copies.fill(None);
        covered
    }

    /// Runs one pixel through ink and masks into the preview, without brush or symmetry.
    /// Each pixel is inked once per stroke, from what the sprite held before it.
    pub fn plot(&mut self, sprite: &Sprite, x: i32, y: i32, ink: &Ink) {
        self.target.plot(&mut self.buffer, sprite, x, y, ink);
    }

    /// Sets one pixel regardless of selection and alpha lock, for tools that move pixels
    /// rather than paint them. The first write to a pixel wins.
    pub fn plot_unmasked(&mut self, x: i32, y: i32, color: Rgba8) {
        if self.buffer.is_free(x, y) {
            self.buffer.put(x, y, color.normalized());
        }
    }

    /// Paints one pixel, and its mirror images, with the stroke's ink but without the brush.
    pub fn fill(&mut self, sprite: &Sprite, x: i32, y: i32) {
        let canvas = self.target.canvas;
        for copy in 0..self.symmetry.copies() {
            let p = self
                .symmetry
                .image(copy, Point::new(x, y), canvas.w, canvas.h);
            self.target
                .plot(&mut self.buffer, sprite, p.x, p.y, &self.ink);
        }
    }

    /// Stamps the brush at one place, with the stroke's ink. A custom brush carries its own
    /// colours, which a plain colour ink lets through.
    fn stamp_at(&mut self, sprite: &Sprite, center: Point) {
        let own_colors = matches!(self.ink, Ink::Simple(_));
        for &(offset, color) in &self.footprint {
            let (x, y) = (center.x + offset.x, center.y + offset.y);
            match color.filter(|_| own_colors) {
                Some(color) => {
                    let ink = Ink::Simple(color);
                    self.target.plot(&mut self.buffer, sprite, x, y, &ink);
                }
                None => self.target.plot(&mut self.buffer, sprite, x, y, &self.ink),
            }
        }
    }

    /// Stamps the brush at `center` and at its mirror images. With `connected`, a copy that
    /// had to be rounded into place is joined up with where that copy was stamped last.
    fn stamp_copies(&mut self, sprite: &Sprite, center: Point, connected: bool) {
        let canvas = self.target.canvas;
        for copy in 0..self.symmetry.copies() {
            let at = self.symmetry.image(copy, center, canvas.w, canvas.h);
            let previous = self.copies[copy].filter(|_| connected && !self.symmetry.is_exact());
            match previous.filter(|previous| !touching(*previous, at)) {
                Some(previous) => {
                    for between in line(previous, at).skip(1) {
                        self.stamp_at(sprite, between);
                    }
                }
                None => self.stamp_at(sprite, at),
            }
            self.copies[copy] = Some(at);
        }
    }

    /// Stamps the brush at `center` with the stroke's ink, and at its mirror images.
    pub fn stamp(&mut self, sprite: &Sprite, center: Point) {
        self.stamp_copies(sprite, center, false);
    }

    /// Stamps the brush on every pixel of the straight line between two points.
    pub fn stamp_line(&mut self, sprite: &Sprite, from: Point, to: Point) {
        self.copies.fill(None);
        for center in line(from, to) {
            self.stamp_copies(sprite, center, true);
        }
    }

    /// Adds one pixel to a freehand line, taking the previous one back if that turned out
    /// to be the corner of an L.
    fn freehand_point(&mut self, sprite: &Sprite, point: Point) {
        if !self.pixel_perfect {
            self.stamp_copies(sprite, point, true);
            return;
        }
        match self.tail.take() {
            Some(tail)
                if self
                    .before_tail
                    .is_some_and(|a| is_corner(a, tail.point, point)) =>
            {
                self.buffer.take_back(&tail.wrote);
                self.copies = tail.copies_before;
            }
            Some(tail) => self.before_tail = Some(tail.point),
            None => {}
        }
        let copies_before = self.copies.clone();
        self.buffer.journal = Some(Vec::new());
        self.stamp_copies(sprite, point, true);
        self.tail = Some(Tail {
            point,
            wrote: self.buffer.journal.take().unwrap_or_default(),
            copies_before,
        });
    }

    /// Continues the stroke to `point`, filling every pixel on the way from the previous
    /// point. Returns the canvas area whose preview changed.
    pub fn extend_to(&mut self, sprite: &Sprite, point: Point) -> Rect {
        let from = self.last.replace(point);
        // The line from the last point starts on a pixel that is already drawn.
        let skip = usize::from(from.is_some());
        let from = from.unwrap_or(point);
        for center in line(from, point).skip(skip) {
            self.freehand_point(sprite, center);
        }

        self.brush
            .bounds_at(from)
            .union(self.brush.bounds_at(point))
            .union(self.buffer.take_dirty())
            .intersect(sprite.bounds())
    }

    /// Writes the stroke into a transaction that is already open, and hands the preview
    /// buffer back for the next stroke.
    pub fn apply(self, tx: &mut Transaction<'_>) -> Result<StrokeBuffer, SpriteError> {
        let (layer, frame, buffer) = (self.target.layer, self.target.frame, self.buffer);
        let rect = buffer.bounds();
        if !rect.is_empty() {
            let mut pixels = tx.sprite().read_region(layer, frame, rect);
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    if let Some(color) = buffer.get(x, y) {
                        pixels.set(x - rect.x, y - rect.y, color);
                    }
                }
            }
            tx.set_pixels(layer, frame, rect, &pixels)?;
        }
        Ok(buffer)
    }

    /// Writes the stroke into the document as one undo step called `name`, and hands the
    /// preview buffer back for the next stroke.
    pub fn commit(self, document: &mut Document, name: &str) -> Result<StrokeBuffer, SpriteError> {
        document.transact(name, |tx| self.apply(tx))
    }

    /// Abandons the stroke. Returns the preview buffer and the canvas area it had covered.
    pub fn cancel(self) -> (StrokeBuffer, Rect) {
        let rect = self.buffer.bounds();
        (self.buffer, rect)
    }
}

/// Renders a brush footprint into a small image, for tests and brush previews.
pub fn footprint_image(brush: &Brush, color: Rgba8) -> Image {
    let bounds = brush.bounds_at(Point::new(0, 0));
    let mut image = Image::new(bounds.w, bounds.h);
    for offset in brush.footprint() {
        image.set(offset.x - bounds.x, offset.y - bounds.y, color);
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
        let image = footprint_image(&brush, RED);
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
        assert_eq!(Ink::Simple(RED).apply(blue, 0, 0), RED);
        assert_eq!(Ink::Simple(RED).apply(Rgba8::TRANSPARENT, 0, 0), RED);
        assert_eq!(Ink::Erase.apply(blue, 0, 0), Rgba8::TRANSPARENT);
        // Painting with an invisible colour is the same as erasing.
        assert_eq!(
            Ink::Simple(Rgba8::new(9, 9, 9, 0)).apply(blue, 0, 0),
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

    #[test]
    fn alpha_lock_mask() {
        let blue = Rgba8::rgb(0, 0, 255);
        let faint_blue = Rgba8::new(0, 0, 255, 40);
        // Existing pixels take the new colour and keep their alpha.
        assert_eq!(lock_alpha(blue, RED), RED);
        assert_eq!(lock_alpha(faint_blue, RED), Rgba8::new(255, 0, 0, 40));
        assert_eq!(lock_alpha(blue, Rgba8::new(255, 0, 0, 10)), RED);
        // Nothing appears where there was nothing, and nothing can be erased.
        assert_eq!(lock_alpha(Rgba8::TRANSPARENT, RED), Rgba8::TRANSPARENT);
        assert_eq!(lock_alpha(faint_blue, Rgba8::TRANSPARENT), faint_blue);
    }

    /// A 8×8 document whose only layer has `edit` applied to its properties.
    fn document_with(edit: impl FnOnce(&mut crate::sprite::LayerProps)) -> Document {
        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        doc.transact("Props", |tx| tx.update_layer(L1, edit))
            .unwrap();
        doc
    }

    #[test]
    fn with_alpha_lock_a_stroke_only_recolours_what_is_there() {
        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        let mut s = stroke(
            &doc,
            Brush::default(),
            Ink::Simple(Rgba8::new(0, 0, 255, 100)),
        );
        s.extend_to(doc.sprite(), Point::new(2, 3));
        s.extend_to(doc.sprite(), Point::new(4, 3));
        s.commit(&mut doc, "Pencil").unwrap();
        doc.transact("Lock alpha", |tx| {
            tx.update_layer(L1, |props| {
                if let Some(image) = &mut props.image {
                    image.alpha_lock = true;
                }
            })
        })
        .unwrap();

        // A red line straight through the three blue pixels and well beyond them.
        let mut s = stroke(&doc, Brush::default(), Ink::Simple(RED));
        s.extend_to(doc.sprite(), Point::new(0, 3));
        s.extend_to(doc.sprite(), Point::new(7, 3));
        s.commit(&mut doc, "Pencil").unwrap();
        assert_eq!(doc.sprite().pixel(L1, 0, 3, 3), Rgba8::new(255, 0, 0, 100));
        assert_eq!(doc.sprite().pixel(L1, 0, 1, 3), Rgba8::TRANSPARENT);
        assert_eq!(
            doc.sprite().cel(L1, 0).unwrap().rect(),
            Rect::new(2, 3, 3, 1)
        );

        // The eraser does nothing at all, and leaves no undo step.
        let steps = doc.history().len();
        let mut s = stroke(&doc, Brush::new(BrushShape::Square, 8), Ink::Erase);
        s.extend_to(doc.sprite(), Point::new(4, 4));
        s.commit(&mut doc, "Eraser").unwrap();
        assert_eq!(doc.sprite().pixel(L1, 0, 3, 3), Rgba8::new(255, 0, 0, 100));
        assert_eq!(doc.history().len(), steps);
    }

    #[test]
    fn a_stroke_cannot_start_on_a_locked_hidden_or_group_layer() {
        let begin = |doc: &Document, layer| {
            Stroke::begin(
                doc.sprite(),
                layer,
                0,
                Brush::default(),
                Ink::Simple(RED),
                StrokeBuffer::new(),
            )
            .err()
        };

        let locked = document_with(|props| {
            if let Some(image) = &mut props.image {
                image.locked = true;
            }
        });
        assert_eq!(begin(&locked, L1), Some(SpriteError::LayerLocked(1)));

        let hidden = document_with(|props| props.visible = false);
        assert_eq!(begin(&hidden, L1), Some(SpriteError::LayerHidden(1)));

        let mut grouped = Document::new(Sprite::new(8, 8).unwrap());
        let group = grouped
            .transact("Add", |tx| {
                tx.add_layer(crate::sprite::Slot::new(None, 1), "Group", true)
            })
            .unwrap();
        assert_eq!(
            begin(&grouped, group),
            Some(SpriteError::NotAnImageLayer(group.0))
        );
        assert_eq!(begin(&grouped, L1), None);
    }
    const BLUE: Rgba8 = Rgba8::rgb(0, 0, 255);

    fn settings(edit: impl FnOnce(&mut StrokeSettings)) -> StrokeSettings {
        let mut settings = StrokeSettings::new(Brush::default(), Ink::Simple(RED));
        edit(&mut settings);
        settings
    }

    fn stroke_with(doc: &Document, settings: StrokeSettings) -> Stroke {
        Stroke::begin_with(doc.sprite(), L1, 0, settings, StrokeBuffer::new()).unwrap()
    }

    /// Draws a freehand line through `path` and commits it.
    fn draw(doc: &mut Document, settings: StrokeSettings, path: &[(i32, i32)]) {
        let mut s = stroke_with(doc, settings);
        for &(x, y) in path {
            s.extend_to(doc.sprite(), Point::new(x, y));
        }
        s.commit(doc, "Draw").unwrap();
    }

    /// The painted pixels of the layer, sorted.
    fn painted(doc: &Document) -> Vec<(i32, i32)> {
        let bounds = doc.sprite().bounds();
        let mut pixels = Vec::new();
        for y in 0..bounds.h as i32 {
            for x in 0..bounds.w as i32 {
                if doc.sprite().pixel(L1, 0, x, y).a != 0 {
                    pixels.push((x, y));
                }
            }
        }
        pixels
    }

    fn sorted(mut pixels: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
        pixels.sort_unstable_by_key(|&(x, y)| (y, x));
        pixels
    }

    fn document_with_symmetry(size: u32, symmetry: Symmetry) -> Document {
        let mut doc = Document::new(Sprite::new(size, size).unwrap());
        doc.set_symmetry(symmetry);
        doc
    }

    #[test]
    fn corners_of_an_l_are_recognised() {
        let p = Point::new;
        assert!(is_corner(p(0, 0), p(1, 0), p(1, 1)));
        assert!(is_corner(p(0, 0), p(0, 1), p(1, 1)));
        assert!(is_corner(p(5, 5), p(5, 4), p(4, 4)));
        // Straight on, a diagonal, and a step back are not.
        assert!(!is_corner(p(0, 0), p(1, 0), p(2, 0)));
        assert!(!is_corner(p(0, 0), p(1, 1), p(2, 2)));
        assert!(!is_corner(p(0, 0), p(1, 0), p(0, 0)));
        assert!(!is_corner(p(0, 0), p(1, 1), p(2, 1)));
    }

    #[test]
    fn pixel_perfect_turns_a_staircase_into_a_diagonal() {
        let stairs = [(1, 1), (2, 1), (2, 2), (3, 2), (3, 3), (4, 3), (4, 4)];

        let mut plain = Document::new(Sprite::new(8, 8).unwrap());
        draw(&mut plain, settings(|_| {}), &stairs);
        assert_eq!(painted(&plain), sorted(stairs.to_vec()));

        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        draw(&mut doc, settings(|s| s.pixel_perfect = true), &stairs);
        assert_eq!(painted(&doc), [(1, 1), (2, 2), (3, 3), (4, 4)]);
        assert_eq!(doc.history().len(), 1);
        doc.undo();
        assert!(painted(&doc).is_empty());
    }

    #[test]
    fn pixel_perfect_shows_the_newest_pixel_until_it_turns_out_to_be_a_corner() {
        let doc = Document::new(Sprite::new(8, 8).unwrap());
        let mut s = stroke_with(&doc, settings(|s| s.pixel_perfect = true));
        s.extend_to(doc.sprite(), Point::new(1, 1));
        s.extend_to(doc.sprite(), Point::new(2, 1));
        // Nothing says yet that (2, 1) is a corner: it is under the pointer and shown.
        assert_eq!(s.buffer().get(2, 1), Some(RED));

        let damage = s.extend_to(doc.sprite(), Point::new(2, 2));
        assert_eq!(s.buffer().get(2, 1), None);
        assert_eq!(s.buffer().get(2, 2), Some(RED));
        // The pixel that went away is part of what has to be redrawn.
        assert!(damage.contains(Point::new(2, 1)));

        // Straight lines and long strokes are left as they are.
        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        draw(
            &mut doc,
            settings(|s| s.pixel_perfect = true),
            &[(0, 6), (7, 6)],
        );
        assert_eq!(painted(&doc).len(), 8);
    }

    #[test]
    fn pixel_perfect_cuts_the_corner_of_a_turn() {
        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        draw(
            &mut doc,
            settings(|s| s.pixel_perfect = true),
            &[(1, 1), (5, 1), (5, 5)],
        );
        let pixels = painted(&doc);
        assert!(!pixels.contains(&(5, 1)));
        assert!(pixels.contains(&(4, 1)) && pixels.contains(&(5, 2)));
        assert_eq!(pixels.len(), 8);
    }

    #[test]
    fn mirror_symmetries_repeat_every_pixel() {
        let path = [(1, 2), (3, 2)];
        let cases: [(Symmetry, &[(i32, i32)]); 3] = [
            (Symmetry::Horizontal, &[(4, 2), (5, 2), (6, 2)]),
            (Symmetry::Vertical, &[(1, 5), (2, 5), (3, 5)]),
            (
                Symmetry::Both,
                &[
                    (4, 2),
                    (5, 2),
                    (6, 2),
                    (1, 5),
                    (2, 5),
                    (3, 5),
                    (4, 5),
                    (5, 5),
                    (6, 5),
                ],
            ),
        ];
        for (symmetry, mirrored) in cases {
            let mut doc = document_with_symmetry(8, symmetry);
            draw(&mut doc, settings(|s| s.symmetric = true), &path);
            let mut expected = vec![(1, 2), (2, 2), (3, 2)];
            expected.extend_from_slice(mirrored);
            assert_eq!(painted(&doc), sorted(expected), "{symmetry:?}");
            assert_eq!(doc.history().len(), 1);
        }

        // A stroke that does not ask for symmetry ignores the sprite's setting.
        let mut doc = document_with_symmetry(8, Symmetry::Both);
        draw(&mut doc, settings(|_| {}), &path);
        assert_eq!(painted(&doc).len(), 3);
    }

    #[test]
    fn mirrors_work_on_canvases_of_odd_size() {
        let mut doc = document_with_symmetry(7, Symmetry::Both);
        draw(
            &mut doc,
            settings(|s| s.symmetric = true),
            &[(0, 3), (3, 3)],
        );
        // The middle row is its own mirror image; the middle pixel too.
        assert_eq!(painted(&doc), (0..7).map(|x| (x, 3)).collect::<Vec<_>>());
    }

    #[test]
    fn radial_symmetry_of_four_is_exact_quarter_turns() {
        let mut doc = document_with_symmetry(8, Symmetry::Radial(4));
        draw(&mut doc, settings(|s| s.symmetric = true), &[(1, 2)]);
        assert_eq!(painted(&doc), sorted(vec![(1, 2), (5, 1), (6, 5), (2, 6)]));

        let mut doc = document_with_symmetry(8, Symmetry::Radial(2));
        draw(&mut doc, settings(|s| s.symmetric = true), &[(1, 2)]);
        assert_eq!(painted(&doc), sorted(vec![(1, 2), (6, 5)]));
    }

    /// How many groups of touching pixels there are.
    fn components(pixels: &[(i32, i32)]) -> usize {
        let mut left: Vec<(i32, i32)> = pixels.to_vec();
        let mut count = 0;
        while let Some(start) = left.pop() {
            count += 1;
            let mut stack = vec![start];
            while let Some((x, y)) = stack.pop() {
                let (near, far): (Vec<_>, Vec<_>) = left
                    .iter()
                    .partition(|&&(ox, oy)| (ox - x).abs() <= 1 && (oy - y).abs() <= 1);
                left = far;
                stack.extend(near);
            }
        }
        count
    }

    #[test]
    fn radial_symmetry_at_other_angles_leaves_no_gaps() {
        for count in [3, 5, 6, 7] {
            let mut doc = document_with_symmetry(64, Symmetry::Radial(count));
            // Well away from the centre, so the copies do not run into each other.
            let path = [(40, 8), (47, 9), (52, 14), (50, 20)];
            draw(&mut doc, settings(|s| s.symmetric = true), &path);
            let pixels = painted(&doc);
            assert_eq!(components(&pixels), usize::from(count), "{count}");
            // Each copy has about as many pixels as the original line.
            let per_copy = pixels.len() / usize::from(count);
            assert!((16..=26).contains(&per_copy), "{count}: {per_copy}");
        }
    }

    #[test]
    fn symmetry_images_stay_in_step_with_the_copy_count() {
        let p = Point::new(3, 9);
        for symmetry in [
            Symmetry::Off,
            Symmetry::Horizontal,
            Symmetry::Vertical,
            Symmetry::Both,
            Symmetry::Radial(4),
            Symmetry::Radial(7),
        ] {
            assert_eq!(symmetry.image(0, p, 20, 20), p);
            let mut images: Vec<(i32, i32)> = (0..symmetry.copies())
                .map(|copy| symmetry.image(copy, p, 20, 20))
                .map(|p| (p.x, p.y))
                .collect();
            images.sort_unstable();
            images.dedup();
            assert_eq!(images.len(), symmetry.copies(), "{symmetry:?}");
        }
        assert!(Symmetry::Both.is_exact() && Symmetry::Radial(4).is_exact());
        assert!(!Symmetry::Radial(3).is_exact());
    }

    #[test]
    fn tiled_strokes_wrap_around_the_canvas() {
        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        draw(&mut doc, settings(|s| s.tiled = true), &[(6, 3), (10, 3)]);
        assert_eq!(painted(&doc), [(0, 3), (1, 3), (2, 3), (6, 3), (7, 3)]);

        // A brush stamped on the corner shows up in all four corners.
        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        let brush = Brush::new(BrushShape::Square, 2);
        draw(
            &mut doc,
            settings(|s| {
                s.tiled = true;
                s.brush = brush;
            }),
            &[(0, 0)],
        );
        assert_eq!(painted(&doc), [(0, 0), (7, 0), (0, 7), (7, 7)]);

        // Without it, the same strokes are cut off at the edge.
        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        draw(&mut doc, settings(|_| {}), &[(6, 3), (10, 3)]);
        assert_eq!(painted(&doc), [(6, 3), (7, 3)]);
    }

    fn two_pixel_brush() -> Brush {
        let mut image = Image::new(4, 4);
        image.set(1, 1, RED);
        image.set(2, 2, BLUE);
        Brush::custom(&image).unwrap()
    }

    #[test]
    fn a_custom_brush_is_cropped_and_centred() {
        let brush = two_pixel_brush();
        assert!(brush.is_custom());
        assert_eq!(brush.size(), 2);
        assert_eq!(brush.bounds_at(Point::new(4, 4)), Rect::new(3, 3, 2, 2));
        assert_eq!(brush.footprint(), [Point::new(-1, -1), Point::new(0, 0)]);

        assert_eq!(Brush::custom(&Image::new(4, 4)), None);
        let too_large = Image::from_pixels(65, 1, vec![RED; 65]).unwrap();
        assert_eq!(Brush::custom(&too_large), None);

        // Changing the size does nothing to a custom brush; picking a shape makes it plain.
        let mut brush = brush;
        brush.set_size(9);
        assert_eq!(brush.size(), 2);
        brush.set_shape(BrushShape::Round);
        assert!(!brush.is_custom());
        brush.set_size(9);
        assert_eq!(brush.size(), 9);
    }

    #[test]
    fn a_custom_brush_stamps_its_own_colours_with_a_plain_ink() {
        let green = Rgba8::rgb(0, 255, 0);
        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        draw(
            &mut doc,
            settings(|s| {
                s.brush = two_pixel_brush();
                s.ink = Ink::Simple(green);
            }),
            &[(4, 4)],
        );
        assert_eq!(doc.sprite().pixel(L1, 0, 3, 3), RED);
        assert_eq!(doc.sprite().pixel(L1, 0, 4, 4), BLUE);
        assert_eq!(painted(&doc).len(), 2);

        // Other inks only use its shape.
        draw(
            &mut doc,
            settings(|s| {
                s.brush = two_pixel_brush();
                s.ink = Ink::Dither {
                    first: green,
                    second: Rgba8::WHITE,
                    pattern: Pattern::Checker,
                    density: 50,
                };
            }),
            &[(4, 4)],
        );
        assert_eq!(doc.sprite().pixel(L1, 0, 3, 3), green);
        assert_eq!(doc.sprite().pixel(L1, 0, 4, 4), green);
        draw(
            &mut doc,
            settings(|s| {
                s.brush = two_pixel_brush();
                s.ink = Ink::Erase;
            }),
            &[(4, 4)],
        );
        assert!(painted(&doc).is_empty());
    }

    #[test]
    fn dither_ink_lays_two_colours_in_a_pattern() {
        let ink = |pattern, density| Ink::Dither {
            first: RED,
            second: BLUE,
            pattern,
            density,
        };
        assert_eq!(ink(Pattern::Checker, 0).apply(Rgba8::WHITE, 0, 0), RED);
        assert_eq!(ink(Pattern::Checker, 0).apply(Rgba8::WHITE, 1, 0), BLUE);
        assert_eq!(ink(Pattern::Bayer4, 0).apply(Rgba8::WHITE, 0, 0), BLUE);
        assert_eq!(ink(Pattern::Bayer4, 100).apply(Rgba8::WHITE, 3, 1), RED);

        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        draw(
            &mut doc,
            settings(|s| {
                s.brush = Brush::new(BrushShape::Square, 4);
                s.ink = ink(Pattern::Bayer2, 50);
            }),
            &[(2, 2), (5, 2)],
        );
        for y in 0..4 {
            for x in 0..7 {
                let expected = if (x + y) % 2 == 0 { RED } else { BLUE };
                assert_eq!(doc.sprite().pixel(L1, 0, x, y), expected, "({x}, {y})");
            }
        }

        // A transparent second colour leaves holes rather than painting nothing.
        let holes = Ink::Dither {
            first: RED,
            second: Rgba8::new(1, 2, 3, 0),
            pattern: Pattern::Checker,
            density: 50,
        };
        assert_eq!(holes.apply(BLUE, 1, 0), Rgba8::TRANSPARENT);
    }

    #[test]
    fn shade_ink_steps_along_a_ramp_once_per_stroke() {
        let (dark, mid, light) = (
            Rgba8::rgb(60, 0, 0),
            Rgba8::rgb(150, 0, 0),
            Rgba8::rgb(250, 0, 0),
        );
        let ramps: Arc<[Ramp]> = vec![vec![BLUE, Rgba8::WHITE], vec![dark, mid, light]].into();
        let shade = |step| Ink::Shade {
            ramps: ramps.clone(),
            step,
        };
        assert_eq!(shade(1).apply(dark, 0, 0), mid);
        assert_eq!(shade(1).apply(light, 0, 0), light);
        assert_eq!(shade(-1).apply(mid, 0, 0), dark);
        assert_eq!(shade(-1).apply(dark, 0, 0), dark);
        assert_eq!(shade(2).apply(dark, 0, 0), light);
        assert_eq!(shade(1).apply(BLUE, 0, 0), Rgba8::WHITE);
        // Colours in no ramp, and empty pixels, are left alone.
        assert_eq!(shade(1).apply(RED, 0, 0), RED);
        assert_eq!(shade(1).apply(Rgba8::TRANSPARENT, 0, 0), Rgba8::TRANSPARENT);

        let mut doc = Document::new(Sprite::new(8, 1).unwrap());
        let row = Image::from_pixels(4, 1, vec![dark, mid, light, RED]).unwrap();
        doc.transact("Paint", |tx| {
            tx.set_pixels(L1, 0, Rect::new(0, 0, 4, 1), &row)
        })
        .unwrap();
        // Back and forth over the same pixels: each still moves one step only.
        draw(
            &mut doc,
            settings(|s| s.ink = shade(1)),
            &[(0, 0), (7, 0), (0, 0), (7, 0)],
        );
        let after: Vec<Rgba8> = (0..5).map(|x| doc.sprite().pixel(L1, 0, x, 0)).collect();
        assert_eq!(after, [mid, light, light, RED, Rgba8::TRANSPARENT]);
        assert_eq!(doc.history().len(), 2);
    }

    #[test]
    fn shapes_are_mirrored_too() {
        let mut doc = document_with_symmetry(8, Symmetry::Horizontal);
        let mut s = stroke_with(&doc, settings(|s| s.symmetric = true));
        s.stamp_line(doc.sprite(), Point::new(0, 0), Point::new(2, 2));
        s.fill(doc.sprite(), 1, 6);
        s.commit(&mut doc, "Shape").unwrap();
        assert_eq!(
            painted(&doc),
            [
                (0, 0),
                (7, 0),
                (1, 1),
                (6, 1),
                (2, 2),
                (5, 2),
                (1, 6),
                (6, 6)
            ]
        );
    }
}
