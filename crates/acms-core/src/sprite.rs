//! The document model (see `PLAN.md`, section 4.1).
//!
//! A [`Sprite`] can be read freely, but it can only be changed through a transaction on the
//! [`Document`](crate::document::Document) that owns it: the mutating methods here are private
//! to this crate.

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;

use crate::color::Rgba8;
use crate::geom::{Point, Rect};
use crate::image::Image;

/// Largest width or height of a sprite. The canvas is shown as a single texture, and this is
/// a size every GPU the app targets can hold.
pub const MAX_SPRITE_SIZE: u32 = 4096;

/// How long a new frame is shown.
pub const DEFAULT_FRAME_MS: u32 = 100;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SpriteError {
    #[error("a sprite must be between 1 and {MAX_SPRITE_SIZE} pixels on each side, not {0}×{1}")]
    InvalidSize(u32, u32),
    #[error("a sprite needs at least one layer")]
    NoLayers,
    #[error("a sprite needs at least one frame")]
    NoFrames,
    #[error("layer id {0} is used twice")]
    DuplicateLayer(u32),
    #[error("there is no layer with id {0}")]
    UnknownLayer(u32),
    #[error("there is no frame {0}")]
    UnknownFrame(usize),
    #[error("pixel data is {got_w}×{got_h} but the area to fill is {want_w}×{want_h}")]
    SizeMismatch {
        want_w: u32,
        want_h: u32,
        got_w: u32,
        got_h: u32,
    },
}

/// Identifies a layer for as long as the sprite exists, however layers are reordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteEntry {
    pub color: Rgba8,
    pub name: Option<String>,
}

/// An ordered list of colours.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    entries: Vec<PaletteEntry>,
}

impl Palette {
    pub fn new(entries: Vec<PaletteEntry>) -> Self {
        Self { entries }
    }

    pub fn from_colors(colors: impl IntoIterator<Item = Rgba8>) -> Self {
        let entries = colors
            .into_iter()
            .map(|color| PaletteEntry { color, name: None })
            .collect();
        Self { entries }
    }

    /// The palette new sprites start with: seven neutrals, then three shades each of eight
    /// hues, then a brown.
    pub fn default_palette() -> Self {
        const COLORS: [u32; 32] = [
            0x000000, 0x1D1F27, 0x3B3F4C, 0x666C7A, 0x9AA1AE, 0xCDD2DA, 0xFFFFFF, //
            0x6E1A2B, 0xC2293A, 0xF0605D, // red
            0x9C4A1A, 0xE8852B, 0xF7B76B, // orange
            0xB58A12, 0xF2CE3B, 0xFAE89A, // yellow
            0x1F6B3A, 0x3FA34D, 0x9BD86B, // green
            0x14596B, 0x2A9D9C, 0x7FD8C4, // teal
            0x1E3A8A, 0x3B6FE0, 0x7FB2F5, // blue
            0x4A2A7A, 0x8A4FD0, 0xC49BF0, // purple
            0x8A2A6B, 0xD9529E, 0xF5A3CC, // pink
            0x4A2E1F, // brown
        ];
        Self::from_colors(
            COLORS
                .into_iter()
                .map(|rgb| Rgba8::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)),
        )
    }

    pub fn entries(&self) -> &[PaletteEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The pixels of one layer in one frame.
///
/// A cel only stores the rectangle that has something in it, so a small drawing on a large
/// canvas stays small. A stored cel is never empty, has no fully transparent border, and its
/// fully transparent pixels are all [`Rgba8::TRANSPARENT`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cel {
    origin: Point,
    /// Shared so that linked cels can point at the same pixels.
    image: Arc<Image>,
}

impl Cel {
    /// Where the top-left pixel of [`Cel::image`] sits on the canvas.
    pub fn origin(&self) -> Point {
        self.origin
    }

    pub fn image(&self) -> &Image {
        &self.image
    }

