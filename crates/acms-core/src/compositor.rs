//! Flattens the layers of one frame into a single image (see `PLAN.md`, section 4.6).

use crate::color::Rgba8;
use crate::geom::{Point, Rect};
use crate::image::Image;
use crate::sprite::{BlendMode, Cel, LayerId, Sprite};
use crate::stroke::StrokeBuffer;

/// A stroke that is still being drawn, shown as if it were already part of its cel.
#[derive(Clone, Copy)]
pub struct Overlay<'a> {
    pub layer: LayerId,
    pub frame: usize,
    pub buffer: &'a StrokeBuffer,
}

/// `a * b / 255`, rounded.
fn mul(a: u32, b: u32) -> u32 {
    (a * b + 127) / 255
}

/// A colour with its alpha scaled by a layer opacity (0 to 255).
pub fn with_opacity(color: Rgba8, opacity: u8) -> Rgba8 {
    let alpha = mul(u32::from(color.a), u32::from(opacity)) as u8;
    Rgba8::new(color.r, color.g, color.b, alpha).normalized()
}

/// One colour channel of `src` blended with the same channel of `dst`, ignoring alpha.
fn blend_channel(mode: BlendMode, src: u8, dst: u8) -> u8 {
    let (s, d) = (u32::from(src), u32::from(dst));
    let value = match mode {
        BlendMode::Normal => s,
        BlendMode::Multiply => mul(s, d),
        BlendMode::Screen => s + d - mul(s, d),
        // Multiply where what is below is dark, screen where it is light.
        BlendMode::Overlay if d < 128 => mul(2 * s, d),
        BlendMode::Overlay => 255 - mul(2 * (255 - s), 255 - d),
        BlendMode::Add => (s + d).min(255),
    };
    value.min(255) as u8
}

/// `src` drawn on top of `dst` with a blend mode.
///
/// The blend only shows where there is something below: on a transparent backdrop every mode
/// looks like [`BlendMode::Normal`], and on a translucent one it is mixed in proportionally.
pub fn blend(mode: BlendMode, src: Rgba8, dst: Rgba8) -> Rgba8 {
    if mode == BlendMode::Normal || dst.a == 0 || src.a == 0 {
        return src.over(dst);
    }
    let below = u32::from(dst.a);
    let mix = |s: u8, d: u8| {
        let blended = u32::from(blend_channel(mode, s, d));
        (mul(u32::from(s), 255 - below) + mul(blended, below)) as u8
    };
    let mixed = Rgba8::new(
        mix(src.r, dst.r),
        mix(src.g, dst.g),
        mix(src.b, dst.b),
        src.a,
    );
    mixed.over(dst)
}

/// Flattens a canvas rectangle of one frame. The result is `rect`-sized; anything outside the
/// canvas is transparent.
///
/// Layers are painted bottom to top, each with its opacity and blend mode. A hidden layer, or
/// one inside a hidden group, is skipped.
pub fn compose_rect(
    sprite: &Sprite,
    frame: usize,
    overlay: Option<Overlay<'_>>,
    rect: Rect,
) -> Image {
    let mut out = Image::new(rect.w, rect.h);
    let visible = rect.intersect(sprite.bounds());

    for entry in sprite.walk() {
        let Some(image) = entry.layer.as_image() else {
            continue;
        };
        if !entry.shown || image.opacity == 0 {
            continue;
        }
        let id = entry.layer.id;
        let cel = sprite.cel(id, frame);
        let overlay = overlay.filter(|o| o.layer == id && o.frame == frame);

        // Only the part of the rectangle this layer has anything in.
        let cel_rect = cel.map_or(Rect::EMPTY, Cel::rect);
        let overlay_rect = overlay.map_or(Rect::EMPTY, |o| o.buffer.bounds());
        let covered = cel_rect.union(overlay_rect).intersect(visible);

        for y in covered.y..covered.bottom() {
            for x in covered.x..covered.right() {
                let color = overlay
                    .and_then(|o| o.buffer.get(x, y))
                    .unwrap_or_else(|| cel.map_or(Rgba8::TRANSPARENT, |cel| cel.pixel(x, y)));
                let color = with_opacity(color, image.opacity);
                if color.a != 0 {
                    let (ox, oy) = (x - rect.x, y - rect.y);
                    out.set(ox, oy, blend(image.blend, color, out.get(ox, oy)));
                }
            }
        }
    }
    out
}

