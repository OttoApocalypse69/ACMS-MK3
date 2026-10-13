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
#[inline]
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
#[inline]
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

/// How many pixels, counted once per layer that covers them, are worth spreading over several
/// threads. Below this, starting the threads costs more than it saves: the area redrawn while
/// a stroke is drawn is far smaller.
const PARALLEL_FROM: usize = 200_000;

/// Most threads one flattening is spread over.
const MAX_THREADS: usize = 8;

/// One layer's part in flattening a rectangle.
struct Pass<'a> {
    cel: Option<&'a Cel>,
    overlay: Option<Overlay<'a>>,
    opacity: u8,
    blend: BlendMode,
    /// The part of the rectangle this layer has anything in.
    covered: Rect,
}

impl Pass<'_> {
    /// Paints a run of this layer's pixels onto the pixels below them.
    fn paint_run(&self, from: &[Rgba8], onto: &mut [Rgba8]) {
        let covers = self.blend == BlendMode::Normal && self.opacity == 255;
        for (below, &color) in onto.iter_mut().zip(from) {
            let color = if self.opacity == 255 {
                color
            } else {
                with_opacity(color, self.opacity)
            };
            if color.a == 0 {
                continue;
            }
            *below = if below.a == 0 || (covers && color.a == 255) {
                color
            } else {
                blend(self.blend, color, *below)
            };
        }
    }

    /// Paints this layer onto the rows `first_row..` of the output that `rows` holds; `rect`
    /// is the rectangle the whole output stands for.
    fn paint(&self, rows: &mut [Rgba8], first_row: i32, rect: Rect) {
        let width = rect.w as usize;
        let row_count = (rows.len() / width) as i32;
        let top = self.covered.y.max(first_row);
        let bottom = self.covered.bottom().min(first_row + row_count);
        let cel_rect = self.cel.map_or(Rect::EMPTY, Cel::rect);

        for y in top..bottom {
            let out_row = &mut rows[(y - first_row) as usize * width..][..width];
            // The cel's pixels on this row, if it has any here.
            let cel_row = self
                .cel
                .filter(|_| y >= cel_rect.y && y < cel_rect.bottom())
                .map(|cel| {
                    let cel_width = cel_rect.w as usize;
                    &cel.image().pixels()[(y - cel_rect.y) as usize * cel_width..][..cel_width]
                });
            // Without a stroke in the way, the two rows are walked side by side.
            if let (None, Some(row)) = (self.overlay, cel_row) {
                let left = self.covered.x.max(cel_rect.x);
                let right = self.covered.right().min(cel_rect.right());
                if left < right {
                    let count = (right - left) as usize;
                    let from = &row[(left - cel_rect.x) as usize..][..count];
                    let onto = &mut out_row[(left - rect.x) as usize..][..count];
                    self.paint_run(from, onto);
                }
                continue;
            }
            for x in self.covered.x..self.covered.right() {
                let stroke = self.overlay.and_then(|overlay| overlay.buffer.get(x, y));
                let color = match (stroke, cel_row) {
                    (Some(color), _) => color,
                    (None, Some(row)) if x >= cel_rect.x && x < cel_rect.right() => {
                        row[(x - cel_rect.x) as usize]
                    }
                    _ => continue,
                };
                let color = if self.opacity == 255 {
                    color
                } else {
                    with_opacity(color, self.opacity)
                };
                if color.a == 0 {
                    continue;
                }
                let below = &mut out_row[(x - rect.x) as usize];
                // On nothing, and when it hides what is below, the colour stands as it is;
                // `blend` would work out the same.
                let stands = below.a == 0 || (color.a == 255 && self.blend == BlendMode::Normal);
                *below = if stands {
                    color
                } else {
                    blend(self.blend, color, *below)
                };
            }
        }
    }
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
    let available = std::thread::available_parallelism().map_or(1, |count| count.get());
    compose_rect_on(sprite, frame, overlay, rect, available.min(MAX_THREADS))
}

