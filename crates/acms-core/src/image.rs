//! A plain RGBA8 pixel buffer.

use crate::color::Rgba8;
use crate::geom::{Point, Rect};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Image {
    width: u32,
    height: u32,
    /// Row by row, top to bottom.
    pixels: Vec<Rgba8>,
}

impl Image {
    /// A fully transparent image.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![Rgba8::TRANSPARENT; width as usize * height as usize],
        }
    }

    /// `None` if `pixels` does not hold exactly `width * height` pixels.
    pub fn from_pixels(width: u32, height: u32, pixels: Vec<Rgba8>) -> Option<Self> {
        (pixels.len() == width as usize * height as usize).then_some(Self {
            width,
            height,
            pixels,
        })
    }

    /// Builds an image from `R, G, B, A` bytes. `None` if the length does not match.
    pub fn from_rgba_bytes(width: u32, height: u32, bytes: &[u8]) -> Option<Self> {
        if bytes.len() != width as usize * height as usize * 4 {
            return None;
        }
        let pixels = bytes
            .chunks_exact(4)
            .map(|px| Rgba8::new(px[0], px[1], px[2], px[3]))
            .collect();
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn to_rgba_bytes(&self) -> Vec<u8> {
        self.pixels.iter().flat_map(|px| px.to_array()).collect()
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The image's own rectangle, at the origin.
    pub fn bounds(&self) -> Rect {
        Rect::from_size(self.width, self.height)
    }

    pub fn is_empty(&self) -> bool {
        self.pixels.is_empty()
    }

    pub fn pixels(&self) -> &[Rgba8] {
        &self.pixels
    }

    /// Bytes of pixel data held, for memory accounting.
    pub fn byte_size(&self) -> usize {
        self.pixels.len() * size_of::<Rgba8>()
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        let inside = x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height;
        inside.then(|| y as usize * self.width as usize + x as usize)
    }

    /// The pixel at `(x, y)`; transparent outside the image.
    pub fn get(&self, x: i32, y: i32) -> Rgba8 {
        self.index(x, y)
            .map_or(Rgba8::TRANSPARENT, |index| self.pixels[index])
    }

    /// Sets the pixel at `(x, y)`; does nothing outside the image.
    pub fn set(&mut self, x: i32, y: i32, color: Rgba8) {
        if let Some(index) = self.index(x, y) {
            self.pixels[index] = color;
        }
    }

    /// A copy of `rect`. Parts of `rect` outside the image come out transparent.
    pub fn crop(&self, rect: Rect) -> Self {
        let mut out = Self::new(rect.w, rect.h);
        let shared = rect.intersect(self.bounds());
        for y in shared.y..shared.bottom() {
            let src = y as usize * self.width as usize;
            let dst = (y - rect.y) as usize * rect.w as usize + (shared.x - rect.x) as usize;
            let (from, to) = (src + shared.x as usize, src + shared.right() as usize);
            out.pixels[dst..dst + shared.w as usize].copy_from_slice(&self.pixels[from..to]);
        }
        out
    }

    /// Copies `src` into this image with its top-left corner at `at`, replacing what was there.
    /// Parts that fall outside are dropped.
    pub fn blit(&mut self, src: &Self, at: Point) {
        let shared = src.bounds().translated(at.x, at.y).intersect(self.bounds());
        for y in shared.y..shared.bottom() {
            let dst = y as usize * self.width as usize + shared.x as usize;
            let from = (y - at.y) as usize * src.width as usize + (shared.x - at.x) as usize;
            self.pixels[dst..dst + shared.w as usize]
                .copy_from_slice(&src.pixels[from..from + shared.w as usize]);
        }
    }

    /// The smallest rectangle containing every pixel that is not fully transparent.
    pub fn content_bounds(&self) -> Rect {
        let width = self.width as usize;
        let mut bounds = Rect::EMPTY;
        for (y, row) in self.pixels.chunks_exact(width.max(1)).enumerate() {
            let Some(first) = row.iter().position(|px| px.a != 0) else {
                continue;
            };
            // `first` exists, so a last one does too.
            let last = row.iter().rposition(|px| px.a != 0).unwrap_or(first);
            let row_rect = Rect::new(first as i32, y as i32, (last - first + 1) as u32, 1);
            bounds = bounds.union(row_rect);
        }
        bounds
    }

    /// A new image of the given size whose pixel `(x, y)` is this image's pixel `source(x, y)`.
    fn remapped(&self, width: u32, height: u32, source: impl Fn(i32, i32) -> (i32, i32)) -> Self {
        let mut out = Self::new(width, height);
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                let (sx, sy) = source(x, y);
                out.set(x, y, self.get(sx, sy));
            }
        }
        out
    }

    /// Mirrored left to right.
    pub fn flipped_horizontal(&self) -> Self {
        let last = self.width as i32 - 1;
        self.remapped(self.width, self.height, |x, y| (last - x, y))
    }

    /// Mirrored top to bottom.
    pub fn flipped_vertical(&self) -> Self {
        let last = self.height as i32 - 1;
        self.remapped(self.width, self.height, |x, y| (x, last - y))
    }

    /// Turned a quarter clockwise; width and height swap.
    pub fn rotated_clockwise(&self) -> Self {
        let last = self.height as i32 - 1;
        self.remapped(self.height, self.width, |x, y| (y, last - x))
    }

    /// Turned a quarter anticlockwise; width and height swap.
    pub fn rotated_anticlockwise(&self) -> Self {
        let last = self.width as i32 - 1;
        self.remapped(self.height, self.width, |x, y| (last - y, x))
    }

    /// Resized to any size with nearest-neighbour sampling, which keeps pixels hard-edged.
    pub fn scaled_to(&self, width: u32, height: u32) -> Self {
        let (from_w, from_h) = (u64::from(self.width), u64::from(self.height));
        self.remapped(width, height, |x, y| {
            let sx = (x as u64 * from_w / u64::from(width.max(1))) as i32;
            let sy = (y as u64 * from_h / u64::from(height.max(1))) as i32;
            (sx, sy)
        })
    }

    /// Nearest-neighbour upscale by a whole factor.
    pub fn scaled_up(&self, factor: u32) -> Self {
        let factor = factor.max(1);
        let mut out = Self::new(self.width * factor, self.height * factor);
        let out_width = out.width as usize;
        for (index, px) in out.pixels.iter_mut().enumerate() {
            let x = (index % out_width) as u32 / factor;
            let y = (index / out_width) as u32 / factor;
            *px = self.pixels[y as usize * self.width as usize + x as usize];
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const BLUE: Rgba8 = Rgba8::rgb(0, 0, 255);

    #[test]
    fn get_and_set_ignore_coordinates_outside() {
        let mut image = Image::new(2, 2);
        image.set(1, 1, RED);
        image.set(2, 0, RED);
        image.set(-1, 0, RED);
        assert_eq!(image.get(1, 1), RED);
        assert_eq!(image.get(0, 0), Rgba8::TRANSPARENT);
        assert_eq!(image.get(5, 5), Rgba8::TRANSPARENT);
        assert_eq!(image.pixels().iter().filter(|px| **px == RED).count(), 1);
    }

    #[test]
    fn bytes_round_trip() {
        let mut image = Image::new(2, 1);
        image.set(0, 0, Rgba8::new(1, 2, 3, 4));
        image.set(1, 0, Rgba8::new(5, 6, 7, 8));
        let bytes = image.to_rgba_bytes();
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(Image::from_rgba_bytes(2, 1, &bytes), Some(image));
        assert_eq!(Image::from_rgba_bytes(2, 2, &bytes), None);
    }

    #[test]
    fn crop_pads_with_transparency() {
        let mut image = Image::new(3, 3);
        image.set(0, 0, RED);
        image.set(2, 2, BLUE);

        let inside = image.crop(Rect::new(2, 2, 1, 1));
        assert_eq!(inside.pixels(), [BLUE]);

        let overhanging = image.crop(Rect::new(-1, -1, 2, 2));
        assert_eq!(overhanging.get(1, 1), RED);
        assert_eq!(overhanging.get(0, 0), Rgba8::TRANSPARENT);

        assert!(
            image
                .crop(Rect::new(10, 10, 2, 2))
                .content_bounds()
                .is_empty()
        );
    }

    #[test]
    fn blit_replaces_and_clips() {
        let mut stamp = Image::new(2, 2);
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            stamp.set(x, y, RED);
        }

        let mut image = Image::new(3, 3);
        image.set(2, 2, BLUE);
        image.blit(&stamp, Point::new(2, 2));
        assert_eq!(image.get(2, 2), RED);
        assert_eq!(image.pixels().iter().filter(|px| **px == RED).count(), 1);

        image.blit(&stamp, Point::new(-1, -1));
        assert_eq!(image.get(0, 0), RED);
        assert_eq!(image.get(1, 0), Rgba8::TRANSPARENT);
    }

    #[test]
    fn content_bounds_is_tight() {
        let mut image = Image::new(8, 8);
        assert_eq!(image.content_bounds(), Rect::EMPTY);
        image.set(2, 3, RED);
        assert_eq!(image.content_bounds(), Rect::new(2, 3, 1, 1));
        image.set(6, 5, BLUE);
        assert_eq!(image.content_bounds(), Rect::new(2, 3, 5, 3));
        assert_eq!(Image::new(0, 0).content_bounds(), Rect::EMPTY);
    }

    /// ```text
    /// R B
    /// . .
    /// G .
    /// ```
    fn corner_marks() -> Image {
        let mut image = Image::new(2, 3);
        image.set(0, 0, RED);
        image.set(1, 0, BLUE);
        image.set(0, 2, Rgba8::rgb(0, 255, 0));
        image
    }

    #[test]
    fn flips_and_quarter_turns() {
        let image = corner_marks();
        let green = Rgba8::rgb(0, 255, 0);

        let h = image.flipped_horizontal();
        assert_eq!((h.get(0, 0), h.get(1, 0), h.get(1, 2)), (BLUE, RED, green));
        let v = image.flipped_vertical();
        assert_eq!((v.get(0, 2), v.get(1, 2), v.get(0, 0)), (RED, BLUE, green));

        // Clockwise: the top row becomes the right column.
        let cw = image.rotated_clockwise();
        assert_eq!((cw.width(), cw.height()), (3, 2));
        assert_eq!(
            (cw.get(2, 0), cw.get(2, 1), cw.get(0, 0)),
            (RED, BLUE, green)
        );
        let ccw = image.rotated_anticlockwise();
        assert_eq!(
            (ccw.get(0, 1), ccw.get(0, 0), ccw.get(2, 1)),
            (RED, BLUE, green)
        );

        assert_eq!(image.flipped_horizontal().flipped_horizontal(), image);
        assert_eq!(image.rotated_clockwise().rotated_anticlockwise(), image);
        let turned_twice = image.rotated_clockwise().rotated_clockwise();
        assert_eq!(turned_twice, image.flipped_horizontal().flipped_vertical());
    }

    #[test]
    fn scaling_to_a_size_samples_the_nearest_pixel() {
        let image = corner_marks();
        assert_eq!(image.scaled_to(4, 6), image.scaled_up(2));
        assert_eq!(image.scaled_to(2, 3), image);
        // Shrinking keeps a pixel from each block.
        let small = image.scaled_up(4).scaled_to(2, 3);
        assert_eq!(small, image);
        // Uneven factors.
        let wide = image.scaled_to(5, 3);
        assert_eq!(
            (wide.get(0, 0), wide.get(2, 0), wide.get(3, 0)),
            (RED, RED, BLUE)
        );
    }

    #[test]
    fn scaling_up_repeats_pixels() {
        let mut image = Image::new(2, 1);
        image.set(0, 0, RED);
        image.set(1, 0, BLUE);
        let big = image.scaled_up(3);
        assert_eq!((big.width(), big.height()), (6, 3));
        assert_eq!(big.get(2, 2), RED);
        assert_eq!(big.get(3, 0), BLUE);
        assert_eq!(image.scaled_up(1), image);
    }
}
