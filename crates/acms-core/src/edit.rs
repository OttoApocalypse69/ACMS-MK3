//! Edits that act on "the selected pixels of a layer": copy, cut, paste, delete, flip, rotate
//! and scale. Without a selection they act on everything the layer holds in that frame.
//!
//! All of them run inside a transaction, so each is one undo step.

use crate::color::Rgba8;
use crate::document::Transaction;
use crate::geom::{Point, Rect};
use crate::image::Image;
use crate::palette;
use crate::pattern::Pattern;
use crate::selection::Mask;
use crate::sprite::{LayerId, Sprite, SpriteError};
use crate::stroke::{Brush, MAX_BRUSH_SIZE};

/// Pixels taken off a layer, and where their top-left corner was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub image: Image,
    pub origin: Point,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transform {
    FlipHorizontal,
    FlipVertical,
    RotateClockwise,
    RotateAnticlockwise,
    /// Nearest-neighbour scale to this size, keeping the top-left corner in place.
    Scale {
        width: u32,
        height: u32,
    },
}

/// The pixels an edit acts on: the selection, or else the rectangle the layer's cel covers.
/// `None` if that is nothing at all.
pub fn region(sprite: &Sprite, layer: LayerId, frame: usize) -> Option<Mask> {
    let (width, height) = (sprite.width(), sprite.height());
    match sprite.selection() {
        Some(mask) => Some((**mask).clone()),
        None => {
            let rect = sprite.cel(layer, frame)?.rect();
            Some(Mask::from_rect(width, height, rect))
        }
    }
}

/// The layer's pixels inside `mask`, cropped to the mask's bounds; transparent elsewhere.
fn lift(sprite: &Sprite, layer: LayerId, frame: usize, mask: &Mask) -> Image {
    let bounds = mask.bounds();
    let mut image = sprite.read_region(layer, frame, bounds);
    for y in 0..bounds.h as i32 {
        for x in 0..bounds.w as i32 {
            if !mask.contains(bounds.x + x, bounds.y + y) {
                image.set(x, y, Rgba8::TRANSPARENT);
            }
        }
    }
    image
}

/// What Copy would put on the clipboard; `None` if there is nothing visible to copy.
pub fn copy(sprite: &Sprite, layer: LayerId, frame: usize) -> Option<Clip> {
    sprite.check_cel(layer, frame).ok()?;
    let mask = region(sprite, layer, frame)?;
    let image = lift(sprite, layer, frame, &mask);
    let content = image.content_bounds();
    (!content.is_empty()).then(|| Clip {
        image: image.crop(content),
        origin: Point::new(mask.bounds().x + content.x, mask.bounds().y + content.y),
    })
}

/// A rectangle of a layer being rewritten: read once, changed pixel by pixel, written back.
struct Patch {
    layer: LayerId,
    frame: usize,
    rect: Rect,
    pixels: Image,
}

impl Patch {
    fn new(tx: &Transaction<'_>, layer: LayerId, frame: usize, rect: Rect) -> Self {
        let rect = rect.intersect(tx.sprite().bounds());
        Self {
            layer,
            frame,
            rect,
            pixels: tx.sprite().read_region(layer, frame, rect),
        }
    }

    fn set(&mut self, x: i32, y: i32, color: Rgba8) {
        self.pixels.set(x - self.rect.x, y - self.rect.y, color);
    }

    fn write(self, tx: &mut Transaction<'_>) -> Result<(), SpriteError> {
        if self.rect.is_empty() {
            return Ok(());
        }
        tx.set_pixels(self.layer, self.frame, self.rect, &self.pixels)
    }
}

/// Makes the selected pixels transparent.
pub fn clear(tx: &mut Transaction<'_>, layer: LayerId, frame: usize) -> Result<(), SpriteError> {
    tx.sprite().check_cel(layer, frame)?;
    let Some(mask) = region(tx.sprite(), layer, frame) else {
        return Ok(());
    };
    let mut patch = Patch::new(tx, layer, frame, mask.bounds());
    for p in mask.pixels() {
        patch.set(p.x, p.y, Rgba8::TRANSPARENT);
    }
    patch.write(tx)
}