    /// The canvas rectangle this cel covers.
    pub fn rect(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.image.width(),
            self.image.height(),
        )
    }

    /// The pixel at a canvas position; transparent outside the cel.
    pub fn pixel(&self, x: i32, y: i32) -> Rgba8 {
        self.image.get(x - self.origin.x, y - self.origin.y)
    }
}

/// One cel as handed to [`Sprite::from_parts`]. It may be any size and sit anywhere; the sprite
/// clips and crops it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CelSource {
    pub layer: LayerId,
    pub frame: usize,
    pub origin: Point,
    pub image: Image,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sprite {
    width: u32,
    height: u32,
    palette: Palette,
    /// Bottom to top.
    layers: Vec<Layer>,
    frames: Vec<Frame>,
    /// A missing entry is an empty cel.
    cels: HashMap<(LayerId, usize), Cel>,
}

impl Sprite {
    /// A blank sprite with one layer, one frame and the default palette.
    pub fn new(width: u32, height: u32) -> Result<Self, SpriteError> {
        let layer = Layer {
            id: LayerId(1),
            name: "Layer 1".to_owned(),
        };
        let frame = Frame {
            duration_ms: DEFAULT_FRAME_MS,
        };
        Self::from_parts(
            width,
            height,
            Palette::default_palette(),
            vec![layer],
            vec![frame],
            Vec::new(),
        )
    }