/// Flattens a whole frame.
pub fn compose(sprite: &Sprite, frame: usize) -> Image {
    compose_rect(sprite, frame, None, sprite.bounds())
}

/// The flattened picture of one frame, kept up to date by recomposing only what changed.
#[derive(Default)]
pub struct FrameCache {
    image: Image,
    frame: usize,
    valid: bool,
}

impl FrameCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the picture, e.g. because a different sprite is shown. The next
    /// [`FrameCache::refresh`] recomposes all of it.
    pub fn invalidate(&mut self) {
        self.valid = false;
    }

    /// Brings the picture up to date. `damage` is the canvas area that changed since the last
    /// call. Returns the area of the picture that was redrawn: all of it the first time or
    /// when the frame changed, otherwise the damaged part.
    pub fn refresh(
        &mut self,
        sprite: &Sprite,
        frame: usize,
        overlay: Option<Overlay<'_>>,
        damage: Rect,
    ) -> Rect {
        let bounds = sprite.bounds();
        if !self.valid || self.frame != frame || self.image.bounds() != bounds {
            self.image = compose_rect(sprite, frame, overlay, bounds);
            self.frame = frame;
            self.valid = true;
            return bounds;
        }

        let damage = damage.intersect(bounds);
        if !damage.is_empty() {
            let part = compose_rect(sprite, frame, overlay, damage);
            self.image.blit(&part, Point::new(damage.x, damage.y));
        }
        damage
    }

    /// The flattened frame as of the last [`FrameCache::refresh`].
    pub fn image(&self) -> &Image {
        &self.image
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::sprite::{CelSource, Frame, ImageLayer, Layer, LayerKind, Palette, Slot};
    use crate::stroke::{Brush, BrushShape, Ink, Stroke};

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const BLUE: Rgba8 = Rgba8::rgb(0, 0, 255);

    fn filled(w: u32, h: u32, color: Rgba8) -> Image {
        Image::from_pixels(w, h, vec![color; (w * h) as usize]).unwrap()
    }

    fn cel(layer: u32, x: i32, y: i32, image: Image) -> CelSource {
        CelSource {
            layer: LayerId(layer),
            frame: 0,
            origin: Point::new(x, y),
            image,
        }
    }

    fn sprite_with(layers: Vec<Layer>, cels: Vec<CelSource>) -> Sprite {
        let frames = vec![Frame { duration_ms: 100 }];
        Sprite::from_parts(8, 8, Palette::default(), layers, frames, cels).unwrap()
    }

    fn two_layer_sprite(cels: Vec<CelSource>) -> Sprite {
        let layers = [1, 2]
            .map(|id| Layer::image(LayerId(id), format!("Layer {id}")))
            .to_vec();
        sprite_with(layers, cels)
    }

    /// An image layer with the given look.
    fn styled(id: u32, opacity: u8, blend: BlendMode, visible: bool) -> Layer {
        let mut layer = Layer::image(LayerId(id), format!("Layer {id}"));
        layer.visible = visible;
        layer.kind = LayerKind::Image(ImageLayer {
            opacity,
            blend,
            ..ImageLayer::default()
        });
        layer
    }

    #[test]
    fn an_empty_sprite_is_transparent() {
        let sprite = Sprite::new(4, 3).unwrap();
        assert_eq!(compose(&sprite, 0), Image::new(4, 3));
    }

    #[test]
    fn upper_layers_cover_lower_ones() {
        let sprite = two_layer_sprite(vec![
            cel(1, 0, 0, filled(4, 1, RED)),
            cel(2, 2, 0, filled(4, 1, BLUE)),
        ]);
        let flat = compose(&sprite, 0);
        assert_eq!(flat.get(0, 0), RED);
        assert_eq!(flat.get(1, 0), RED);
        assert_eq!(flat.get(2, 0), BLUE);
        assert_eq!(flat.get(5, 0), BLUE);
        assert_eq!(flat.get(6, 0), Rgba8::TRANSPARENT);
    }

    #[test]
    fn translucent_pixels_blend_with_what_is_below() {
        let half_white = Rgba8::new(255, 255, 255, 128);
        let sprite = two_layer_sprite(vec![
            cel(1, 0, 0, filled(1, 1, Rgba8::BLACK)),
            cel(2, 0, 0, filled(2, 1, half_white)),
        ]);
        let flat = compose(&sprite, 0);
        assert_eq!(flat.get(0, 0), Rgba8::rgb(128, 128, 128));
        assert_eq!(flat.get(1, 0), half_white);
    }

    #[test]
    fn a_rectangle_matches_the_same_part_of_the_full_image() {
        let sprite = two_layer_sprite(vec![
            cel(1, 1, 1, filled(5, 5, RED)),
            cel(2, 3, 0, filled(2, 8, BLUE)),
        ]);
        let full = compose(&sprite, 0);
        for rect in [
            Rect::new(0, 0, 8, 8),
            Rect::new(2, 2, 3, 3),
            Rect::new(7, 7, 1, 1),
            Rect::new(-2, -2, 6, 6),
            Rect::new(6, 6, 10, 10),
        ] {
            assert_eq!(
                compose_rect(&sprite, 0, None, rect),
                full.crop(rect),
                "{rect:?}"
            );
        }
    }

    #[test]
    fn the_overlay_replaces_pixels_of_its_own_layer_only() {
        let sprite = two_layer_sprite(vec![
            cel(1, 0, 0, filled(8, 1, RED)),
            cel(2, 0, 0, filled(2, 1, BLUE)),
        ]);

        // Erase the bottom layer's row; the blue pixels above it must stay.
        let mut stroke = Stroke::begin(
            &sprite,
            LayerId(1),
            0,
            Brush::default(),
            Ink::Erase,
            StrokeBuffer::new(),
        )
        .unwrap();
        stroke.extend_to(&sprite, Point::new(0, 0));
        stroke.extend_to(&sprite, Point::new(4, 0));

        let overlay = Overlay {
            layer: stroke.layer(),
            frame: stroke.frame(),
            buffer: stroke.buffer(),
        };
        let flat = compose_rect(&sprite, 0, Some(overlay), sprite.bounds());
        assert_eq!(flat.get(0, 0), BLUE);
        assert_eq!(flat.get(1, 0), BLUE);
        assert_eq!(flat.get(2, 0), Rgba8::TRANSPARENT);
        assert_eq!(flat.get(4, 0), Rgba8::TRANSPARENT);
        assert_eq!(flat.get(5, 0), RED);

        // Without the overlay nothing has changed.
        assert_eq!(compose(&sprite, 0).get(3, 0), RED);
    }

    #[test]
    fn an_overlay_on_an_empty_layer_is_shown() {
        let sprite = Sprite::new(8, 8).unwrap();
        let mut stroke = Stroke::begin(
            &sprite,
            LayerId(1),
            0,
            Brush::new(BrushShape::Square, 2),
            Ink::Simple(RED),
            StrokeBuffer::new(),
        )
        .unwrap();
        stroke.extend_to(&sprite, Point::new(4, 4));
        let overlay = Overlay {
            layer: stroke.layer(),
            frame: stroke.frame(),
            buffer: stroke.buffer(),
        };
        let flat = compose_rect(&sprite, 0, Some(overlay), sprite.bounds());
        assert_eq!(flat.content_bounds(), Rect::new(3, 3, 2, 2));
    }

    #[test]
    fn opacity_scales_alpha() {
        assert_eq!(with_opacity(RED, 255), RED);
        assert_eq!(with_opacity(RED, 128), Rgba8::new(255, 0, 0, 128));
        assert_eq!(with_opacity(RED, 0), Rgba8::TRANSPARENT);
        assert_eq!(
            with_opacity(Rgba8::new(10, 20, 30, 100), 128),
            Rgba8::new(10, 20, 30, 50)
        );

        let layers = vec![
            styled(1, 255, BlendMode::Normal, true),
            styled(2, 128, BlendMode::Normal, true),
        ];
        let sprite = sprite_with(
            layers,
            vec![
                cel(1, 0, 0, filled(1, 1, Rgba8::BLACK)),
                cel(2, 0, 0, filled(2, 1, Rgba8::WHITE)),
            ],
        );
        let flat = compose(&sprite, 0);
        assert_eq!(flat.get(0, 0), Rgba8::rgb(128, 128, 128));
        assert_eq!(flat.get(1, 0), Rgba8::new(255, 255, 255, 128));
    }

    #[test]
    fn hidden_layers_and_hidden_groups_are_skipped() {
        let group = |visible| {
            let mut group = Layer::group(
                LayerId(3),
                "Group",
                vec![styled(2, 255, BlendMode::Normal, true)],
            );
            group.visible = visible;
            group
        };
        let cels = || {
            vec![
                cel(1, 0, 0, filled(2, 1, RED)),
                cel(2, 1, 0, filled(2, 1, BLUE)),
            ]
        };

        let shown = sprite_with(
            vec![styled(1, 255, BlendMode::Normal, true), group(true)],
            cels(),
        );
        assert_eq!(compose(&shown, 0).get(1, 0), BLUE);

        let group_hidden = sprite_with(
            vec![styled(1, 255, BlendMode::Normal, true), group(false)],
            cels(),
        );
        let flat = compose(&group_hidden, 0);
        assert_eq!(flat.get(1, 0), RED);
        assert_eq!(flat.get(2, 0), Rgba8::TRANSPARENT);

        let bottom_hidden = sprite_with(
            vec![styled(1, 255, BlendMode::Normal, false), group(true)],
            cels(),
        );
        let flat = compose(&bottom_hidden, 0);
        assert_eq!(flat.get(0, 0), Rgba8::TRANSPARENT);
        assert_eq!(flat.get(1, 0), BLUE);
    }

    #[test]
    fn blend_modes_on_an_opaque_backdrop() {
        let below = Rgba8::rgb(200, 100, 40);
        let above = Rgba8::rgb(100, 200, 250);
        let result = |mode| blend(mode, above, below);

        assert_eq!(result(BlendMode::Normal), above);
        assert_eq!(result(BlendMode::Multiply), Rgba8::rgb(78, 78, 39));
        assert_eq!(result(BlendMode::Screen), Rgba8::rgb(222, 222, 251));
        // Overlay: screen for the bright red channel, multiply for the darker ones.
        assert_eq!(result(BlendMode::Overlay), Rgba8::rgb(188, 157, 78));
        assert_eq!(result(BlendMode::Add), Rgba8::rgb(255, 255, 255));
    }

    #[test]
    fn blend_modes_have_their_identities() {
        let color = Rgba8::rgb(37, 150, 222);
        // White leaves multiply alone, black leaves screen and add alone.
        assert_eq!(blend(BlendMode::Multiply, Rgba8::WHITE, color), color);
        assert_eq!(
            blend(BlendMode::Multiply, Rgba8::BLACK, color),
            Rgba8::BLACK
        );
        assert_eq!(blend(BlendMode::Screen, Rgba8::BLACK, color), color);
        assert_eq!(blend(BlendMode::Screen, Rgba8::WHITE, color), Rgba8::WHITE);
        assert_eq!(blend(BlendMode::Add, Rgba8::BLACK, color), color);
        // Multiply and screen do not care which colour is on top.
        let other = Rgba8::rgb(90, 14, 201);
        for mode in [BlendMode::Multiply, BlendMode::Screen, BlendMode::Add] {
            assert_eq!(blend(mode, color, other), blend(mode, other, color));
        }
    }

    #[test]
    fn on_nothing_every_blend_mode_is_just_the_colour() {
        let color = Rgba8::new(100, 200, 250, 180);
        for mode in BlendMode::ALL {
            assert_eq!(blend(mode, color, Rgba8::TRANSPARENT), color, "{mode:?}");
            assert_eq!(blend(mode, Rgba8::TRANSPARENT, color), color, "{mode:?}");
        }
    }

    #[test]
    fn a_translucent_source_blends_part_of_the_way() {
        let below = Rgba8::rgb(200, 100, 40);
        let above = Rgba8::new(100, 200, 250, 128);
        let full = blend(BlendMode::Multiply, Rgba8::rgb(100, 200, 250), below);
        let half = blend(BlendMode::Multiply, above, below);
        // Opaque result, each channel about halfway between the backdrop and the full blend.
        assert_eq!(half.a, 255);
        for (h, (f, b)) in [(half.r, (full.r, below.r)), (half.g, (full.g, below.g))] {
            let midpoint = (i32::from(f) + i32::from(b)) / 2;
            assert!((i32::from(h) - midpoint).abs() <= 1, "{h} vs {midpoint}");
        }
    }

    #[test]
    fn layers_use_their_blend_mode() {
        let layers = vec![
            styled(1, 255, BlendMode::Normal, true),
            styled(2, 255, BlendMode::Multiply, true),
        ];
        let sprite = sprite_with(
            layers,
            vec![
                cel(1, 0, 0, filled(1, 1, Rgba8::rgb(200, 100, 40))),
                cel(2, 0, 0, filled(2, 1, Rgba8::rgb(100, 200, 250))),
            ],
        );
        let flat = compose(&sprite, 0);
        assert_eq!(flat.get(0, 0), Rgba8::rgb(78, 78, 39));
        // Where the multiply layer has nothing below it, it shows as itself.
        assert_eq!(flat.get(1, 0), Rgba8::rgb(100, 200, 250));
    }

    #[test]
    fn the_cache_matches_a_full_recomposition_after_every_kind_of_change() {
        let mut doc = Document::new(Sprite::new(16, 16).unwrap());
        let mut cache = FrameCache::new();
        let check = |doc: &mut Document, cache: &mut FrameCache| {
            let damage = doc.take_damage();
            cache.refresh(doc.sprite(), 0, None, damage);
            assert_eq!(*cache.image(), compose(doc.sprite(), 0));
        };
        let paint = |doc: &mut Document, layer, rect: Rect, color| {
            doc.transact("Paint", |tx| {
                tx.set_pixels(layer, 0, rect, &filled(rect.w, rect.h, color))
            })
            .unwrap();
        };

        // The first refresh draws everything, whatever the damage says.
        assert_eq!(
            cache.refresh(doc.sprite(), 0, None, Rect::EMPTY),
            Rect::from_size(16, 16)
        );

        paint(&mut doc, LayerId(1), Rect::new(2, 2, 6, 6), RED);
        check(&mut doc, &mut cache);

        let upper = doc
            .transact("Add", |tx| tx.add_layer(Slot::new(None, 1), "Upper", false))
            .unwrap();
        paint(
            &mut doc,
            upper,
            Rect::new(5, 5, 8, 8),
            Rgba8::new(0, 0, 255, 150),
        );
        check(&mut doc, &mut cache);

        let set = |doc: &mut Document, edit: fn(&mut ImageLayer)| {
            doc.transact("Props", |tx| {
                tx.update_layer(upper, |props| {
                    if let Some(image) = &mut props.image {
                        edit(image);
                    }
                })
            })
            .unwrap();
        };
        set(&mut doc, |image| image.opacity = 90);
        check(&mut doc, &mut cache);
        set(&mut doc, |image| image.blend = BlendMode::Screen);
        check(&mut doc, &mut cache);

        doc.transact("Hide", |tx| {
            tx.update_layer(upper, |props| props.visible = false)
        })
        .unwrap();
        check(&mut doc, &mut cache);
        doc.undo();
        check(&mut doc, &mut cache);

        doc.transact("Move", |tx| tx.move_layer(upper, Slot::new(None, 0)))
            .unwrap();
        check(&mut doc, &mut cache);
        doc.transact("Duplicate", |tx| tx.duplicate_layer(upper))
            .unwrap();
        check(&mut doc, &mut cache);
        doc.transact("Delete", |tx| tx.remove_layer(upper)).unwrap();
        check(&mut doc, &mut cache);
        doc.transact("Flatten", |tx| tx.flatten()).unwrap();
        check(&mut doc, &mut cache);

        while doc.undo() {
            check(&mut doc, &mut cache);
        }
        while doc.redo() {
            check(&mut doc, &mut cache);
        }
    }

    #[test]
    fn the_cache_only_redraws_the_damage() {
        let sprite = two_layer_sprite(vec![cel(1, 0, 0, filled(8, 8, RED))]);
        let mut cache = FrameCache::new();
        cache.refresh(&sprite, 0, None, Rect::EMPTY);

        assert_eq!(cache.refresh(&sprite, 0, None, Rect::EMPTY), Rect::EMPTY);
        assert_eq!(
            cache.refresh(&sprite, 0, None, Rect::new(6, 6, 10, 10)),
            Rect::new(6, 6, 2, 2)
        );

        cache.invalidate();
        assert_eq!(
            cache.refresh(&sprite, 0, None, Rect::EMPTY),
            Rect::from_size(8, 8)
        );
        // A sprite of another size is noticed even without being told.
        let other = Sprite::new(4, 4).unwrap();
        assert_eq!(
            cache.refresh(&other, 0, None, Rect::EMPTY),
            Rect::from_size(4, 4)
        );
    }
}