/// Draws `image` onto a layer with its top-left corner at `at`. Transparent pixels of the
/// image leave the layer as it is.
fn stamp_image(patch: &mut Patch, image: &Image, at: Point, only: Option<&Image>) {
    for y in 0..image.height() as i32 {
        for x in 0..image.width() as i32 {
            let color = image.get(x, y);
            let wanted = only.is_none_or(|mask| mask.get(x, y).a != 0);
            if wanted && color.a != 0 {
                patch.set(at.x + x, at.y + y, color);
            }
        }
    }
}

/// Puts a clip onto a layer and selects it, ready to be moved. It lands where it was copied
/// from if that is on the canvas, otherwise in the middle.
pub fn paste(
    tx: &mut Transaction<'_>,
    layer: LayerId,
    frame: usize,
    clip: &Clip,
) -> Result<(), SpriteError> {
    tx.sprite().check_cel(layer, frame)?;
    let canvas = tx.sprite().bounds();
    let (w, h) = (clip.image.width(), clip.image.height());
    let original = Rect::new(clip.origin.x, clip.origin.y, w, h);
    let at = if original.intersect(canvas) == original {
        clip.origin
    } else {
        Point::new(
            (canvas.w as i32 - w as i32) / 2,
            (canvas.h as i32 - h as i32) / 2,
        )
    };
    let rect = Rect::new(at.x, at.y, w, h);

    let mut patch = Patch::new(tx, layer, frame, rect);
    stamp_image(&mut patch, &clip.image, at, None);
    patch.write(tx)?;
    tx.set_selection(Some(Mask::from_rect(canvas.w, canvas.h, rect)))
}

/// Flips, turns or scales the selected pixels in place. A selection follows its pixels.
pub fn transform(
    tx: &mut Transaction<'_>,
    layer: LayerId,
    frame: usize,
    transform: Transform,
) -> Result<(), SpriteError> {
    tx.sprite().check_cel(layer, frame)?;
    let had_selection = tx.sprite().selection().is_some();
    let Some(mask) = region(tx.sprite(), layer, frame) else {
        return Ok(());
    };
    let canvas = tx.sprite().bounds();
    let old = mask.bounds();
    let content = lift(tx.sprite(), layer, frame, &mask);
    let shape = mask.to_image().crop(old);

    let apply = |image: &Image| match transform {
        Transform::FlipHorizontal => image.flipped_horizontal(),
        Transform::FlipVertical => image.flipped_vertical(),
        Transform::RotateClockwise => image.rotated_clockwise(),
        Transform::RotateAnticlockwise => image.rotated_anticlockwise(),
        Transform::Scale { width, height } => image.scaled_to(width.max(1), height.max(1)),
    };
    let (content, shape) = (apply(&content), apply(&shape));
    // A quarter turn swaps width and height; keep the middle where it was.
    let (w, h) = (content.width(), content.height());
    let at = match transform {
        Transform::RotateClockwise | Transform::RotateAnticlockwise => Point::new(
            old.x + (old.w as i32 - w as i32) / 2,
            old.y + (old.h as i32 - h as i32) / 2,
        ),
        _ => old.origin(),
    };
    let new = Rect::new(at.x, at.y, w, h);

    let mut patch = Patch::new(tx, layer, frame, old.union(new));
    for p in mask.pixels() {
        patch.set(p.x, p.y, Rgba8::TRANSPARENT);
    }
    stamp_image(&mut patch, &content, at, Some(&shape));
    patch.write(tx)?;

    if had_selection {
        let moved = Mask::from_fn(canvas.w, canvas.h, |x, y| {
            shape.get(x - at.x, y - at.y).a != 0
        });
        tx.set_selection(Some(moved))?;
    }
    Ok(())
}

/// Every cel of an image layer that may be changed, as `(layer, frame, canvas rectangle)`.
/// Locked layers are left out: they refuse pixel changes.
fn editable_cels(sprite: &Sprite) -> Vec<(LayerId, usize, Rect)> {
    let mut cels: Vec<(LayerId, usize, Rect)> = sprite
        .cels()
        .filter(|(layer, _, _)| {
            sprite
                .layer(*layer)
                .and_then(|layer| layer.as_image())
                .is_some_and(|image| !image.locked)
        })
        .map(|(layer, frame, cel)| (layer, frame, cel.rect()))
        .collect();
    // In a fixed order, so the same edit always records the same undo step.
    cels.sort_by_key(|&(layer, frame, _)| (layer, frame));
    cels
}