/// [`compose_rect`] on at most `max_threads` threads. The picture is the same for any number.
fn compose_rect_on(
    sprite: &Sprite,
    frame: usize,
    overlay: Option<Overlay<'_>>,
    rect: Rect,
    max_threads: usize,
) -> Image {
    let visible = rect.intersect(sprite.bounds());
    let mut passes = Vec::new();
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
        let cel_rect = cel.map_or(Rect::EMPTY, Cel::rect);
        let overlay_rect = overlay.map_or(Rect::EMPTY, |o| o.buffer.bounds());
        let covered = cel_rect.union(overlay_rect).intersect(visible);
        if !covered.is_empty() {
            passes.push(Pass {
                cel,
                overlay,
                opacity: image.opacity,
                blend: image.blend,
                covered,
            });
        }
    }

    let width = rect.w as usize;
    let mut out = vec![Rgba8::TRANSPARENT; width * rect.h as usize];
    let work: usize = passes.iter().map(|pass| pass.covered.area()).sum();
    let threads = if work < PARALLEL_FROM {
        1
    } else {
        max_threads.min(rect.h as usize)
    };

    if threads <= 1 {
        for pass in &passes {
            pass.paint(&mut out, rect.y, rect);
        }
    } else {
        // Bands of rows are independent of each other: each thread paints every layer onto
        // its own band, bottom to top.
        let band_rows = (rect.h as usize).div_ceil(threads);
        let passes = &passes;
        std::thread::scope(|scope| {
            for (index, band) in out.chunks_mut(band_rows * width).enumerate() {
                let first_row = rect.y + (index * band_rows) as i32;
                scope.spawn(move || {
                    for pass in passes {
                        pass.paint(band, first_row, rect);
                    }
                });
            }
        });
    }
    // The buffer was made to exactly this size.
    Image::from_pixels(rect.w, rect.h, out).unwrap_or_else(|| Image::new(rect.w, rect.h))
}

/// Flattens a whole frame.
pub fn compose(sprite: &Sprite, frame: usize) -> Image {
    compose_rect(sprite, frame, None, sprite.bounds())
}

/// How the frames around the current one are shown faintly behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OnionSkin {
    /// How many earlier frames are shown.
    pub before: u8,
    /// How many later frames are shown.
    pub after: u8,
    /// How strong the nearest frame is, 0 to 255; frames further away are fainter.
    pub opacity: u8,
    /// Earlier frames are coloured red and later ones blue, to tell them apart.
    pub tint: bool,
}

impl Default for OnionSkin {
    fn default() -> Self {
        Self {
            before: 1,
            after: 1,
            opacity: 96,
            tint: true,
        }
    }
}

/// Most frames an onion skin shows on either side.
pub const MAX_ONION_FRAMES: u8 = 8;

const EARLIER_TINT: Rgba8 = Rgba8::rgb(235, 60, 70);
const LATER_TINT: Rgba8 = Rgba8::rgb(60, 120, 240);

/// The onion skin for one frame: the frames before and after it, flattened, tinted and faded,
/// nearer ones on top. Canvas-sized. Frames that do not exist are left out; the animation is
/// not wrapped around.
pub fn onion_skin(sprite: &Sprite, frame: usize, onion: &OnionSkin) -> Image {
    let bounds = sprite.bounds();
    let mut out = Image::new(bounds.w, bounds.h);
    let count = sprite.frames().len();
    let reach = onion.before.max(onion.after).min(MAX_ONION_FRAMES);

    for distance in (1..=reach).rev() {
        let earlier = (distance <= onion.before)
            .then(|| frame.checked_sub(usize::from(distance)))
            .flatten();
        let later = (distance <= onion.after)
            .then(|| frame + usize::from(distance))
            .filter(|later| *later < count);
        // Fading evenly: with three frames shown, they get 3/3, 2/3 and 1/3 of the opacity.
        let strength = |shown: u8| {
            let step = u32::from(shown - distance + 1);
            (u32::from(onion.opacity) * step / u32::from(shown.max(1))) as u8
        };
        let sides = [
            (earlier, EARLIER_TINT, onion.before),
            (later, LATER_TINT, onion.after),
        ];
        for (shown_frame, tint, shown) in sides {
            let Some(shown_frame) = shown_frame else {
                continue;
            };
            let flat = compose(sprite, shown_frame);
            let opacity = strength(shown.min(MAX_ONION_FRAMES));
            for y in 0..bounds.h as i32 {
                for x in 0..bounds.w as i32 {
                    let pixel = flat.get(x, y);
                    if pixel.a == 0 {
                        continue;
                    }
                    let color = if onion.tint {
                        // Half its own colour, half the tint: the drawing stays readable.
                        let mix = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 2) as u8;
                        Rgba8::new(
                            mix(pixel.r, tint.r),
                            mix(pixel.g, tint.g),
                            mix(pixel.b, tint.b),
                            pixel.a,
                        )
                    } else {
                        pixel
                    };
                    out.set(x, y, with_opacity(color, opacity).over(out.get(x, y)));
                }
            }
        }
    }
    out
}