    /// Assembles a sprite, for example from a file. Checks that the pieces fit together.
    pub fn from_parts(
        width: u32,
        height: u32,
        palette: Palette,
        layers: Vec<Layer>,
        frames: Vec<Frame>,
        cels: Vec<CelSource>,
    ) -> Result<Self, SpriteError> {
        let valid = 1..=MAX_SPRITE_SIZE;
        if !valid.contains(&width) || !valid.contains(&height) {
            return Err(SpriteError::InvalidSize(width, height));
        }
        if layers.is_empty() {
            return Err(SpriteError::NoLayers);
        }
        if frames.is_empty() {
            return Err(SpriteError::NoFrames);
        }
        for (index, layer) in layers.iter().enumerate() {
            if layers[..index].iter().any(|other| other.id == layer.id) {
                return Err(SpriteError::DuplicateLayer(layer.id.0));
            }
        }

        let mut sprite = Self {
            width,
            height,
            palette,
            layers,
            frames,
            cels: HashMap::new(),
        };
        for cel in cels {
            sprite.check_cel(cel.layer, cel.frame)?;
            let rect = cel.image.bounds().translated(cel.origin.x, cel.origin.y);
            sprite.write_region(cel.layer, cel.frame, rect, &cel.image);
        }
        Ok(sprite)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The canvas rectangle.
    pub fn bounds(&self) -> Rect {
        Rect::from_size(self.width, self.height)
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// Layers from bottom to top.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The cel of a layer in a frame, or `None` if nothing is drawn there.
    pub fn cel(&self, layer: LayerId, frame: usize) -> Option<&Cel> {
        self.cels.get(&(layer, frame))
    }

    /// Every non-empty cel, in no particular order.
    pub fn cels(&self) -> impl Iterator<Item = (LayerId, usize, &Cel)> {
        self.cels
            .iter()
            .map(|(&(layer, frame), cel)| (layer, frame, cel))
    }

    /// `Ok` if the layer and frame exist.
    pub fn check_cel(&self, layer: LayerId, frame: usize) -> Result<(), SpriteError> {
        if !self.layers.iter().any(|l| l.id == layer) {
            return Err(SpriteError::UnknownLayer(layer.0));
        }
        if frame >= self.frames.len() {
            return Err(SpriteError::UnknownFrame(frame));
        }
        Ok(())
    }

    /// The pixel of one layer at a canvas position; transparent where nothing is drawn.
    pub fn pixel(&self, layer: LayerId, frame: usize, x: i32, y: i32) -> Rgba8 {
        self.cel(layer, frame)
            .map_or(Rgba8::TRANSPARENT, |cel| cel.pixel(x, y))
    }

    /// A copy of a canvas rectangle of one layer.
    pub fn read_region(&self, layer: LayerId, frame: usize, rect: Rect) -> Image {
        match self.cel(layer, frame) {
            Some(cel) => cel
                .image
                .crop(rect.translated(-cel.origin.x, -cel.origin.y)),
            None => Image::new(rect.w, rect.h),
        }
    }

    /// Replaces the pixels of `rect` with `pixels` (which must be `rect`-sized), then re-crops
    /// the cel to its content. Anything outside the canvas is dropped.
    ///
    /// The caller has checked that the layer and frame exist.
    pub(crate) fn write_region(
        &mut self,
        layer: LayerId,
        frame: usize,
        rect: Rect,
        pixels: &Image,
    ) {
        debug_assert_eq!((rect.w, rect.h), (pixels.width(), pixels.height()));
        let target = rect.intersect(self.bounds());
        if target.is_empty() {
            return;
        }

        let old = self.cels.remove(&(layer, frame));
        let old_rect = old.as_ref().map_or(Rect::EMPTY, Cel::rect);
        let grown = old_rect.union(target);

        let mut image = match old {
            Some(cel) if grown == old_rect => Arc::unwrap_or_clone(cel.image),
            Some(cel) => {
                let mut image = Image::new(grown.w, grown.h);
                let at = Point::new(old_rect.x - grown.x, old_rect.y - grown.y);
                image.blit(&cel.image, at);
                image
            }
            None => Image::new(grown.w, grown.h),
        };

        // Only erasing can leave the cel with an empty border to crop away.
        let mut erased = false;
        for y in target.y..target.bottom() {
            for x in target.x..target.right() {
                let color = pixels.get(x - rect.x, y - rect.y).normalized();
                erased |= color.a == 0;
                image.set(x - grown.x, y - grown.y, color);
            }
        }

        let mut cel_rect = grown;
        if erased {
            let content = image.content_bounds();
            if content.is_empty() {
                return;
            }
            if content != image.bounds() {
                image = image.crop(content);
                cel_rect = content.translated(grown.x, grown.y);
            }
        }
        self.cels.insert(
            (layer, frame),
            Cel {
                origin: cel_rect.origin(),
                image: Arc::new(image),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const L1: LayerId = LayerId(1);

    fn filled(w: u32, h: u32, color: Rgba8) -> Image {
        Image::from_pixels(w, h, vec![color; (w * h) as usize]).unwrap()
    }

    #[test]
    fn new_sprite_has_one_layer_one_frame_and_no_pixels() {
        let sprite = Sprite::new(16, 8).unwrap();
        assert_eq!((sprite.width(), sprite.height()), (16, 8));
        assert_eq!(sprite.layers().len(), 1);
        assert_eq!(sprite.frames().len(), 1);
        assert_eq!(sprite.cels().count(), 0);
        assert_eq!(sprite.palette().len(), 32);
        assert_eq!(sprite.pixel(L1, 0, 3, 3), Rgba8::TRANSPARENT);
    }

    #[test]
    fn sizes_outside_the_limits_are_rejected() {
        assert_eq!(Sprite::new(0, 8), Err(SpriteError::InvalidSize(0, 8)));
        assert_eq!(
            Sprite::new(8, MAX_SPRITE_SIZE + 1),
            Err(SpriteError::InvalidSize(8, MAX_SPRITE_SIZE + 1))
        );
        assert!(Sprite::new(MAX_SPRITE_SIZE, 1).is_ok());
    }

    #[test]
    fn writing_grows_the_cel_just_enough() {
        let mut sprite = Sprite::new(32, 32).unwrap();
        sprite.write_region(L1, 0, Rect::new(10, 10, 2, 2), &filled(2, 2, RED));
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(10, 10, 2, 2));

        sprite.write_region(L1, 0, Rect::new(20, 5, 1, 1), &filled(1, 1, RED));
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(10, 5, 11, 7));
        assert_eq!(sprite.pixel(L1, 0, 20, 5), RED);
        assert_eq!(sprite.pixel(L1, 0, 11, 11), RED);
        assert_eq!(sprite.pixel(L1, 0, 15, 8), Rgba8::TRANSPARENT);
    }

    #[test]
    fn erasing_crops_the_cel_and_removes_it_when_nothing_is_left() {
        let mut sprite = Sprite::new(32, 32).unwrap();
        sprite.write_region(L1, 0, Rect::new(4, 4, 6, 1), &filled(6, 1, RED));
        sprite.write_region(L1, 0, Rect::new(7, 4, 3, 1), &Image::new(3, 1));
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(4, 4, 3, 1));

        sprite.write_region(L1, 0, Rect::new(0, 0, 32, 32), &Image::new(32, 32));
        assert!(sprite.cel(L1, 0).is_none());
    }

    #[test]
    fn writes_are_clipped_to_the_canvas() {
        let mut sprite = Sprite::new(4, 4).unwrap();
        sprite.write_region(L1, 0, Rect::new(-2, -2, 4, 4), &filled(4, 4, RED));
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(0, 0, 2, 2));

        sprite.write_region(L1, 0, Rect::new(10, 10, 4, 4), &filled(4, 4, RED));
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(0, 0, 2, 2));
    }