/// Changes every pixel of every unlocked layer with `recolor`, which is given the pixel's
/// colour and returns the new one. Returns how many pixels changed.
fn recolor_sprite(
    tx: &mut Transaction<'_>,
    mut recolor: impl FnMut(Rgba8) -> Rgba8,
) -> Result<usize, SpriteError> {
    let mut changed = 0;
    for (layer, frame, rect) in editable_cels(tx.sprite()) {
        let mut pixels = tx.sprite().read_region(layer, frame, rect);
        for y in 0..rect.h as i32 {
            for x in 0..rect.w as i32 {
                let old = pixels.get(x, y);
                let new = recolor(old).normalized();
                if new != old {
                    pixels.set(x, y, new);
                    changed += 1;
                }
            }
        }
        tx.set_pixels(layer, frame, rect, &pixels)?;
    }
    Ok(changed)
}

/// Turns every pixel of colour `from` into `to`, on every unlocked layer and in the palette.
/// Returns how many pixels changed.
pub fn replace_color(
    tx: &mut Transaction<'_>,
    from: Rgba8,
    to: Rgba8,
) -> Result<usize, SpriteError> {
    let (from, to) = (from.normalized(), to.normalized());
    let changed = recolor_sprite(tx, |color| if color == from { to } else { color })?;
    let palette = palette::with_replaced(tx.sprite().palette(), from, to);
    tx.set_palette(palette);
    Ok(changed)
}

/// Gives every pixel of every unlocked layer the palette colour nearest to it, keeping how
/// transparent it is. Returns how many pixels changed.
pub fn reduce_to_palette(tx: &mut Transaction<'_>) -> Result<usize, SpriteError> {
    let palette = tx.sprite().palette().clone();
    if !palette.colors().any(|color| color.a != 0) {
        return Err(SpriteError::EmptyPalette);
    }
    // Sprites have few distinct colours, so each is only looked up once.
    let mut known = std::collections::HashMap::new();
    recolor_sprite(tx, |color| {
        if color.a == 0 {
            return color;
        }
        let solid = Rgba8::rgb(color.r, color.g, color.b);
        let nearest = *known
            .entry(solid)
            .or_insert_with(|| palette::nearest(&palette, solid).unwrap_or(solid));
        Rgba8::new(nearest.r, nearest.g, nearest.b, color.a)
    })
}

/// A brush that stamps the selected pixels of a layer.
pub fn brush_from_selection(
    sprite: &Sprite,
    layer: LayerId,
    frame: usize,
) -> Result<Brush, SpriteError> {
    let clip = copy(sprite, layer, frame).ok_or(SpriteError::EmptyImage)?;
    Brush::custom(&clip.image).ok_or(SpriteError::TooLargeForBrush)
}