/// Most memory the pictures of frames other than the one shown may take.
const KEPT_FRAME_BYTES: usize = 128 * 1024 * 1024;

/// The flattened picture of one frame, kept up to date by recomposing only what changed.
///
/// The pictures of frames shown earlier are kept too, for as long as the sprite stays as it
/// is, so that playing an animation flattens every frame once and not once per loop.
#[derive(Default)]
pub struct FrameCache {
    image: Image,
    frame: usize,
    valid: bool,
    /// Whether `image` is the frame as it is, without a stroke in progress drawn into it.
    clean: bool,
    /// The state of the sprite the kept pictures are of; see [`FrameCache::note_revision`].
    revision: Option<u64>,
    kept: std::collections::HashMap<usize, Image>,
}

impl FrameCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the picture, e.g. because a different sprite is shown. The next
    /// [`FrameCache::refresh`] recomposes all of it.
    pub fn invalidate(&mut self) {
        self.valid = false;
        self.kept.clear();
        self.revision = None;
    }

    /// Says which state of the sprite the next [`FrameCache::refresh`] will be given: a number
    /// that changes whenever the sprite does, like [`crate::document::Document::revision`].
    /// The pictures of other frames are only kept while it stays the same. Without this call
    /// none are kept.
    pub fn note_revision(&mut self, revision: u64) {
        if self.revision != Some(revision) {
            self.kept.clear();
            self.revision = Some(revision);
        }
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
        if self.image.bounds() != bounds {
            self.kept.clear();
        }
        if !self.valid || self.frame != frame || self.image.bounds() != bounds {
            let kept = (overlay.is_none())
                .then(|| self.kept.remove(&frame))
                .flatten();
            let next = match kept {
                Some(image) => image,
                None => compose_rect(sprite, frame, overlay, bounds),
            };
            let previous = std::mem::replace(&mut self.image, next);
            // The frame that was shown is kept for when it comes round again.
            let room = (self.kept.len() + 1) * previous.byte_size() <= KEPT_FRAME_BYTES;
            if self.valid && self.clean && self.revision.is_some() && previous.bounds() == bounds {
                if !room {
                    self.kept.clear();
                }
                self.kept.insert(self.frame, previous);
            }
            self.frame = frame;
            self.valid = true;
            self.clean = overlay.is_none();
            return bounds;
        }
        self.clean = overlay.is_none();

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

    #[test]
    fn the_picture_is_the_same_on_one_thread_and_on_many() {
        // Large enough for the threads to be used, with every kind of layer in it: opaque,
        // translucent, blended, faded, and cels that cover only part of the canvas.
        let size = 300;
        let mut doc = Document::new(Sprite::new(size, size).unwrap());
        let noise = |seed: u32, w: u32, h: u32, alpha: Option<u8>| {
            let mut state = seed;
            let pixels = (0..w * h)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let [r, g, b, a] = state.to_be_bytes();
                    Rgba8::new(r, g, b, alpha.unwrap_or(a)).normalized()
                })
                .collect();
            Image::from_pixels(w, h, pixels).unwrap()
        };
        let modes = [
            BlendMode::Normal,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Overlay,
            BlendMode::Add,
            BlendMode::Normal,
        ];
        for (index, mode) in modes.into_iter().enumerate() {
            let index = index as u32;
            let layer = match index {
                0 => LayerId(1),
                _ => doc
                    .transact("Add", |tx| {
                        tx.add_layer(Slot::new(None, index as usize), "Layer", false)
                    })
                    .unwrap(),
            };
            let inset = (index * 17) as i32;
            let side = size - 2 * index * 17;
            let alpha = index.is_multiple_of(2).then_some(255);
            doc.transact("Paint", |tx| {
                tx.set_pixels(
                    layer,
                    0,
                    Rect::new(inset, inset, side, side),
                    &noise(index + 1, side, side, alpha),
                )?;
                tx.update_layer(layer, |props| {
                    if let Some(image) = &mut props.image {
                        image.blend = mode;
                        image.opacity = 255 - (index * 30) as u8;
                    }
                })
            })
            .unwrap();
        }
        let sprite = doc.sprite();
        for rect in [
            sprite.bounds(),
            Rect::new(-20, 10, 290, 301),
            Rect::new(7, 7, 1, 280),
        ] {
            let one = compose_rect_on(sprite, 0, None, rect, 1);
            for threads in [2, 3, 8, 64, 1000] {
                let many = compose_rect_on(sprite, 0, None, rect, threads);
                assert!(one == many, "{threads} threads differ on {rect:?}");
            }
            assert!(one == compose_rect(sprite, 0, None, rect));
        }
        // And both are what painting pixel by pixel, the plain way, gives.
        let whole = compose(sprite, 0);
        let mut plain = Image::new(size, size);
        for entry in sprite.walk() {
            let image = entry.layer.as_image().unwrap();
            for y in 0..size as i32 {
                for x in 0..size as i32 {
                    let color = with_opacity(sprite.pixel(entry.layer.id, 0, x, y), image.opacity);
                    if color.a != 0 {
                        plain.set(x, y, blend(image.blend, color, plain.get(x, y)));
                    }
                }
            }
        }
        assert!(whole == plain);
    }

    #[test]
    #[ignore = "a measurement; run it in release"]
    fn time_to_flatten_a_large_sprite() {
        let mut doc = Document::new(Sprite::new(512, 512).unwrap());
        for index in 0..20_u32 {
            let layer = match index {
                0 => LayerId(1),
                _ => doc
                    .transact("Add", |tx| {
                        tx.add_layer(Slot::new(None, index as usize), "Layer", false)
                    })
                    .unwrap(),
            };
            let alpha = if index == 0 { 255 } else { 40 };
            let color = Rgba8::new((index * 12) as u8, 200, 128, alpha);
            doc.transact("Paint", |tx| {
                tx.set_pixels(
                    layer,
                    0,
                    Rect::from_size(512, 512),
                    &filled(512, 512, color),
                )
            })
            .unwrap();
        }
        let sprite = doc.sprite();
        eprintln!("cores: {:?}", std::thread::available_parallelism());
        for threads in [1, 2, 4, 8, 16] {
            let started = std::time::Instant::now();
            for _ in 0..10 {
                std::hint::black_box(compose_rect_on(sprite, 0, None, sprite.bounds(), threads));
            }
            eprintln!("{threads} threads: {:?}", started.elapsed() / 10);
        }
    }

    #[test]
    fn frames_shown_before_are_kept_until_the_sprite_changes() {
        let mut document = Document::new(filmstrip());
        let mut cache = FrameCache::new();
        let all = Rect::from_size(8, 1);
        let show = |document: &Document, cache: &mut FrameCache, frame: usize| {
            cache.note_revision(document.revision());
            let redrawn = cache.refresh(document.sprite(), frame, None, all);
            assert_eq!(redrawn, all);
            assert_eq!(*cache.image(), compose(document.sprite(), frame));
        };
        for frame in [0, 1, 2, 3, 4, 0, 1, 2] {
            show(&document, &mut cache, frame);
        }
        // Every frame but the one shown is kept.
        assert_eq!(cache.kept.len(), 4);

        // An edit to a frame that is not shown: nothing stale may be shown afterwards.
        let pixel = Image::from_pixels(1, 1, vec![Rgba8::WHITE]).unwrap();
        document
            .transact("Paint", |tx| {
                tx.set_pixels(LayerId(1), 3, Rect::new(7, 0, 1, 1), &pixel)
            })
            .unwrap();
        for frame in [3, 4, 2, 3] {
            show(&document, &mut cache, frame);
        }
        // Undo is a change too.
        document.undo();
        show(&document, &mut cache, 3);
        show(&document, &mut cache, 2);

        // Without being told the state of the sprite, the cache keeps nothing.
        let mut untold = FrameCache::new();
        untold.refresh(document.sprite(), 0, None, all);
        untold.refresh(document.sprite(), 1, None, all);
        assert!(untold.kept.is_empty());
        // A picture with a stroke in progress drawn into it is not kept either.
        cache.invalidate();
        assert!(cache.kept.is_empty());
    }

    /// Five frames, each with one red pixel at x = its frame number.
    fn filmstrip() -> Sprite {
        let mut document = Document::new(Sprite::new(8, 1).unwrap());
        document
            .transact("Frames", |tx| {
                for frame in 0..5 {
                    if frame > 0 {
                        tx.add_frame(frame, None)?;
                    }
                    let at = Rect::new(frame as i32, 0, 1, 1);
                    tx.set_pixels(LayerId(1), frame, at, &filled(1, 1, RED))?;
                }
                Ok::<_, crate::sprite::SpriteError>(())
            })
            .unwrap();
        document.sprite().clone()
    }

    #[test]
    fn each_frame_is_composed_from_its_own_cels() {
        let sprite = filmstrip();
        for frame in 0..5 {
            let flat = compose(&sprite, frame);
            for x in 0..8 {
                let expected = if x == frame as i32 {
                    RED
                } else {
                    Rgba8::TRANSPARENT
                };
                assert_eq!(flat.get(x, 0), expected, "frame {frame}, x {x}");
            }
        }
    }

    #[test]
    fn the_onion_skin_shows_neighbouring_frames_fainter_with_distance() {
        let sprite = filmstrip();
        let onion = OnionSkin {
            before: 2,
            after: 1,
            opacity: 200,
            tint: false,
        };
        let skin = onion_skin(&sprite, 2, &onion);
        // Two back at half strength, one back and one ahead at full; not the frame itself.
        assert_eq!(skin.get(0, 0), Rgba8::new(255, 0, 0, 100));
        assert_eq!(skin.get(1, 0), Rgba8::new(255, 0, 0, 200));
        assert_eq!(skin.get(2, 0), Rgba8::TRANSPARENT);
        assert_eq!(skin.get(3, 0), Rgba8::new(255, 0, 0, 200));
        assert_eq!(skin.get(4, 0), Rgba8::TRANSPARENT);

        // At the ends there is nothing before the first frame or after the last.
        let first = onion_skin(&sprite, 0, &onion);
        assert_eq!(first.get(1, 0).a, 200);
        assert_eq!(first.content_bounds(), Rect::new(1, 0, 1, 1));
        let last = onion_skin(&sprite, 4, &onion);
        assert_eq!(last.content_bounds(), Rect::new(2, 0, 2, 1));

        // Tinted, earlier frames lean red and later ones blue.
        let tinted = onion_skin(
            &sprite,
            2,
            &OnionSkin {
                tint: true,
                ..onion
            },
        );
        assert!(tinted.get(1, 0).r > tinted.get(1, 0).b);
        assert!(tinted.get(3, 0).b > 0 && tinted.get(3, 0).r < 255);

        let off = OnionSkin {
            before: 0,
            after: 0,
            ..onion
        };
        assert!(onion_skin(&sprite, 2, &off).content_bounds().is_empty());
    }
}