    #[test]
    fn invisible_colour_is_stored_as_plain_transparency() {
        let mut sprite = Sprite::new(4, 4).unwrap();
        let ghost = Rgba8::new(200, 100, 50, 0);
        sprite.write_region(L1, 0, Rect::new(0, 0, 2, 1), &filled(2, 1, RED));
        sprite.write_region(L1, 0, Rect::new(1, 0, 1, 1), &filled(1, 1, ghost));
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(0, 0, 1, 1));
        assert_eq!(sprite.pixel(L1, 0, 1, 0), Rgba8::TRANSPARENT);
    }

    #[test]
    fn read_region_pads_with_transparency() {
        let mut sprite = Sprite::new(8, 8).unwrap();
        sprite.write_region(L1, 0, Rect::new(2, 2, 1, 1), &filled(1, 1, RED));
        let region = sprite.read_region(L1, 0, Rect::new(1, 1, 3, 3));
        assert_eq!(region.get(1, 1), RED);
        assert_eq!(region.content_bounds(), Rect::new(1, 1, 1, 1));
        assert!(
            sprite
                .read_region(L1, 0, Rect::new(5, 5, 2, 2))
                .content_bounds()
                .is_empty()
        );
    }

    #[test]
    fn from_parts_checks_its_pieces() {
        let layer = |id| Layer {
            id: LayerId(id),
            name: format!("Layer {id}"),
        };
        let frame = Frame { duration_ms: 100 };
        let build = |layers, frames, cels| {
            Sprite::from_parts(8, 8, Palette::default(), layers, frames, cels)
        };

        assert_eq!(
            build(vec![], vec![frame], vec![]),
            Err(SpriteError::NoLayers)
        );
        assert_eq!(
            build(vec![layer(1)], vec![], vec![]),
            Err(SpriteError::NoFrames)
        );
        assert_eq!(
            build(vec![layer(1), layer(1)], vec![frame], vec![]),
            Err(SpriteError::DuplicateLayer(1))
        );

        let cel = |layer, frame| CelSource {
            layer: LayerId(layer),
            frame,
            origin: Point::new(6, 6),
            image: filled(4, 4, RED),
        };
        assert_eq!(
            build(vec![layer(1)], vec![frame], vec![cel(2, 0)]),
            Err(SpriteError::UnknownLayer(2))
        );
        assert_eq!(
            build(vec![layer(1)], vec![frame], vec![cel(1, 1)]),
            Err(SpriteError::UnknownFrame(1))
        );

        // A cel hanging over the edge is clipped to the canvas.
        let sprite = build(vec![layer(1)], vec![frame], vec![cel(1, 0)]).unwrap();
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(6, 6, 2, 2));
    }
}