/// A dither pattern from the selected rectangle of a layer: where there is a pixel the
/// pattern takes the first colour, where there is none the second.
pub fn pattern_from_selection(
    sprite: &Sprite,
    layer: LayerId,
    frame: usize,
) -> Result<Pattern, SpriteError> {
    sprite.check_cel(layer, frame)?;
    let bounds = region(sprite, layer, frame)
        .map(|mask| mask.bounds())
        .ok_or(SpriteError::EmptyImage)?;
    if bounds.w > MAX_BRUSH_SIZE || bounds.h > MAX_BRUSH_SIZE {
        return Err(SpriteError::TooLargeForBrush);
    }
    let tile = Mask::from_image(&sprite.read_region(layer, frame, bounds));
    Pattern::custom(tile).ok_or(SpriteError::EmptyImage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const BLUE: Rgba8 = Rgba8::rgb(0, 0, 255);
    const L1: LayerId = LayerId(1);

    /// 8×8 with a red 4×2 bar at (1, 1) whose top-left pixel is blue.
    fn document() -> Document {
        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        let mut bar = Image::from_pixels(4, 2, vec![RED; 8]).unwrap();
        bar.set(0, 0, BLUE);
        doc.transact("Paint", |tx| {
            tx.set_pixels(L1, 0, Rect::new(1, 1, 4, 2), &bar)
        })
        .unwrap();
        doc
    }

    fn select(doc: &mut Document, rect: Rect) {
        doc.transact("Select", |tx| {
            tx.set_selection(Some(Mask::from_rect(8, 8, rect)))
        })
        .unwrap();
    }

    fn px(doc: &Document, x: i32, y: i32) -> Rgba8 {
        doc.sprite().pixel(L1, 0, x, y)
    }

    /// Runs an edit and checks it is exactly one undo step that undoes and redoes cleanly.
    fn step(
        doc: &mut Document,
        edit: impl FnOnce(&mut Transaction<'_>) -> Result<(), SpriteError>,
    ) {
        let before = doc.sprite().clone();
        let steps = doc.history().len();
        doc.transact("Edit", edit).unwrap();
        let after = doc.sprite().clone();
        assert_eq!(doc.history().len(), steps + 1);
        doc.undo();
        assert_eq!(*doc.sprite(), before);
        doc.redo();
        assert_eq!(*doc.sprite(), after);
    }

    #[test]
    fn copy_takes_the_selection_or_the_whole_layer() {
        let mut doc = document();
        let all = copy(doc.sprite(), L1, 0).unwrap();
        assert_eq!(all.origin, Point::new(1, 1));
        assert_eq!((all.image.width(), all.image.height()), (4, 2));

        // A selection larger than the content is trimmed to what is visible in it.
        select(&mut doc, Rect::new(3, 0, 5, 8));
        let part = copy(doc.sprite(), L1, 0).unwrap();
        assert_eq!(part.origin, Point::new(3, 1));
        assert_eq!(part.image, Image::from_pixels(2, 2, vec![RED; 4]).unwrap());

        select(&mut doc, Rect::new(6, 6, 2, 2));
        assert_eq!(copy(doc.sprite(), L1, 0), None);
    }

    #[test]
    fn clear_empties_only_the_selection() {
        let mut doc = document();
        select(&mut doc, Rect::new(3, 0, 5, 8));
        step(&mut doc, |tx| clear(tx, L1, 0));
        assert_eq!(px(&doc, 2, 1), RED);
        assert_eq!(px(&doc, 3, 1), Rgba8::TRANSPARENT);

        // Without a selection the whole layer goes.
        doc.transact("Deselect", |tx| tx.set_selection(None))
            .unwrap();
        step(&mut doc, |tx| clear(tx, L1, 0));
        assert_eq!(doc.sprite().cels().count(), 0);
    }

    #[test]
    fn paste_lands_where_it_came_from_and_selects_itself() {
        let mut doc = document();
        let clip = copy(doc.sprite(), L1, 0).unwrap();
        doc.transact("Clear", |tx| clear(tx, L1, 0)).unwrap();

        step(&mut doc, |tx| paste(tx, L1, 0, &clip));
        assert_eq!((px(&doc, 1, 1), px(&doc, 4, 2)), (BLUE, RED));
        assert_eq!(
            doc.sprite().selection().unwrap().bounds(),
            Rect::new(1, 1, 4, 2)
        );

        // A clip from somewhere off this canvas is centred instead.
        let far = Clip {
            origin: Point::new(50, 50),
            ..clip
        };
        doc.transact("Paste", |tx| paste(tx, L1, 0, &far)).unwrap();
        assert_eq!(
            doc.sprite().selection().unwrap().bounds(),
            Rect::new(2, 3, 4, 2)
        );
        assert_eq!(px(&doc, 2, 3), BLUE);
    }

    #[test]
    fn pasting_keeps_what_shows_through_the_clip() {
        let mut doc = document();
        let mut image = Image::new(2, 1);
        image.set(1, 0, BLUE);
        let clip = Clip {
            image,
            origin: Point::new(2, 2),
        };
        doc.transact("Paste", |tx| paste(tx, L1, 0, &clip)).unwrap();
        // The clip's transparent pixel left the red one alone.
        assert_eq!((px(&doc, 2, 2), px(&doc, 3, 2)), (RED, BLUE));
    }

    #[test]
    fn flipping_mirrors_within_the_region() {
        let mut doc = document();
        step(&mut doc, |tx| {
            transform(tx, L1, 0, Transform::FlipHorizontal)
        });
        assert_eq!((px(&doc, 4, 1), px(&doc, 1, 1)), (BLUE, RED));
        step(&mut doc, |tx| transform(tx, L1, 0, Transform::FlipVertical));
        assert_eq!((px(&doc, 4, 2), px(&doc, 4, 1)), (BLUE, RED));
        assert!(doc.sprite().selection().is_none());

        // With a selection only that part flips, and the selection stays on it.
        let mut doc = document();
        select(&mut doc, Rect::new(1, 1, 2, 2));
        step(&mut doc, |tx| {
            transform(tx, L1, 0, Transform::FlipHorizontal)
        });
        assert_eq!(
            (px(&doc, 2, 1), px(&doc, 1, 1), px(&doc, 3, 1)),
            (BLUE, RED, RED)
        );
        assert_eq!(
            doc.sprite().selection().unwrap().bounds(),
            Rect::new(1, 1, 2, 2)
        );
    }

    #[test]
    fn a_quarter_turn_swaps_the_sides_around_the_middle() {
        let mut doc = document();
        select(&mut doc, Rect::new(1, 1, 4, 2));
        step(&mut doc, |tx| {
            transform(tx, L1, 0, Transform::RotateClockwise)
        });
        // 4×2 at (1, 1) becomes 2×4 at (2, 0); the blue corner goes top-right.
        let bounds = doc.sprite().selection().unwrap().bounds();
        assert_eq!(bounds, Rect::new(2, 0, 2, 4));
        assert_eq!(doc.sprite().cel(L1, 0).unwrap().rect(), bounds);
        assert_eq!((px(&doc, 3, 0), px(&doc, 2, 0)), (BLUE, RED));

        step(&mut doc, |tx| {
            transform(tx, L1, 0, Transform::RotateAnticlockwise)
        });
        assert_eq!(
            doc.sprite().cel(L1, 0).unwrap().rect(),
            Rect::new(1, 1, 4, 2)
        );
        assert_eq!(px(&doc, 1, 1), BLUE);
    }

    #[test]
    fn scaling_the_selection_keeps_pixels_hard_edged() {
        let mut doc = document();
        select(&mut doc, Rect::new(1, 1, 2, 2));
        let scale = Transform::Scale {
            width: 4,
            height: 4,
        };
        step(&mut doc, |tx| transform(tx, L1, 0, scale));
        // The blue pixel became a 2×2 block; the selection grew with it.
        for (x, y) in [(1, 1), (2, 1), (1, 2), (2, 2)] {
            assert_eq!(px(&doc, x, y), BLUE);
        }
        assert_eq!(px(&doc, 3, 1), RED);
        assert_eq!(
            doc.sprite().selection().unwrap().bounds(),
            Rect::new(1, 1, 4, 4)
        );
    }

    #[test]
    fn edits_on_a_locked_layer_or_a_group_are_refused() {
        let mut doc = document();
        doc.transact("Lock", |tx| {
            tx.update_layer(L1, |props| {
                if let Some(image) = &mut props.image {
                    image.locked = true;
                }
            })
        })
        .unwrap();
        let before = doc.sprite().clone();
        let result = doc.transact("Flip", |tx| transform(tx, L1, 0, Transform::FlipHorizontal));
        assert_eq!(result, Err(SpriteError::LayerLocked(1)));
        assert_eq!(
            doc.transact("Clear", |tx| clear(tx, L1, 0)),
            Err(SpriteError::LayerLocked(1))
        );
        assert_eq!(*doc.sprite(), before);
        assert!(copy(doc.sprite(), LayerId(9), 0).is_none());
    }
    #[test]
    fn replacing_a_colour_reaches_every_unlocked_layer_and_the_palette() {
        use crate::sprite::{Palette, Slot};

        let mut doc = document();
        let upper = doc
            .transact("Add", |tx| tx.add_layer(Slot::new(None, 1), "Upper", false))
            .unwrap();
        let locked = doc
            .transact("Add", |tx| {
                tx.add_layer(Slot::new(None, 2), "Locked", false)
            })
            .unwrap();
        doc.transact("Setup", |tx| {
            let red = Image::from_pixels(1, 1, vec![RED]).unwrap();
            tx.set_pixels(upper, 0, Rect::new(7, 7, 1, 1), &red)?;
            tx.set_pixels(locked, 0, Rect::new(0, 7, 1, 1), &red)?;
            tx.update_layer(locked, |props| {
                if let Some(image) = &mut props.image {
                    image.locked = true;
                }
            })?;
            tx.set_palette(Palette::from_colors([RED, BLUE]).with_ramps(vec![vec![BLUE, RED]]));
            Ok::<_, SpriteError>(())
        })
        .unwrap();
        let before = doc.sprite().clone();

        let green = Rgba8::rgb(0, 255, 0);
        let changed = doc
            .transact("Replace", |tx| replace_color(tx, RED, green))
            .unwrap();
        assert_eq!(changed, 8);
        let sprite = doc.sprite();
        assert_eq!(sprite.pixel(L1, 0, 2, 1), green);
        assert_eq!(sprite.pixel(L1, 0, 1, 1), BLUE);
        assert_eq!(sprite.pixel(upper, 0, 7, 7), green);
        assert_eq!(sprite.pixel(locked, 0, 0, 7), RED);
        assert_eq!(sprite.palette().colors().collect::<Vec<_>>(), [green, BLUE]);
        assert_eq!(sprite.palette().ramps(), [vec![BLUE, green]]);
        assert_eq!(doc.history().last(), Some("Replace"));

        let after = doc.sprite().clone();
        doc.undo();
        assert_eq!(*doc.sprite(), before);
        doc.redo();
        assert_eq!(*doc.sprite(), after);
    }

    #[test]
    fn reducing_to_the_palette_keeps_transparency() {
        use crate::sprite::Palette;

        let mut doc = Document::new(Sprite::new(4, 1).unwrap());
        let pixels = vec![
            Rgba8::rgb(250, 10, 10),
            Rgba8::new(10, 10, 200, 100),
            Rgba8::rgb(30, 30, 30),
            Rgba8::TRANSPARENT,
        ];
        doc.transact("Paint", |tx| {
            tx.set_palette(Palette::from_colors([RED, BLUE, Rgba8::BLACK]));
            let image = Image::from_pixels(4, 1, pixels).unwrap();
            tx.set_pixels(L1, 0, Rect::new(0, 0, 4, 1), &image)
        })
        .unwrap();
        let before = doc.sprite().clone();

        let changed = doc.transact("Reduce", reduce_to_palette).unwrap();
        assert_eq!(changed, 3);
        let row: Vec<Rgba8> = (0..4).map(|x| doc.sprite().pixel(L1, 0, x, 0)).collect();
        assert_eq!(
            row,
            [
                RED,
                Rgba8::new(0, 0, 255, 100),
                Rgba8::BLACK,
                Rgba8::TRANSPARENT
            ]
        );
        // Already reduced: nothing to do, and no undo step.
        let steps = doc.history().len();
        assert_eq!(doc.transact("Reduce", reduce_to_palette), Ok(0));
        assert_eq!(doc.history().len(), steps);
        doc.undo();
        assert_eq!(*doc.sprite(), before);

        doc.transact("Empty", |tx| {
            tx.set_palette(Palette::default());
            Ok::<_, SpriteError>(())
        })
        .unwrap();
        assert_eq!(
            doc.transact("Reduce", reduce_to_palette),
            Err(SpriteError::EmptyPalette)
        );
    }

    #[test]
    fn a_brush_and_a_pattern_can_be_made_from_the_selection() {
        let mut doc = document();
        // No selection: the whole bar.
        let brush = brush_from_selection(doc.sprite(), L1, 0).unwrap();
        assert_eq!(brush.bounds_at(Point::new(0, 0)), Rect::new(-2, -1, 4, 2));
        assert_eq!(brush.image().unwrap().get(0, 0), BLUE);

        // A selection reaching past the bar: the brush is cropped to the pixels, the
        // pattern keeps the empty part as its second colour.
        doc.transact("Select", |tx| {
            tx.set_selection(Some(Mask::from_rect(8, 8, Rect::new(4, 1, 2, 2))))
        })
        .unwrap();
        let brush = brush_from_selection(doc.sprite(), L1, 0).unwrap();
        assert_eq!(brush.footprint().len(), 2);
        let pattern = pattern_from_selection(doc.sprite(), L1, 0).unwrap();
        assert!(pattern.picks_first(0, 0, 0));
        assert!(!pattern.picks_first(1, 0, 0));
        assert!(pattern.picks_first(2, 1, 0));

        doc.transact("Select", |tx| {
            tx.set_selection(Some(Mask::from_rect(8, 8, Rect::new(6, 6, 2, 2))))
        })
        .unwrap();
        assert_eq!(
            brush_from_selection(doc.sprite(), L1, 0),
            Err(SpriteError::EmptyImage)
        );
    }
}
