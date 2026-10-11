//! Flattens the layers of one frame into a single image (see `PLAN.md`, section 4.6).

use crate::color::Rgba8;
use crate::geom::Rect;
use crate::image::Image;
use crate::sprite::{LayerId, Sprite};
use crate::stroke::StrokeBuffer;

/// A stroke that is still being drawn, shown as if it were already part of its cel.
#[derive(Clone, Copy)]
pub struct Overlay<'a> {
    pub layer: LayerId,
    pub frame: usize,
    pub buffer: &'a StrokeBuffer,
}

/// Flattens a canvas rectangle of one frame. The result is `rect`-sized; anything outside the
/// canvas is transparent.
///
/// Passing only the rectangle that changed is what keeps redrawing cheap while painting.
pub fn compose_rect(
    sprite: &Sprite,
    frame: usize,
    overlay: Option<Overlay<'_>>,
    rect: Rect,
) -> Image {
    let mut out = Image::new(rect.w, rect.h);
    let visible = rect.intersect(sprite.bounds());

    for layer in sprite.layers() {
        let cel = sprite.cel(layer.id, frame);
        let overlay = overlay.filter(|o| o.layer == layer.id && o.frame == frame);
        if cel.is_none() && overlay.is_none() {
            continue;
        }

        for y in visible.y..visible.bottom() {
            for x in visible.x..visible.right() {
                let color = overlay
                    .and_then(|o| o.buffer.get(x, y))
                    .unwrap_or_else(|| cel.map_or(Rgba8::TRANSPARENT, |cel| cel.pixel(x, y)));
                if color.a != 0 {
                    let (ox, oy) = (x - rect.x, y - rect.y);
                    out.set(ox, oy, color.over(out.get(ox, oy)));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;
    use crate::sprite::{CelSource, Frame, Layer, Palette};
    use crate::stroke::{Brush, Ink, Stroke};

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

    fn two_layer_sprite(cels: Vec<CelSource>) -> Sprite {
        let layers = [1, 2]
            .map(|id| Layer {
                id: LayerId(id),
                name: format!("Layer {id}"),
            })
            .to_vec();
        let frames = vec![Frame { duration_ms: 100 }];
        Sprite::from_parts(8, 8, Palette::default(), layers, frames, cels).unwrap()
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
}
