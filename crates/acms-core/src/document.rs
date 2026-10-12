//! A sprite together with its undo history (see `PLAN.md`, section 4.3).
//!
//! The only way to change the sprite is [`Document::transact`]. Everything done inside one
//! call becomes a single undo step.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::compositor;
use crate::geom::{Point, Rect};
use crate::image::Image;
use crate::selection::Mask;
use crate::sprite::{
    CanvasState, Cel, CelSource, DEFAULT_FRAME_MS, Frame, FrameState, Layer, LayerId, LayerKind,
    LayerProps, MAX_FRAME_MS, MAX_SPRITE_SIZE, Palette, PlacedCel, Slot, Sprite, SpriteError,
    Symmetry, Tag,
};
use crate::tiles::{MAX_TILE_SIZE, Slice, Tileset, TilesetId};

/// Default cap on the memory the undo history may hold.
pub const DEFAULT_HISTORY_BYTES: usize = 256 * 1024 * 1024;

/// A layer (with everything inside it) and its cels, at a place in the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Placement {
    slot: Slot,
    layer: Layer,
    cels: Vec<PlacedCel>,
}

impl Placement {
    fn insert(&self, sprite: &mut Sprite) {
        sprite.insert_layer(self.slot, self.layer.clone(), self.cels.clone());
    }

    fn remove(&self, sprite: &mut Sprite) {
        sprite.remove_layer(self.layer.id);
    }

    fn bounds(&self) -> Rect {
        self.cels
            .iter()
            .fold(Rect::EMPTY, |bounds, (_, _, cel)| bounds.union(cel.rect()))
    }
}

/// One reversible change.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Op {
    /// The pixels of `rect` on one cel went from `before` to `after`.
    Pixels {
        layer: LayerId,
        frame: usize,
        rect: Rect,
        before: Image,
        after: Image,
    },
    /// A layer was added.
    Insert(Box<Placement>),
    /// A layer was taken away.
    Remove(Box<Placement>),
    /// A layer went from one place in the tree to another. Both indices are the layer's
    /// position with the layer itself not counted among its siblings.
    Move { id: LayerId, from: Slot, to: Slot },
    /// A layer's name, visibility or image properties changed.
    Props {
        id: LayerId,
        before: LayerProps,
        after: LayerProps,
    },
    /// The selection changed.
    Selection {
        before: Option<Arc<Mask>>,
        after: Option<Arc<Mask>>,
    },
    /// The canvas changed size, and every cel with it.
    Canvas {
        before: Box<CanvasState>,
        after: Box<CanvasState>,
    },
    /// The palette's colours or ramps changed.
    Palette {
        before: Box<Palette>,
        after: Box<Palette>,
    },
    /// Frames, the cels in them, links between cels, tags, tilesets or tiles changed.
    Frames {
        before: Box<FrameState>,
        after: Box<FrameState>,
    },
    Slices {
        before: Vec<Slice>,
        after: Vec<Slice>,
    },
}

impl Op {
    fn apply(&self, sprite: &mut Sprite) {
        match self {
            Self::Pixels {
                layer,
                frame,
                rect,
                after,
                ..
            } => sprite.write_region(*layer, *frame, *rect, after),
            Self::Insert(placement) => placement.insert(sprite),
            Self::Remove(placement) => placement.remove(sprite),
            Self::Move { id, to, .. } => sprite.move_layer(*id, *to),
            Self::Props { id, after, .. } => sprite.set_layer_props(*id, after),
            Self::Selection { after, .. } => sprite.set_selection(after.clone()),
            Self::Canvas { after, .. } => sprite.set_canvas_state((**after).clone()),
            Self::Palette { after, .. } => sprite.set_palette((**after).clone()),
            Self::Frames { after, .. } => sprite.set_frame_state((**after).clone()),
            Self::Slices { after, .. } => sprite.set_slices(after.clone()),
        }
    }

    fn revert(&self, sprite: &mut Sprite) {
        match self {
            Self::Pixels {
                layer,
                frame,
                rect,
                before,
                ..
            } => sprite.write_region(*layer, *frame, *rect, before),
            Self::Insert(placement) => placement.remove(sprite),
            Self::Remove(placement) => placement.insert(sprite),
            Self::Move { id, from, .. } => sprite.move_layer(*id, *from),
            Self::Props { id, before, .. } => sprite.set_layer_props(*id, before),
            Self::Selection { before, .. } => sprite.set_selection(before.clone()),
            Self::Canvas { before, .. } => sprite.set_canvas_state((**before).clone()),
            Self::Palette { before, .. } => sprite.set_palette((**before).clone()),
            Self::Frames { before, .. } => sprite.set_frame_state((**before).clone()),
            Self::Slices { before, .. } => sprite.set_slices(before.clone()),
        }
    }

    /// The canvas area that looks different after this op. `sprite` may be in the state
    /// before or after it; the layers it refers to cover the same pixels either way.
    fn damage(&self, sprite: &Sprite) -> Rect {
        match self {
            Self::Pixels { rect, .. } => *rect,
            Self::Insert(placement) | Self::Remove(placement) => placement.bounds(),
            Self::Move { id, .. } | Self::Props { id, .. } => sprite.layer_bounds(*id),
            // The outline is drawn by the view, not composed into the picture.
            Self::Selection { .. } | Self::Palette { .. } | Self::Slices { .. } => Rect::EMPTY,
            Self::Canvas { .. } | Self::Frames { .. } => {
                Rect::from_size(MAX_SPRITE_SIZE, MAX_SPRITE_SIZE)
            }
        }
    }

    fn byte_size(&self) -> usize {
        match self {
            Self::Pixels { before, after, .. } => before.byte_size() + after.byte_size(),
            Self::Insert(placement) | Self::Remove(placement) => placement
                .cels
                .iter()
                .map(|(_, _, cel)| cel.image().byte_size())
                .sum(),
            Self::Move { .. } | Self::Props { .. } | Self::Slices { .. } => 0,
            // The pictures are shared with the sprite; only the bookkeeping is extra.
            Self::Frames { before, after } => {
                (before.cels.len() + after.cels.len()) * std::mem::size_of::<Cel>()
            }
            Self::Palette { before, after } => {
                (before.len() + after.len()) * std::mem::size_of::<crate::color::Rgba8>()
            }
            Self::Selection { before, after } => [before, after]
                .into_iter()
                .flatten()
                .map(|mask| mask.width() as usize * mask.height() as usize)
                .sum(),
            // Cels the resize did not have to rebuild are shared, but counting them all is
            // the safe side to err on.
            Self::Canvas { before, after } => [before, after]
                .into_iter()
                .flat_map(|state| state.cels.values())
                .map(|cel| cel.image().byte_size())
                .sum(),
        }
    }
}

/// Collects the changes made inside [`Document::transact`].
pub struct Transaction<'a> {
    sprite: &'a mut Sprite,
    next_layer_id: &'a mut u32,
    ops: Vec<Op>,
}

impl Transaction<'_> {
    /// The sprite as it is right now, including the changes made so far in this transaction.
    pub fn sprite(&self) -> &Sprite {
        self.sprite
    }

    fn push(&mut self, op: Op) {
        op.apply(self.sprite);
        self.ops.push(op);
    }

    fn new_layer_id(&mut self) -> LayerId {
        let id = LayerId(*self.next_layer_id);
        *self.next_layer_id += 1;
        id
    }

    fn known(&self, id: LayerId) -> Result<&Layer, SpriteError> {
        self.sprite.layer(id).ok_or(SpriteError::UnknownLayer(id.0))
    }

    /// Checks that a layer could be placed at `slot`. `already_there` says the layer is one of
    /// that slot's siblings now and will be taken out first, which leaves one place fewer.
    fn check_slot(&self, slot: Slot, already_there: bool) -> Result<(), SpriteError> {
        let siblings = self.sprite.siblings(slot.parent)?.len();
        let room = siblings - usize::from(already_there);
        if slot.index > room {
            return Err(SpriteError::InvalidSlot { index: slot.index });
        }
        Ok(())
    }

    /// Replaces the pixels of the canvas rectangle `rect` on one cel with `pixels`, which must
    /// be exactly `rect`-sized. Parts outside the canvas are ignored. A locked layer refuses.
    ///
    /// Only the area that really changes is written and remembered for undo.
    pub fn set_pixels(
        &mut self,
        layer: LayerId,
        frame: usize,
        rect: Rect,
        pixels: &Image,
    ) -> Result<(), SpriteError> {
        if self.sprite.check_cel(layer, frame)?.locked {
            return Err(SpriteError::LayerLocked(layer.0));
        }
        if (rect.w, rect.h) != (pixels.width(), pixels.height()) {
            return Err(SpriteError::SizeMismatch {
                want_w: rect.w,
                want_h: rect.h,
                got_w: pixels.width(),
                got_h: pixels.height(),
            });
        }
        // A tilemap has no pixels of its own: the write goes to its tiles.
        if self.sprite.tileset_of(layer).is_some() {
            let context = self.sprite.tile_context();
            self.edit_frames(|state| state.edit_tile_pixels(&context, layer, frame, rect, pixels));
            return Ok(());
        }

        let target = rect.intersect(self.sprite.bounds());
        let before = self.sprite.read_region(layer, frame, target);
        let mut after = Image::new(target.w, target.h);
        let mut changed = Rect::EMPTY;
        for y in 0..target.h as i32 {
            for x in 0..target.w as i32 {
                let new = pixels
                    .get(x + target.x - rect.x, y + target.y - rect.y)
                    .normalized();
                after.set(x, y, new);
                if new != before.get(x, y) {
                    changed = changed.union(Rect::new(x, y, 1, 1));
                }
            }
        }
        if changed.is_empty() {
            return Ok(());
        }

        self.push(Op::Pixels {
            layer,
            frame,
            rect: changed.translated(target.x, target.y),
            before: before.crop(changed),
            after: after.crop(changed),
        });
        Ok(())
    }

    /// Replaces the selection. `None`, or a mask with nothing in it, selects nothing, which
    /// leaves every pixel editable.
    pub fn set_selection(&mut self, selection: Option<Mask>) -> Result<(), SpriteError> {
        let bounds = self.sprite.bounds();
        if let Some(mask) = &selection
            && (mask.width(), mask.height()) != (bounds.w, bounds.h)
        {
            return Err(SpriteError::SizeMismatch {
                want_w: bounds.w,
                want_h: bounds.h,
                got_w: mask.width(),
                got_h: mask.height(),
            });
        }
        let before = self.sprite.selection().cloned();
        let after = selection.filter(|mask| !mask.is_empty()).map(Arc::new);
        if before.as_deref() != after.as_deref() {
            self.push(Op::Selection { before, after });
        }
        Ok(())
    }

    /// Adds an empty tileset. Returns its id.
    pub fn add_tileset(&mut self, name: &str, tile_width: u32, tile_height: u32) -> TilesetId {
        let highest = self.sprite.tilesets().iter().map(|set| set.id.0).max();
        let id = TilesetId(highest.map_or(1, |id| id + 1));
        let size = |side: u32| side.clamp(1, MAX_TILE_SIZE);
        let tileset = Tileset::new(id, name, size(tile_width), size(tile_height));
        self.edit_frames(|state| state.tilesets.push(tileset));
        id
    }

    /// Adds an empty tilemap layer that draws from `tileset`. Returns its id.
    pub fn add_tilemap_layer(
        &mut self,
        slot: Slot,
        name: &str,
        tileset: TilesetId,
    ) -> Result<LayerId, SpriteError> {
        self.check_slot(slot, false)?;
        if self.sprite.tileset(tileset).is_none() {
            return Err(SpriteError::UnknownTileset(tileset.0));
        }
        let id = self.new_layer_id();
        self.push(Op::Insert(Box::new(Placement {
            slot,
            layer: Layer::tilemap(id, name, tileset),
            cels: Vec::new(),
        })));
        Ok(id)
    }

    /// Puts tiles into cells of a tilemap cel: `(column, row, tile number)` each, 0 for none.
    /// Cells outside the map and tile numbers the set does not have are ignored or emptied.
    pub fn set_tiles(
        &mut self,
        layer: LayerId,
        frame: usize,
        cells: &[(u32, u32, u32)],
    ) -> Result<(), SpriteError> {
        if self.sprite.check_cel(layer, frame)?.locked {
            return Err(SpriteError::LayerLocked(layer.0));
        }
        if self.sprite.tileset_of(layer).is_none() {
            return Err(SpriteError::NotATilemap(layer.0));
        }
        let context = self.sprite.tile_context();
        self.edit_frames(|state| state.set_tiles(&context, layer, frame, cells));
        Ok(())
    }

    fn check_slice(slice: &Slice) -> Result<(), SpriteError> {
        if slice.name.trim().is_empty() || slice.rect.is_empty() {
            return Err(SpriteError::InvalidSlice);
        }
        Ok(())
    }

    fn edit_slices(&mut self, edit: impl FnOnce(&mut Vec<Slice>)) {
        let before = self.sprite.slices().to_vec();
        let mut after = before.clone();
        edit(&mut after);
        if after != before {
            self.push(Op::Slices { before, after });
        }
    }

    /// Adds a slice. Returns its position among the slices.
    pub fn add_slice(&mut self, slice: Slice) -> Result<usize, SpriteError> {
        Self::check_slice(&slice)?;
        self.edit_slices(|slices| slices.push(slice));
        Ok(self.sprite.slices().len() - 1)
    }

    /// Replaces the slice at `index`.
    pub fn set_slice(&mut self, index: usize, slice: Slice) -> Result<(), SpriteError> {
        if index >= self.sprite.slices().len() {
            return Err(SpriteError::UnknownSlice(index));
        }
        Self::check_slice(&slice)?;
        self.edit_slices(|slices| slices[index] = slice);
        Ok(())
    }

    pub fn remove_slice(&mut self, index: usize) -> Result<(), SpriteError> {
        if index >= self.sprite.slices().len() {
            return Err(SpriteError::UnknownSlice(index));
        }
        self.edit_slices(|slices| {
            slices.remove(index);
        });
        Ok(())
    }

    /// Tilemaps are laid out on the canvas as it is; resizing would have to re-cut them.
    fn check_no_tilemaps(&self) -> Result<(), SpriteError> {
        if self.sprite.tile_context().layers.is_empty() {
            Ok(())
        } else {
            Err(SpriteError::HasTilemaps)
        }
    }

    /// Changes what is laid out along the time axis, as one reversible step.
    fn edit_frames(&mut self, edit: impl FnOnce(&mut FrameState)) {
        let before = self.sprite.frame_state();
        let mut after = before.clone();
        edit(&mut after);
        if after != before {
            self.push(Op::Frames {
                before: Box::new(before),
                after: Box::new(after),
            });
        }
    }

    fn check_frame(&self, frame: usize) -> Result<(), SpriteError> {
        if frame < self.sprite.frames().len() {
            Ok(())
        } else {
            Err(SpriteError::UnknownFrame(frame))
        }
    }

    /// Adds a frame so that it ends up at position `at`. With `copy_of`, the new frame gets
    /// that frame's duration and a copy of its cels; otherwise it is empty.
    pub fn add_frame(&mut self, at: usize, copy_of: Option<usize>) -> Result<(), SpriteError> {
        if at > self.sprite.frames().len() {
            return Err(SpriteError::UnknownFrame(at));
        }
        if let Some(source) = copy_of {
            self.check_frame(source)?;
        }
        self.edit_frames(|state| {
            let copied: Vec<(LayerId, Cel)> = state
                .cels
                .iter()
                .filter(|((_, frame), _)| Some(*frame) == copy_of)
                .map(|((layer, _), cel)| (*layer, cel.clone()))
                .collect();
            let duration_ms = copy_of.map_or(DEFAULT_FRAME_MS, |f| state.frames[f].duration_ms);

            state.remap(|frame| Some(if frame >= at { frame + 1 } else { frame }));
            state.frames.insert(at, Frame { duration_ms });
            for (layer, cel) in copied {
                state.cels.insert((layer, at), cel);
            }
            // A frame put inside a tag makes the tag longer; one put before it moves it.
            for tag in &mut state.tags {
                if at <= tag.from {
                    tag.from += 1;
                    tag.to += 1;
                } else if at <= tag.to {
                    tag.to += 1;
                }
            }
        });
        Ok(())
    }

    /// Deletes a frame and its cels. The sprite keeps at least one frame.
    pub fn remove_frame(&mut self, frame: usize) -> Result<(), SpriteError> {
        self.check_frame(frame)?;
        if self.sprite.frames().len() == 1 {
            return Err(SpriteError::LastFrame);
        }
        self.edit_frames(|state| {
            state.remap(|old| match old.cmp(&frame) {
                std::cmp::Ordering::Less => Some(old),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(old - 1),
            });
            state.frames.remove(frame);
            // A tag loses the frame; a tag that was only this frame goes with it.
            state
                .tags
                .retain(|tag| !(tag.from == frame && tag.to == frame));
            for tag in &mut state.tags {
                if frame < tag.from {
                    tag.from -= 1;
                }
                if frame <= tag.to {
                    tag.to -= 1;
                }
            }
        });
        Ok(())
    }

    /// Moves a frame so that it ends up at position `to`. Tags stay where they are on the
    /// time axis.
    pub fn move_frame(&mut self, from: usize, to: usize) -> Result<(), SpriteError> {
        self.check_frame(from)?;
        self.check_frame(to)?;
        self.edit_frames(|state| {
            let moved = state.frames.remove(from);
            state.frames.insert(to, moved);
            state.remap(|old| {
                Some(if old == from {
                    to
                } else if from < old && old <= to {
                    old - 1
                } else if to <= old && old < from {
                    old + 1
                } else {
                    old
                })
            });
        });
        Ok(())
    }

    /// Sets how long a frame is shown, clamped to `1..=MAX_FRAME_MS` milliseconds.
    pub fn set_frame_duration(&mut self, frame: usize, ms: u32) -> Result<(), SpriteError> {
        self.check_frame(frame)?;
        self.edit_frames(|state| state.frames[frame].duration_ms = ms.clamp(1, MAX_FRAME_MS));
        Ok(())
    }

    /// Puts what one cel holds into another cel, replacing what was there. With `copy` the
    /// source keeps its pixels; otherwise it is left empty. Both must be on image layers that
    /// are not locked.
    pub fn move_cel(
        &mut self,
        from: (LayerId, usize),
        to: (LayerId, usize),
        copy: bool,
    ) -> Result<(), SpriteError> {
        self.sprite.check_cel(from.0, from.1)?;
        self.sprite.check_cel(to.0, to.1)?;
        if self.sprite.tileset_of(from.0).is_some() || self.sprite.tileset_of(to.0).is_some() {
            return Err(SpriteError::TilemapCel);
        }
        if from == to {
            return Ok(());
        }
        let canvas = self.sprite.bounds();
        let pixels = self.sprite.read_region(from.0, from.1, canvas);
        self.set_pixels(to.0, to.1, canvas, &pixels)?;
        if !copy {
            // Linked with the target, the source is the target and has just been written.
            let same = self.sprite.linked_frames(from.0, from.1).contains(&to.1) && from.0 == to.0;
            if !same {
                self.set_pixels(from.0, from.1, canvas, &Image::new(canvas.w, canvas.h))?;
            }
        }
        Ok(())
    }

    /// Makes the cels of one layer in `frames` one and the same cel, holding what the cel in
    /// `source` holds: from then on, drawing on any of them changes all of them.
    pub fn link_cels(
        &mut self,
        layer: LayerId,
        source: usize,
        frames: &[usize],
    ) -> Result<(), SpriteError> {
        if self.sprite.check_cel(layer, source)?.locked {
            return Err(SpriteError::LayerLocked(layer.0));
        }
        if self.sprite.tileset_of(layer).is_some() {
            return Err(SpriteError::TilemapCel);
        }
        for frame in frames {
            self.check_frame(*frame)?;
        }
        self.edit_frames(|state| {
            let link = match state.links.get(&(layer, source)) {
                Some(link) => *link,
                None => state.new_link(layer),
            };
            let cel = state.cels.get(&(layer, source)).cloned();
            for frame in frames.iter().copied().chain([source]) {
                state.links.insert((layer, frame), link);
                match &cel {
                    Some(cel) => state.cels.insert((layer, frame), cel.clone()),
                    None => state.cels.remove(&(layer, frame)),
                };
            }
            state.drop_single_links();
        });
        Ok(())
    }

    /// Makes a linked cel a cel of its own again. It keeps its pixels.
    pub fn unlink_cel(&mut self, layer: LayerId, frame: usize) -> Result<(), SpriteError> {
        self.sprite.check_cel(layer, frame)?;
        self.edit_frames(|state| {
            state.links.remove(&(layer, frame));
            state.drop_single_links();
        });
        Ok(())
    }

    fn check_tag(&self, tag: &Tag) -> Result<(), SpriteError> {
        if tag.from > tag.to || tag.to >= self.sprite.frames().len() {
            return Err(SpriteError::InvalidTag);
        }
        Ok(())
    }

    /// Adds a tag. Returns its position among the tags.
    pub fn add_tag(&mut self, tag: Tag) -> Result<usize, SpriteError> {
        self.check_tag(&tag)?;
        self.edit_frames(|state| state.tags.push(tag));
        Ok(self.sprite.tags().len() - 1)
    }

    /// Replaces the tag at `index`.
    pub fn set_tag(&mut self, index: usize, tag: Tag) -> Result<(), SpriteError> {
        if index >= self.sprite.tags().len() {
            return Err(SpriteError::UnknownTag(index));
        }
        self.check_tag(&tag)?;
        self.edit_frames(|state| state.tags[index] = tag);
        Ok(())
    }

    pub fn remove_tag(&mut self, index: usize) -> Result<(), SpriteError> {
        if index >= self.sprite.tags().len() {
            return Err(SpriteError::UnknownTag(index));
        }
        self.edit_frames(|state| {
            state.tags.remove(index);
        });
        Ok(())
    }

    /// Replaces the palette, colours and ramps.
    pub fn set_palette(&mut self, palette: Palette) {
        if *self.sprite.palette() != palette {
            let before = Box::new(self.sprite.palette().clone());
            self.push(Op::Palette {
                before,
                after: Box::new(palette),
            });
        }
    }

    /// Gives the sprite a new canvas built from `cels`, dropping the selection. `place` says
    /// where a reference picture goes that was shown over a given rectangle.
    fn rebuild_canvas(
        &mut self,
        width: u32,
        height: u32,
        cels: Vec<CelSource>,
        place: impl Fn(Rect) -> Rect,
    ) -> Result<(), SpriteError> {
        let valid = 1..=MAX_SPRITE_SIZE;
        if !valid.contains(&width) || !valid.contains(&height) {
            return Err(SpriteError::InvalidSize(width, height));
        }
        let before = Box::new(self.sprite.canvas_state());
        let mut resized = self.sprite.clone();
        resized.rebuild_canvas(width, height, cels, place);
        let after = Box::new(resized.canvas_state());
        if before != after {
            self.push(Op::Canvas { before, after });
        }
        Ok(())
    }

    /// Changes the canvas size without scaling: what is drawn moves by `(dx, dy)` and
    /// anything that ends up outside the new canvas is cut off.
    pub fn resize_canvas(
        &mut self,
        width: u32,
        height: u32,
        dx: i32,
        dy: i32,
    ) -> Result<(), SpriteError> {
        self.check_no_tilemaps()?;
        let mut cels = self.sprite.cel_sources();
        for cel in &mut cels {
            cel.origin = Point::new(cel.origin.x + dx, cel.origin.y + dy);
        }
        self.rebuild_canvas(width, height, cels, |rect| rect.translated(dx, dy))
    }

    /// Scales the whole sprite to a new size with nearest-neighbour sampling.
    pub fn scale_sprite(&mut self, width: u32, height: u32) -> Result<(), SpriteError> {
        self.check_no_tilemaps()?;
        let bounds = self.sprite.bounds();
        let cels = self
            .sprite
            .cel_sources()
            .into_iter()
            .map(|cel| {
                // Scale the cel as it sits on the canvas, so its position scales with it.
                let mut canvas = Image::new(bounds.w, bounds.h);
                canvas.blit(&cel.image, cel.origin);
                CelSource {
                    origin: Point::new(0, 0),
                    image: canvas.scaled_to(width.max(1), height.max(1)),
                    ..cel
                }
            })
            .collect();
        // A reference picture keeps covering the same part of the sprite.
        let scale = |at: i32, old: u32, new: u32| {
            (i64::from(at) * i64::from(new)).div_euclid(i64::from(old.max(1))) as i32
        };
        self.rebuild_canvas(width, height, cels, |rect| {
            Rect::from_edges(
                scale(rect.x, bounds.w, width),
                scale(rect.y, bounds.h, height),
                scale(rect.right(), bounds.w, width),
                scale(rect.bottom(), bounds.h, height),
            )
        })
    }

    /// Adds a reference layer showing `image`, as large as fits on the canvas. Returns its id.
    pub fn add_reference_layer(
        &mut self,
        slot: Slot,
        name: &str,
        image: Image,
    ) -> Result<LayerId, SpriteError> {
        self.check_slot(slot, false)?;
        if image.is_empty() {
            return Err(SpriteError::EmptyImage);
        }
        let rect = self.sprite.fitted(image.width(), image.height());
        let id = self.new_layer_id();
        self.push(Op::Insert(Box::new(Placement {
            slot,
            layer: Layer::reference(id, name, Arc::new(image), rect),
            cels: Vec::new(),
        })));
        Ok(id)
    }

    /// Adds an empty image layer, or an empty group, at `slot`. Returns its id.
    pub fn add_layer(
        &mut self,
        slot: Slot,
        name: &str,
        group: bool,
    ) -> Result<LayerId, SpriteError> {
        self.check_slot(slot, false)?;
        let id = self.new_layer_id();
        let layer = if group {
            Layer::group(id, name, Vec::new())
        } else {
            Layer::image(id, name)
        };
        self.push(Op::Insert(Box::new(Placement {
            slot,
            layer,
            cels: Vec::new(),
        })));
        Ok(id)
    }

    /// Deletes a layer and, for a group, everything inside it. The sprite keeps at least one
    /// layer.
    pub fn remove_layer(&mut self, id: LayerId) -> Result<(), SpriteError> {
        let slot = self
            .sprite
            .slot_of(id)
            .ok_or(SpriteError::UnknownLayer(id.0))?;
        if slot.parent.is_none() && self.sprite.layers().len() == 1 {
            return Err(SpriteError::LastLayer);
        }

        // Taking it out and putting it back is the simplest way to collect its cels.
        let Some((slot, layer, cels)) = self.sprite.remove_layer(id) else {
            return Err(SpriteError::UnknownLayer(id.0));
        };
        let placement = Placement { slot, layer, cels };
        placement.insert(self.sprite);
        self.push(Op::Remove(Box::new(placement)));
        Ok(())
    }

    /// Copies a layer (with everything inside it) and puts the copy directly above the
    /// original. Returns the id of the copy.
    pub fn duplicate_layer(&mut self, id: LayerId) -> Result<LayerId, SpriteError> {
        let slot = self
            .sprite
            .slot_of(id)
            .ok_or(SpriteError::UnknownLayer(id.0))?;
        let mut copy = self.known(id)?.clone();

        // Every layer in the copy needs an id of its own.
        let mut renamed = HashMap::new();
        self.assign_new_ids(&mut copy, &mut renamed);
        copy.name.push_str(" copy");

        let cels = self
            .sprite
            .cels()
            .filter_map(|(layer, frame, cel)| Some((*renamed.get(&layer)?, frame, cel.clone())))
            .collect();
        let copy_id = copy.id;
        self.push(Op::Insert(Box::new(Placement {
            slot: Slot::new(slot.parent, slot.index + 1),
            layer: copy,
            cels,
        })));
        // A copied tilemap needs its tiles as well as the picture they make.
        self.edit_frames(|state| {
            let copies: Vec<_> = state
                .grids
                .iter()
                .filter_map(|((layer, frame), grid)| {
                    Some(((*renamed.get(layer)?, *frame), grid.clone()))
                })
                .collect();
            state.grids.extend(copies);
        });
        Ok(copy_id)
    }

    fn assign_new_ids(&mut self, layer: &mut Layer, renamed: &mut HashMap<LayerId, LayerId>) {
        let id = self.new_layer_id();
        renamed.insert(layer.id, id);
        layer.id = id;
        if let LayerKind::Group(group) = &mut layer.kind {
            for child in &mut group.children {
                self.assign_new_ids(child, renamed);
            }
        }
    }

    /// Moves a layer to another place in the tree. `to.index` is the position the layer ends
    /// up at among its new siblings.
    pub fn move_layer(&mut self, id: LayerId, to: Slot) -> Result<(), SpriteError> {
        let from = self
            .sprite
            .slot_of(id)
            .ok_or(SpriteError::UnknownLayer(id.0))?;
        if let Some(parent) = to.parent
            && self.sprite.is_within(parent, id)
        {
            return Err(SpriteError::IntoItself);
        }
        self.check_slot(to, from.parent == to.parent)?;
        if from != to {
            self.push(Op::Move { id, from, to });
        }
        Ok(())
    }

    /// Changes a layer's name, visibility or image properties. `edit` receives the current
    /// properties to modify; for a group the image part is absent and stays absent.
    pub fn update_layer(
        &mut self,
        id: LayerId,
        edit: impl FnOnce(&mut LayerProps),
    ) -> Result<(), SpriteError> {
        let before = self.known(id)?.props();
        let mut after = before.clone();
        edit(&mut after);
        if before.image.is_none() {
            after.image = None;
        } else if after.image.is_none() {
            after.image = before.image;
        }
        if before.reference.is_none() {
            after.reference = None;
        } else if after.reference.is_none() {
            after.reference = before.reference;
        }
        if after != before {
            self.push(Op::Props { id, before, after });
        }
        Ok(())
    }

    /// Paints an image layer onto the image layer directly below it — with its opacity and
    /// blend mode, in every frame — and deletes it. Returns the id of the layer below.
    pub fn merge_down(&mut self, id: LayerId) -> Result<LayerId, SpriteError> {
        let slot = self
            .sprite
            .slot_of(id)
            .ok_or(SpriteError::UnknownLayer(id.0))?;
        let upper = *self
            .known(id)?
            .as_image()
            .ok_or(SpriteError::NotAnImageLayer(id.0))?;
        let below = slot
            .index
            .checked_sub(1)
            .and_then(|index| self.sprite.siblings(slot.parent).ok()?.get(index))
            .filter(|layer| layer.as_image().is_some() && !layer.is_tilemap())
            .ok_or(SpriteError::NothingBelow)?
            .id;

        for frame in 0..self.sprite.frames().len() {
            let Some(cel) = self.sprite.cel(id, frame) else {
                continue;
            };
            let rect = cel.rect();
            let mut merged = self.sprite.read_region(below, frame, rect);
            for y in 0..rect.h as i32 {
                for x in 0..rect.w as i32 {
                    let top = compositor::with_opacity(cel.image().get(x, y), upper.opacity);
                    merged.set(x, y, compositor::blend(upper.blend, top, merged.get(x, y)));
                }
            }
            self.set_pixels(below, frame, rect, &merged)?;
        }
        self.remove_layer(id)?;
        Ok(below)
    }

    /// Replaces all layers with a single image layer that looks like the sprite does now.
    /// Hidden layers are not part of what the sprite looks like, so they are dropped.
    /// Reference layers at the top level are not part of the picture at all and stay.
    /// Returns the id of the new layer.
    pub fn flatten(&mut self) -> Result<LayerId, SpriteError> {
        let id = self.new_layer_id();
        let cels = (0..self.sprite.frames().len())
            .filter_map(|frame| {
                let flat = compositor::compose(self.sprite, frame);
                Some((id, frame, Cel::cropped(&flat, Point::new(0, 0))?))
            })
            .collect();

        // The new layer goes in first, so the sprite is never without a layer.
        self.push(Op::Insert(Box::new(Placement {
            slot: Slot::new(None, 0),
            layer: Layer::image(id, "Flattened"),
            cels,
        })));
        let old: Vec<LayerId> = self.sprite.layers()[1..]
            .iter()
            .filter(|layer| !layer.is_reference())
            .map(|layer| layer.id)
            .collect();
        for layer in old {
            self.remove_layer(layer)?;
        }
        Ok(id)
    }
}

struct Entry {
    /// The document's revision once this entry is applied.
    revision: u64,
    name: String,
    ops: Vec<Op>,
    /// Entries with the same token, made one after the other, are one undo step.
    merge: Option<u64>,
}

impl Entry {
    fn byte_size(&self) -> usize {
        self.ops.iter().map(Op::byte_size).sum()
    }

    /// Folds a follow-up change into this entry, if both only change layer properties.
    /// Returns the ops back if they cannot be folded in.
    fn absorb(&mut self, ops: Vec<Op>) -> Result<(), Vec<Op>> {
        let only_props = |ops: &[Op]| ops.iter().all(|op| matches!(op, Op::Props { .. }));
        if !only_props(&self.ops) || !only_props(&ops) {
            return Err(ops);
        }
        for op in ops {
            let Op::Props { id, after, .. } = &op else {
                continue;
            };
            let earlier = self.ops.iter_mut().find_map(|mine| match mine {
                Op::Props {
                    id: mine_id, after, ..
                } if mine_id == id => Some(after),
                _ => None,
            });
            match earlier {
                // The first "before" and the latest "after" are what undo and redo need.
                Some(mine_after) => mine_after.clone_from(after),
                None => self.ops.push(op),
            }
        }
        Ok(())
    }
}

pub struct Document {
    sprite: Sprite,
    /// Undo steps, oldest first. The first `cursor` of them are applied.
    entries: VecDeque<Entry>,
    cursor: usize,
    /// The revision of the state before `entries[0]`.
    base_revision: u64,
    last_revision: u64,
    history_limit: usize,
    /// Never reused, even after an undo, so an id always means the same layer.
    next_layer_id: u32,
    /// Canvas area changed since the last [`Document::take_damage`].
    damage: Rect,
    active_layer: LayerId,
    active_frame: usize,
    /// The slice that is selected, if any. Not part of undo.
    active_slice: Option<usize>,
}

impl Document {
    pub fn new(sprite: Sprite) -> Self {
        let highest_id = sprite.walk().iter().map(|entry| entry.layer.id.0).max();
        let mut document = Self {
            sprite,
            entries: VecDeque::new(),
            cursor: 0,
            base_revision: 0,
            last_revision: 0,
            history_limit: DEFAULT_HISTORY_BYTES,
            next_layer_id: highest_id.map_or(1, |id| id + 1),
            damage: Rect::EMPTY,
            active_layer: LayerId(0),
            active_frame: 0,
            active_slice: None,
        };
        document.repair_active_layer();
        document
    }

    pub fn sprite(&self) -> &Sprite {
        &self.sprite
    }

    /// The layer that is selected: tools draw on it and layer commands act on it. It may be
    /// a group.
    pub fn active_layer(&self) -> LayerId {
        self.active_layer
    }

    /// Selects a layer. Returns `false` if there is no such layer. Not an undo step.
    pub fn set_active_layer(&mut self, id: LayerId) -> bool {
        let exists = self.sprite.layer(id).is_some();
        if exists {
            self.active_layer = id;
        }
        exists
    }

    /// Shows or hides the children of a group in layer lists. Returns `false` if `id` is not
    /// a group. This is how the tree is viewed, not what it contains: it is not an undo step.
    pub fn set_group_collapsed(&mut self, id: LayerId, collapsed: bool) -> bool {
        self.sprite.set_collapsed(id, collapsed)
    }

    /// Sets how strokes are mirrored. Like folding a group, this is about how the sprite is
    /// worked on, not what is in it: it is not an undo step.
    pub fn set_symmetry(&mut self, symmetry: Symmetry) {
        self.sprite.set_symmetry(symmetry);
    }

    /// Makes sure the active layer exists, falling back to the topmost image layer.
    fn repair_active_layer(&mut self) {
        self.active_frame = self.active_frame.min(self.sprite.frames().len() - 1);
        self.active_slice = self
            .active_slice
            .filter(|slice| *slice < self.sprite.slices().len());
        if self.sprite.layer(self.active_layer).is_some() {
            return;
        }
        let entries = self.sprite.walk_top_down();
        let fallback = entries
            .iter()
            .find(|entry| entry.layer.as_image().is_some())
            .or(entries.first());
        if let Some(entry) = fallback {
            self.active_layer = entry.layer.id;
        }
    }

    /// The selected slice, if any.
    pub fn active_slice(&self) -> Option<usize> {
        self.active_slice
    }

    /// Selects a slice, or none. Not an undo step.
    pub fn set_active_slice(&mut self, slice: Option<usize>) {
        self.active_slice = slice.filter(|slice| *slice < self.sprite.slices().len());
    }

    /// The frame being shown and edited.
    pub fn active_frame(&self) -> usize {
        self.active_frame
    }

    /// Shows another frame. Returns `false` if there is no such frame. Not an undo step.
    pub fn set_active_frame(&mut self, frame: usize) -> bool {
        let exists = frame < self.sprite.frames().len();
        if exists && frame != self.active_frame {
            self.active_frame = frame;
            self.damage = Rect::from_size(MAX_SPRITE_SIZE, MAX_SPRITE_SIZE);
        }
        exists
    }

    /// Runs `edit` and records everything it changes as one undo step called `name`.
    ///
    /// If `edit` fails, its changes are rolled back and nothing is recorded. A transaction
    /// that changes nothing leaves no undo step either.
    pub fn transact<T, E>(
        &mut self,
        name: &str,
        edit: impl FnOnce(&mut Transaction<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        self.transact_merging(name, None, edit)
    }

    /// Like [`Document::transact`], but consecutive transactions given the same `merge` token
    /// that only change layer properties become a single undo step. This is for a value being
    /// dragged: every step of the drag applies at once, and undo takes back the whole drag.
    pub fn transact_merging<T, E>(
        &mut self,
        name: &str,
        merge: Option<u64>,
        edit: impl FnOnce(&mut Transaction<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut transaction = Transaction {
            sprite: &mut self.sprite,
            next_layer_id: &mut self.next_layer_id,
            ops: Vec::new(),
        };
        let result = edit(&mut transaction);
        let mut ops = transaction.ops;

        let value = match result {
            Ok(value) => value,
            Err(err) => {
                for op in ops.iter().rev() {
                    op.revert(&mut self.sprite);
                }
                return Err(err);
            }
        };
        if ops.is_empty() {
            return Ok(value);
        }

        for op in &ops {
            self.damage = self.damage.union(op.damage(&self.sprite));
        }
        // A new change discards whatever could have been redone.
        self.entries.truncate(self.cursor);
        self.last_revision += 1;

        if let Some(last) = self.entries.back_mut()
            && merge.is_some()
            && last.merge == merge
        {
            match last.absorb(ops) {
                Ok(()) => {
                    last.revision = self.last_revision;
                    self.repair_active_layer();
                    return Ok(value);
                }
                Err(unmerged) => ops = unmerged,
            }
        }

        self.entries.push_back(Entry {
            revision: self.last_revision,
            name: name.to_owned(),
            ops,
            merge,
        });
        self.cursor = self.entries.len();
        self.enforce_history_limit();
        self.repair_active_layer();
        Ok(value)
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.entries.len()
    }

    /// Reverts the most recent applied step. Returns `false` if there was none.
    pub fn undo(&mut self) -> bool {
        if !self.can_undo() {
            return false;
        }
        self.cursor -= 1;
        for op in self.entries[self.cursor].ops.iter().rev() {
            // Measured on both sides: a layer that is about to vanish has no bounds after.
            self.damage = self.damage.union(op.damage(&self.sprite));
            op.revert(&mut self.sprite);
            self.damage = self.damage.union(op.damage(&self.sprite));
        }
        self.repair_active_layer();
        true
    }

    /// Re-applies the step that was undone last. Returns `false` if there was none.
    pub fn redo(&mut self) -> bool {
        if !self.can_redo() {
            return false;
        }
        for op in &self.entries[self.cursor].ops {
            self.damage = self.damage.union(op.damage(&self.sprite));
            op.apply(&mut self.sprite);
            self.damage = self.damage.union(op.damage(&self.sprite));
        }
        self.cursor += 1;
        self.repair_active_layer();
        true
    }

    /// Names of the undo steps, oldest first.
    pub fn history(&self) -> impl ExactSizeIterator<Item = &str> {
        self.entries.iter().map(|entry| entry.name.as_str())
    }

    /// How many of the steps in [`Document::history`] are currently applied.
    pub fn history_cursor(&self) -> usize {
        self.cursor
    }

    /// Undoes or redoes until exactly `cursor` steps are applied.
    pub fn jump_to(&mut self, cursor: usize) {
        let cursor = cursor.min(self.entries.len());
        while self.cursor > cursor {
            self.undo();
        }
        while self.cursor < cursor {
            self.redo();
        }
    }

    /// A number that identifies the current state of the sprite: it changes with every edit,
    /// and undoing back to an earlier state brings the earlier number back.
    pub fn revision(&self) -> u64 {
        match self.cursor {
            0 => self.base_revision,
            applied => self.entries[applied - 1].revision,
        }
    }

    /// Bytes of pixel data held by the undo history.
    pub fn history_bytes(&self) -> usize {
        self.entries.iter().map(Entry::byte_size).sum()
    }

    /// Changes how much memory the history may use. Oldest steps are dropped first.
    pub fn set_history_limit(&mut self, bytes: usize) {
        self.history_limit = bytes;
        self.enforce_history_limit();
    }

    fn enforce_history_limit(&mut self) {
        // The newest step always stays, and only steps that are applied can be forgotten.
        while self.history_bytes() > self.history_limit && self.entries.len() > 1 && self.cursor > 0
        {
            if let Some(dropped) = self.entries.pop_front() {
                self.base_revision = dropped.revision;
                self.cursor -= 1;
            }
        }
    }

    /// Marks a canvas area as needing to be redrawn without changing the sprite, e.g. because
    /// a tool preview over it changed.
    pub fn add_damage(&mut self, rect: Rect) {
        self.damage = self.damage.union(rect);
    }

    /// The canvas area that changed since the last call; empty if nothing did.
    pub fn take_damage(&mut self) -> Rect {
        std::mem::take(&mut self.damage).intersect(self.sprite.bounds())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;
    use crate::sprite::{BlendMode, ImageLayer};

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const BLUE: Rgba8 = Rgba8::rgb(0, 0, 255);
    const L1: LayerId = LayerId(1);

    fn filled(w: u32, h: u32, color: Rgba8) -> Image {
        Image::from_pixels(w, h, vec![color; (w * h) as usize]).unwrap()
    }

    fn document() -> Document {
        Document::new(Sprite::new(16, 16).unwrap())
    }

    fn paint_on(doc: &mut Document, layer: LayerId, name: &str, rect: Rect, color: Rgba8) {
        doc.transact(name, |tx| {
            tx.set_pixels(layer, 0, rect, &filled(rect.w, rect.h, color))
        })
        .unwrap();
    }

    fn paint(doc: &mut Document, name: &str, rect: Rect, color: Rgba8) {
        paint_on(doc, L1, name, rect, color);
    }

    fn top(index: usize) -> Slot {
        Slot::new(None, index)
    }

    fn order(doc: &Document) -> Vec<u32> {
        doc.sprite()
            .walk()
            .iter()
            .map(|entry| entry.layer.id.0)
            .collect()
    }

    /// Runs `edit` as one transaction, then checks that undo restores the sprite exactly and
    /// redo brings the edited sprite back exactly.
    fn check_undo_redo<T>(
        doc: &mut Document,
        edit: impl FnOnce(&mut Transaction<'_>) -> Result<T, SpriteError>,
    ) -> T {
        let before = doc.sprite().clone();
        let steps = doc.history().len();
        let value = doc.transact("Edit", edit).unwrap();
        let after = doc.sprite().clone();
        assert_ne!(before, after, "the edit changed nothing");
        assert_eq!(doc.history().len(), steps + 1, "not exactly one undo step");

        assert!(doc.undo());
        assert_eq!(*doc.sprite(), before, "undo did not restore the sprite");
        assert!(doc.redo());
        assert_eq!(*doc.sprite(), after, "redo did not restore the edit");
        value
    }

    #[test]
    fn do_undo_redo_returns_identical_data() {
        let mut doc = document();
        let blank = doc.sprite().clone();

        paint(&mut doc, "Red", Rect::new(2, 2, 4, 4), RED);
        let red = doc.sprite().clone();
        paint(&mut doc, "Blue", Rect::new(4, 4, 6, 2), BLUE);
        let blue = doc.sprite().clone();
        paint(
            &mut doc,
            "Erase",
            Rect::new(0, 0, 16, 5),
            Rgba8::TRANSPARENT,
        );
        let erased = doc.sprite().clone();
        assert_ne!(blue, erased);

        assert!(doc.undo());
        assert_eq!(*doc.sprite(), blue);
        assert!(doc.undo());
        assert_eq!(*doc.sprite(), red);
        assert!(doc.undo());
        assert_eq!(*doc.sprite(), blank);
        assert!(!doc.undo());

        assert!(doc.redo());
        assert_eq!(*doc.sprite(), red);
        assert!(doc.redo());
        assert_eq!(*doc.sprite(), blue);
        assert!(doc.redo());
        assert_eq!(*doc.sprite(), erased);
        assert!(!doc.redo());
    }

    #[test]
    fn only_the_changed_area_is_stored() {
        let mut doc = document();
        paint(&mut doc, "Red", Rect::new(0, 0, 16, 16), RED);
        let full = doc.history_bytes();
        assert_eq!(full, 16 * 16 * 4 * 2);

        // Repainting everything red again changes nothing.
        paint(&mut doc, "Same", Rect::new(0, 0, 16, 16), RED);
        assert_eq!(doc.history().len(), 1);

        // One blue pixel in a full-canvas write stores one pixel before and after.
        doc.transact("Dot", |tx| {
            let mut pixels = filled(16, 16, RED);
            pixels.set(5, 7, BLUE);
            tx.set_pixels(L1, 0, Rect::new(0, 0, 16, 16), &pixels)
        })
        .unwrap();
        assert_eq!(doc.history_bytes(), full + 4 * 2);
        assert_eq!(doc.take_damage(), Rect::new(0, 0, 16, 16));
        doc.undo();
        assert_eq!(doc.take_damage(), Rect::new(5, 7, 1, 1));
    }

    #[test]
    fn a_transaction_is_one_undo_step_however_many_writes_it_makes() {
        let mut doc = document();
        doc.transact("Two dots", |tx| {
            tx.set_pixels(L1, 0, Rect::new(1, 1, 1, 1), &filled(1, 1, RED))?;
            tx.set_pixels(L1, 0, Rect::new(9, 9, 1, 1), &filled(1, 1, BLUE))
        })
        .unwrap();
        assert_eq!(doc.history().collect::<Vec<_>>(), ["Two dots"]);

        doc.undo();
        assert_eq!(doc.sprite().cels().count(), 0);
    }

    #[test]
    fn a_failed_transaction_is_rolled_back() {
        let mut doc = document();
        paint(&mut doc, "Red", Rect::new(0, 0, 2, 2), RED);
        let before = doc.sprite().clone();
        let result = doc.transact("Broken", |tx| {
            tx.set_pixels(L1, 0, Rect::new(1, 1, 1, 1), &filled(1, 1, BLUE))?;
            tx.add_layer(top(1), "New", false)?;
            tx.update_layer(L1, |props| props.visible = false)?;
            tx.set_pixels(LayerId(99), 0, Rect::new(1, 1, 1, 1), &filled(1, 1, RED))
        });
        assert_eq!(result, Err(SpriteError::UnknownLayer(99)));
        assert_eq!(*doc.sprite(), before);
        assert_eq!(doc.history().len(), 1);
    }

    #[test]
    fn wrongly_sized_pixel_data_is_an_error() {
        let mut doc = document();
        let result = doc.transact("Bad", |tx| {
            tx.set_pixels(L1, 0, Rect::new(0, 0, 2, 2), &filled(3, 2, RED))
        });
        assert!(matches!(result, Err(SpriteError::SizeMismatch { .. })));
    }

    #[test]
    fn a_new_edit_discards_the_redo_steps() {
        let mut doc = document();
        paint(&mut doc, "A", Rect::new(0, 0, 1, 1), RED);
        paint(&mut doc, "B", Rect::new(1, 0, 1, 1), RED);
        doc.undo();
        paint(&mut doc, "C", Rect::new(2, 0, 1, 1), RED);
        assert_eq!(doc.history().collect::<Vec<_>>(), ["A", "C"]);
        assert!(!doc.can_redo());
    }

    #[test]
    fn jumping_moves_the_cursor_both_ways() {
        let mut doc = document();
        let mut states = vec![doc.sprite().clone()];
        for x in 0..4 {
            paint(&mut doc, "Dot", Rect::new(x, 0, 1, 1), RED);
            states.push(doc.sprite().clone());
        }
        for target in [1, 4, 0, 3, 99] {
            doc.jump_to(target);
            let expected = target.min(4);
            assert_eq!(doc.history_cursor(), expected);
            assert_eq!(*doc.sprite(), states[expected]);
        }
    }

    #[test]
    fn revision_follows_the_state() {
        let mut doc = document();
        let start = doc.revision();
        paint(&mut doc, "A", Rect::new(0, 0, 1, 1), RED);
        let after_a = doc.revision();
        assert_ne!(start, after_a);

        doc.undo();
        assert_eq!(doc.revision(), start);
        doc.redo();
        assert_eq!(doc.revision(), after_a);

        // A different edit from the same starting point is a different state.
        doc.undo();
        paint(&mut doc, "B", Rect::new(1, 0, 1, 1), RED);
        assert_ne!(doc.revision(), after_a);
        assert_ne!(doc.revision(), start);
    }

    #[test]
    fn the_history_limit_drops_the_oldest_steps() {
        let mut doc = document();
        for x in 0..8 {
            paint(&mut doc, &format!("Dot {x}"), Rect::new(x, 0, 1, 1), RED);
        }
        let final_state = doc.sprite().clone();
        let revision = doc.revision();

        // Each step holds one pixel before and after: 8 bytes. Keep room for three.
        doc.set_history_limit(24);
        assert_eq!(
            doc.history().collect::<Vec<_>>(),
            ["Dot 5", "Dot 6", "Dot 7"]
        );
        assert_eq!(doc.history_cursor(), 3);
        assert_eq!(*doc.sprite(), final_state);
        assert_eq!(doc.revision(), revision);

        // Undoing as far as possible lands on the state after the dropped steps.
        while doc.undo() {}
        assert_eq!(
            doc.sprite().cel(L1, 0).unwrap().rect(),
            Rect::new(0, 0, 5, 1)
        );
        assert_ne!(doc.revision(), 0);

        // Even a tiny limit keeps the newest step.
        doc.jump_to(3);
        doc.set_history_limit(0);
        assert_eq!(doc.history().collect::<Vec<_>>(), ["Dot 7"]);
        assert!(doc.can_undo());
    }

    #[test]
    fn damage_accumulates_until_taken() {
        let mut doc = document();
        assert!(doc.take_damage().is_empty());
        paint(&mut doc, "A", Rect::new(1, 1, 1, 1), RED);
        paint(&mut doc, "B", Rect::new(4, 3, 1, 1), RED);
        assert_eq!(doc.take_damage(), Rect::new(1, 1, 4, 3));
        assert!(doc.take_damage().is_empty());

        doc.add_damage(Rect::new(-5, -5, 100, 100));
        assert_eq!(doc.take_damage(), Rect::new(0, 0, 16, 16));
    }

    #[test]
    fn adding_layers_and_groups() {
        let mut doc = document();
        let layer = check_undo_redo(&mut doc, |tx| tx.add_layer(top(1), "Ink", false));
        let group = check_undo_redo(&mut doc, |tx| tx.add_layer(top(2), "Folder", true));
        let inner = check_undo_redo(&mut doc, |tx| {
            tx.add_layer(Slot::new(Some(group), 0), "Inside", false)
        });
        assert_eq!(order(&doc), [1, layer.0, group.0, inner.0]);
        assert_eq!(doc.sprite().layer(layer).unwrap().name, "Ink");
        assert!(doc.sprite().layer(group).unwrap().is_group());
        assert_eq!(doc.sprite().slot_of(inner), Some(Slot::new(Some(group), 0)));

        // Places that do not exist are refused.
        let bad =
            |doc: &mut Document, slot| doc.transact("Bad", |tx| tx.add_layer(slot, "X", false));
        assert_eq!(
            bad(&mut doc, top(9)),
            Err(SpriteError::InvalidSlot { index: 9 })
        );
        assert_eq!(
            bad(&mut doc, Slot::new(Some(layer), 0)),
            Err(SpriteError::NotAGroup(layer.0))
        );
        assert_eq!(
            bad(&mut doc, Slot::new(Some(LayerId(77)), 0)),
            Err(SpriteError::UnknownLayer(77))
        );
    }

    #[test]
    fn layer_ids_are_never_reused() {
        let mut doc = document();
        let first = doc
            .transact("Add", |tx| tx.add_layer(top(1), "A", false))
            .unwrap();
        doc.undo();
        let second = doc
            .transact("Add", |tx| tx.add_layer(top(1), "B", false))
            .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn deleting_a_layer_takes_its_pixels_and_undo_brings_them_back() {
        let mut doc = document();
        let upper = doc
            .transact("Add", |tx| tx.add_layer(top(1), "Upper", false))
            .unwrap();
        paint_on(&mut doc, upper, "Paint", Rect::new(3, 3, 2, 2), BLUE);
        paint(&mut doc, "Paint", Rect::new(0, 0, 2, 2), RED);
        doc.take_damage();

        check_undo_redo(&mut doc, |tx| tx.remove_layer(upper));
        assert_eq!(order(&doc), [1]);
        assert!(doc.sprite().cel(upper, 0).is_none());
        // Removing it changed how its area looks.
        assert_eq!(doc.take_damage(), Rect::new(3, 3, 2, 2));

        doc.undo();
        assert_eq!(doc.sprite().pixel(upper, 0, 4, 4), BLUE);
    }

    #[test]
    fn the_last_layer_cannot_be_deleted() {
        let mut doc = document();
        let result = doc.transact("Delete", |tx| tx.remove_layer(L1));
        assert_eq!(result, Err(SpriteError::LastLayer));

        // With a second layer either may go, but not both.
        let other = doc
            .transact("Add", |tx| tx.add_layer(top(1), "B", false))
            .unwrap();
        doc.transact("Delete", |tx| tx.remove_layer(L1)).unwrap();
        let result = doc.transact("Delete", |tx| tx.remove_layer(other));
        assert_eq!(result, Err(SpriteError::LastLayer));
        assert_eq!(
            doc.transact("Delete", |tx| tx.remove_layer(LayerId(50))),
            Err(SpriteError::UnknownLayer(50))
        );
    }

    #[test]
    fn deleting_a_group_deletes_everything_in_it() {
        let mut doc = document();
        let group = doc
            .transact("Add", |tx| tx.add_layer(top(1), "G", true))
            .unwrap();
        let inner = doc
            .transact("Add", |tx| {
                tx.add_layer(Slot::new(Some(group), 0), "In", false)
            })
            .unwrap();
        paint_on(&mut doc, inner, "Paint", Rect::new(5, 5, 1, 1), BLUE);

        check_undo_redo(&mut doc, |tx| tx.remove_layer(group));
        assert_eq!(order(&doc), [1]);
        assert_eq!(doc.sprite().cels().count(), 0);
        doc.undo();
        assert_eq!(doc.sprite().pixel(inner, 0, 5, 5), BLUE);
    }

    #[test]
    fn duplicating_copies_pixels_and_properties_under_new_ids() {
        let mut doc = document();
        paint(&mut doc, "Paint", Rect::new(1, 1, 2, 2), RED);
        doc.transact("Props", |tx| {
            tx.update_layer(L1, |props| {
                props.image = Some(ImageLayer {
                    opacity: 90,
                    blend: BlendMode::Screen,
                    ..ImageLayer::default()
                });
            })
        })
        .unwrap();

        let copy = check_undo_redo(&mut doc, |tx| tx.duplicate_layer(L1));
        assert_ne!(copy, L1);
        assert_eq!(order(&doc), [1, copy.0]);
        let (original, duplicate) = (
            doc.sprite().layer(L1).unwrap(),
            doc.sprite().layer(copy).unwrap(),
        );
        assert_eq!(duplicate.name, "Layer 1 copy");
        assert_eq!(duplicate.as_image(), original.as_image());
        assert_eq!(doc.sprite().pixel(copy, 0, 2, 2), RED);

        // The copy is independent: painting on it leaves the original alone.
        paint_on(&mut doc, copy, "Paint", Rect::new(1, 1, 1, 1), BLUE);
        assert_eq!(doc.sprite().pixel(L1, 0, 1, 1), RED);
        assert_eq!(doc.sprite().pixel(copy, 0, 1, 1), BLUE);
    }

    #[test]
    fn duplicating_a_group_copies_its_whole_content() {
        let mut doc = document();
        let group = doc
            .transact("Add", |tx| tx.add_layer(top(1), "G", true))
            .unwrap();
        let inner = doc
            .transact("Add", |tx| {
                tx.add_layer(Slot::new(Some(group), 0), "In", false)
            })
            .unwrap();
        paint_on(&mut doc, inner, "Paint", Rect::new(5, 5, 1, 1), BLUE);

        let copy = check_undo_redo(&mut doc, |tx| tx.duplicate_layer(group));
        let copied_group = doc.sprite().layer(copy).unwrap();
        assert_eq!(copied_group.children().len(), 1);
        let copied_inner = copied_group.children()[0].id;
        assert_ne!(copied_inner, inner);
        assert_eq!(doc.sprite().pixel(copied_inner, 0, 5, 5), BLUE);
        assert_eq!(doc.sprite().slot_of(copy), Some(top(2)));
    }

    #[test]
    fn moving_layers_around_the_tree() {
        let mut doc = document();
        let a = doc
            .transact("Add", |tx| tx.add_layer(top(1), "A", false))
            .unwrap();
        let group = doc
            .transact("Add", |tx| tx.add_layer(top(2), "G", true))
            .unwrap();
        let b = doc
            .transact("Add", |tx| {
                tx.add_layer(Slot::new(Some(group), 0), "B", false)
            })
            .unwrap();
        assert_eq!(order(&doc), [1, a.0, group.0, b.0]);

        // Reorder at the top level.
        check_undo_redo(&mut doc, |tx| tx.move_layer(a, top(0)));
        assert_eq!(order(&doc), [a.0, 1, group.0, b.0]);
        // Into a group.
        check_undo_redo(&mut doc, |tx| tx.move_layer(L1, Slot::new(Some(group), 1)));
        assert_eq!(order(&doc), [a.0, group.0, b.0, 1]);
        // Out of it again.
        check_undo_redo(&mut doc, |tx| tx.move_layer(b, top(2)));
        assert_eq!(order(&doc), [a.0, group.0, 1, b.0]);

        // Moving to where it already is changes nothing and leaves no undo step.
        let steps = doc.history().len();
        doc.transact("Move", |tx| tx.move_layer(b, top(2))).unwrap();
        assert_eq!(doc.history().len(), steps);

        let try_move =
            |doc: &mut Document, id, to| doc.transact("Move", |tx| tx.move_layer(id, to));
        assert_eq!(
            try_move(&mut doc, group, Slot::new(Some(group), 0)),
            Err(SpriteError::IntoItself)
        );
        assert_eq!(
            try_move(&mut doc, a, top(3)),
            Err(SpriteError::InvalidSlot { index: 3 })
        );
        assert_eq!(
            try_move(&mut doc, a, Slot::new(Some(b), 0)),
            Err(SpriteError::NotAGroup(b.0))
        );
        // The last valid index is one less when the layer is already in that list.
        assert!(try_move(&mut doc, a, top(2)).is_ok());
    }

    #[test]
    fn a_group_cannot_be_moved_deeper_into_itself() {
        let mut doc = document();
        let outer = doc
            .transact("Add", |tx| tx.add_layer(top(1), "Outer", true))
            .unwrap();
        let inner = doc
            .transact("Add", |tx| {
                tx.add_layer(Slot::new(Some(outer), 0), "Inner", true)
            })
            .unwrap();
        let result = doc.transact("Move", |tx| tx.move_layer(outer, Slot::new(Some(inner), 0)));
        assert_eq!(result, Err(SpriteError::IntoItself));
    }

    #[test]
    fn property_changes_undo_and_groups_keep_no_image_properties() {
        let mut doc = document();
        check_undo_redo(&mut doc, |tx| {
            tx.update_layer(L1, |props| props.name = "Sky".to_owned())
        });
        check_undo_redo(&mut doc, |tx| {
            tx.update_layer(L1, |props| props.visible = false)
        });
        check_undo_redo(&mut doc, |tx| {
            tx.update_layer(L1, |props| {
                if let Some(image) = &mut props.image {
                    image.opacity = 128;
                    image.blend = BlendMode::Add;
                    image.locked = true;
                    image.alpha_lock = true;
                }
            })
        });
        let layer = doc.sprite().layer(L1).unwrap();
        assert_eq!((layer.name.as_str(), layer.visible), ("Sky", false));
        assert_eq!(
            layer.as_image(),
            Some(&ImageLayer {
                opacity: 128,
                blend: BlendMode::Add,
                locked: true,
                alpha_lock: true
            })
        );

        // Setting the same value again is not a change.
        let steps = doc.history().len();
        doc.transact("Same", |tx| {
            tx.update_layer(L1, |props| props.visible = false)
        })
        .unwrap();
        assert_eq!(doc.history().len(), steps);

        // An image layer cannot lose its image properties, a group cannot gain them.
        doc.transact("Odd", |tx| tx.update_layer(L1, |props| props.image = None))
            .unwrap();
        assert!(doc.sprite().layer(L1).unwrap().as_image().is_some());
        let group = doc
            .transact("Add", |tx| tx.add_layer(top(1), "G", true))
            .unwrap();
        doc.transact("Odd", |tx| {
            tx.update_layer(group, |props| props.image = Some(ImageLayer::default()))
        })
        .unwrap();
        assert!(doc.sprite().layer(group).unwrap().is_group());
    }

    #[test]
    fn a_dragged_value_is_one_undo_step() {
        let mut doc = document();
        let set = |doc: &mut Document, opacity: u8, merge| {
            doc.transact_merging("Layer Opacity", merge, |tx| {
                tx.update_layer(L1, |props| {
                    if let Some(image) = &mut props.image {
                        image.opacity = opacity;
                    }
                })
            })
            .unwrap();
        };
        let opacity = |doc: &Document| doc.sprite().layer(L1).unwrap().as_image().unwrap().opacity;
        let original = doc.sprite().clone();

        // One drag: many values, one token.
        let revision = doc.revision();
        for value in [240, 200, 150, 90] {
            set(&mut doc, value, Some(1));
        }
        assert_eq!(doc.history().collect::<Vec<_>>(), ["Layer Opacity"]);
        assert_eq!(opacity(&doc), 90);
        assert_ne!(doc.revision(), revision);

        // A second drag is a second step.
        set(&mut doc, 60, Some(2));
        set(&mut doc, 30, Some(2));
        assert_eq!(doc.history().len(), 2);

        doc.undo();
        assert_eq!(opacity(&doc), 90);
        doc.undo();
        assert_eq!(*doc.sprite(), original);
        doc.redo();
        assert_eq!(opacity(&doc), 90);
        doc.redo();
        assert_eq!(opacity(&doc), 30);

        // Without a token nothing merges, and an undo in between ends a drag.
        set(&mut doc, 20, None);
        set(&mut doc, 10, None);
        assert_eq!(doc.history().len(), 4);
        doc.undo();
        set(&mut doc, 5, Some(2));
        assert_eq!(doc.history().len(), 4);
    }

    #[test]
    fn only_property_changes_merge() {
        let mut doc = document();
        // Two strokes with the same token stay two steps.
        for x in 0..2 {
            doc.transact_merging("Dot", Some(7), |tx| {
                tx.set_pixels(L1, 0, Rect::new(x, 0, 1, 1), &filled(1, 1, RED))
            })
            .unwrap();
        }
        assert_eq!(doc.history().len(), 2);
    }

    #[test]
    fn a_locked_layer_refuses_pixels() {
        let mut doc = document();
        doc.transact("Lock", |tx| {
            tx.update_layer(L1, |props| {
                if let Some(image) = &mut props.image {
                    image.locked = true;
                }
            })
        })
        .unwrap();
        let result = doc.transact("Paint", |tx| {
            tx.set_pixels(L1, 0, Rect::new(0, 0, 1, 1), &filled(1, 1, RED))
        });
        assert_eq!(result, Err(SpriteError::LayerLocked(1)));

        let group = doc
            .transact("Add", |tx| tx.add_layer(top(1), "G", true))
            .unwrap();
        let result = doc.transact("Paint", |tx| {
            tx.set_pixels(group, 0, Rect::new(0, 0, 1, 1), &filled(1, 1, RED))
        });
        assert_eq!(result, Err(SpriteError::NotAnImageLayer(group.0)));
    }

    #[test]
    fn merging_down_keeps_the_look_and_removes_the_upper_layer() {
        let mut doc = document();
        paint(&mut doc, "Paint", Rect::new(0, 0, 4, 1), RED);
        let upper = doc
            .transact("Add", |tx| tx.add_layer(top(1), "Upper", false))
            .unwrap();
        paint_on(
            &mut doc,
            upper,
            "Paint",
            Rect::new(2, 0, 4, 1),
            Rgba8::new(0, 0, 255, 128),
        );
        let look = compositor::compose(doc.sprite(), 0);

        let below = check_undo_redo(&mut doc, |tx| tx.merge_down(upper));
        assert_eq!(below, L1);
        assert_eq!(order(&doc), [1]);
        assert_eq!(compositor::compose(doc.sprite(), 0), look);
        assert_eq!(doc.sprite().pixel(L1, 0, 0, 0), RED);
        assert_eq!(doc.sprite().pixel(L1, 0, 5, 0), Rgba8::new(0, 0, 255, 128));
    }

    #[test]
    fn merging_down_applies_opacity_and_blend_mode() {
        let mut doc = document();
        paint(
            &mut doc,
            "Paint",
            Rect::new(0, 0, 2, 1),
            Rgba8::rgb(200, 100, 50),
        );
        let upper = doc
            .transact("Add", |tx| tx.add_layer(top(1), "Upper", false))
            .unwrap();
        paint_on(
            &mut doc,
            upper,
            "Paint",
            Rect::new(0, 0, 2, 1),
            Rgba8::rgb(128, 128, 128),
        );
        doc.transact("Props", |tx| {
            tx.update_layer(upper, |props| {
                props.image = Some(ImageLayer {
                    opacity: 128,
                    blend: BlendMode::Multiply,
                    ..ImageLayer::default()
                });
            })
        })
        .unwrap();
        let look = compositor::compose(doc.sprite(), 0);

        doc.transact("Merge", |tx| tx.merge_down(upper)).unwrap();
        assert_eq!(compositor::compose(doc.sprite(), 0), look);
        assert_ne!(doc.sprite().pixel(L1, 0, 0, 0), Rgba8::rgb(200, 100, 50));
    }

    #[test]
    fn merging_down_needs_an_unlocked_image_layer_right_below() {
        let mut doc = document();
        let merge = |doc: &mut Document, id| doc.transact("Merge", |tx| tx.merge_down(id));
        assert_eq!(merge(&mut doc, L1), Err(SpriteError::NothingBelow));

        let group = doc
            .transact("Add", |tx| tx.add_layer(top(1), "G", true))
            .unwrap();
        let upper = doc
            .transact("Add", |tx| tx.add_layer(top(2), "Upper", false))
            .unwrap();
        assert_eq!(merge(&mut doc, upper), Err(SpriteError::NothingBelow));
        assert_eq!(
            merge(&mut doc, group),
            Err(SpriteError::NotAnImageLayer(group.0))
        );

        // Onto a locked layer: refused, and nothing is left half done.
        doc.transact("Move", |tx| tx.move_layer(upper, top(1)))
            .unwrap();
        paint_on(&mut doc, upper, "Paint", Rect::new(0, 0, 1, 1), RED);
        doc.transact("Lock", |tx| {
            tx.update_layer(L1, |props| {
                if let Some(image) = &mut props.image {
                    image.locked = true;
                }
            })
        })
        .unwrap();
        let before = doc.sprite().clone();
        assert_eq!(merge(&mut doc, upper), Err(SpriteError::LayerLocked(1)));
        assert_eq!(*doc.sprite(), before);
    }

    #[test]
    fn flattening_leaves_one_layer_that_looks_the_same() {
        let mut doc = document();
        paint(&mut doc, "Paint", Rect::new(0, 0, 6, 6), RED);
        let group = doc
            .transact("Add", |tx| tx.add_layer(top(1), "G", true))
            .unwrap();
        let inner = doc
            .transact("Add", |tx| {
                tx.add_layer(Slot::new(Some(group), 0), "In", false)
            })
            .unwrap();
        paint_on(
            &mut doc,
            inner,
            "Paint",
            Rect::new(3, 3, 6, 6),
            Rgba8::new(0, 0, 255, 100),
        );
        let hidden = doc
            .transact("Add", |tx| tx.add_layer(top(2), "Hidden", false))
            .unwrap();
        paint_on(
            &mut doc,
            hidden,
            "Paint",
            Rect::new(0, 0, 16, 16),
            Rgba8::WHITE,
        );
        doc.transact("Hide", |tx| {
            tx.update_layer(hidden, |props| props.visible = false)
        })
        .unwrap();
        let look = compositor::compose(doc.sprite(), 0);

        let flat = check_undo_redo(&mut doc, |tx| tx.flatten());
        assert_eq!(order(&doc), [flat.0]);
        assert_eq!(doc.sprite().layer(flat).unwrap().name, "Flattened");
        assert_eq!(compositor::compose(doc.sprite(), 0), look);
        assert_eq!(doc.sprite().cels().count(), 1);
        assert_eq!(
            doc.sprite().cel(flat, 0).unwrap().rect(),
            Rect::new(0, 0, 9, 9)
        );
    }

    #[test]
    fn the_active_layer_follows_what_exists() {
        let mut doc = document();
        assert_eq!(doc.active_layer(), L1);

        let upper = doc
            .transact("Add", |tx| tx.add_layer(top(1), "Upper", false))
            .unwrap();
        assert_eq!(doc.active_layer(), L1);
        assert!(doc.set_active_layer(upper));
        assert!(!doc.set_active_layer(LayerId(99)));
        assert_eq!(doc.active_layer(), upper);

        // Undoing the layer's creation moves the selection to one that exists.
        doc.undo();
        assert_eq!(doc.active_layer(), L1);
        doc.redo();
        doc.set_active_layer(upper);
        doc.transact("Delete", |tx| tx.remove_layer(upper)).unwrap();
        assert_eq!(doc.active_layer(), L1);

        let flat = doc.transact("Flatten", |tx| tx.flatten()).unwrap();
        assert_eq!(doc.active_layer(), flat);
    }

    #[test]
    fn collapsing_a_group_is_not_an_undo_step() {
        let mut doc = document();
        let group = doc
            .transact("Add", |tx| tx.add_layer(top(1), "G", true))
            .unwrap();
        let (steps, revision) = (doc.history().len(), doc.revision());

        assert!(doc.set_group_collapsed(group, true));
        assert!(
            doc.sprite()
                .layer(group)
                .unwrap()
                .as_group()
                .unwrap()
                .collapsed
        );
        assert!(!doc.set_group_collapsed(L1, true));
        assert_eq!((doc.history().len(), doc.revision()), (steps, revision));
    }

    #[test]
    fn palette_changes_are_undo_steps() {
        use crate::sprite::Palette;

        let mut doc = document();
        let original = doc.sprite().palette().clone();
        let new = Palette::from_colors([RED, BLUE]).with_ramps(vec![vec![RED, BLUE]]);
        doc.transact("Palette", |tx| {
            tx.set_palette(new.clone());
            Ok::<_, SpriteError>(())
        })
        .unwrap();
        assert_eq!(*doc.sprite().palette(), new);
        assert_eq!(doc.history().collect::<Vec<_>>(), ["Palette"]);
        // Nothing is redrawn for a palette change.
        assert!(doc.take_damage().is_empty());

        doc.undo();
        assert_eq!(*doc.sprite().palette(), original);
        doc.redo();
        assert_eq!(*doc.sprite().palette(), new);

        // Setting the palette it already has is not a step.
        doc.transact("Palette", |tx| {
            tx.set_palette(new.clone());
            Ok::<_, SpriteError>(())
        })
        .unwrap();
        assert_eq!(doc.history().len(), 1);
    }

    #[test]
    fn symmetry_is_a_setting_not_an_undo_step() {
        use crate::sprite::Symmetry;

        let mut doc = document();
        let revision = doc.revision();
        doc.set_symmetry(Symmetry::Radial(6));
        assert_eq!(doc.sprite().symmetry(), Symmetry::Radial(6));
        assert_eq!(doc.revision(), revision);
        assert!(!doc.can_undo());

        // Undoing something else leaves it alone.
        paint(&mut doc, "Paint", Rect::new(0, 0, 2, 2), RED);
        doc.undo();
        assert_eq!(doc.sprite().symmetry(), Symmetry::Radial(6));
    }

    /// A 16×16 document with a 32×16 reference picture on top. Returns the layer's id.
    fn with_reference(doc: &mut Document) -> LayerId {
        doc.transact("Reference", |tx| {
            tx.add_reference_layer(Slot::new(None, 1), "Photo", filled(32, 16, BLUE))
        })
        .unwrap()
    }

    fn reference_rect(doc: &Document, id: LayerId) -> Rect {
        doc.sprite().layer(id).unwrap().as_reference().unwrap().rect
    }

    #[test]
    fn a_reference_layer_is_fitted_shown_and_never_part_of_the_picture() {
        let mut doc = document();
        paint(&mut doc, "Paint", Rect::new(0, 0, 2, 2), RED);
        let before = doc.sprite().clone();
        let flat = compositor::compose(doc.sprite(), 0);

        let id = with_reference(&mut doc);
        let layer = doc.sprite().layer(id).unwrap();
        assert!(layer.is_reference() && layer.visible);
        assert_eq!(layer.as_reference().unwrap().opacity, 128);
        // Twice as wide as high: full width, half the height, in the middle.
        assert_eq!(reference_rect(&doc, id), Rect::new(0, 4, 16, 8));
        assert_eq!(compositor::compose(doc.sprite(), 0), flat);

        // It has no pixels to draw on, select or merge.
        assert_eq!(
            doc.sprite().check_drawable(id, 0),
            Err(SpriteError::NotAnImageLayer(id.0))
        );
        assert_eq!(
            doc.transact("Merge", |tx| tx.merge_down(id)),
            Err(SpriteError::NotAnImageLayer(id.0))
        );
        let upper = doc
            .transact("Add", |tx| tx.add_layer(Slot::new(None, 2), "Upper", false))
            .unwrap();
        assert_eq!(
            doc.transact("Merge", |tx| tx.merge_down(upper)),
            Err(SpriteError::NothingBelow)
        );
        doc.undo();

        doc.undo();
        assert_eq!(*doc.sprite(), before);
        doc.redo();
        assert!(doc.sprite().layer(id).is_some());

        assert_eq!(
            doc.transact("Reference", |tx| {
                tx.add_reference_layer(Slot::new(None, 0), "Nothing", Image::new(0, 0))
            }),
            Err(SpriteError::EmptyImage)
        );
    }

    #[test]
    fn reference_properties_change_like_any_layer_property() {
        let mut doc = document();
        let id = with_reference(&mut doc);
        let before = doc.sprite().clone();

        doc.transact("Opacity", |tx| {
            tx.update_layer(id, |props| {
                props.visible = false;
                if let Some(reference) = &mut props.reference {
                    reference.opacity = 40;
                    reference.rect = Rect::new(1, 1, 4, 2);
                }
                // A reference layer has no image properties to set.
                props.image = Some(ImageLayer::default());
            })
        })
        .unwrap();
        let layer = doc.sprite().layer(id).unwrap();
        assert!(!layer.visible && layer.as_image().is_none());
        assert_eq!(layer.as_reference().unwrap().opacity, 40);
        assert_eq!(reference_rect(&doc, id), Rect::new(1, 1, 4, 2));

        doc.undo();
        assert_eq!(*doc.sprite(), before);

        // And an image layer cannot be given reference properties.
        doc.transact("Props", |tx| {
            tx.update_layer(L1, |props| {
                props.reference = Some(crate::sprite::ReferenceProps {
                    opacity: 1,
                    rect: Rect::EMPTY,
                });
            })
        })
        .unwrap();
        assert!(doc.sprite().layer(L1).unwrap().as_reference().is_none());
    }

    #[test]
    fn a_reference_follows_the_canvas_and_survives_flattening() {
        let mut doc = document();
        paint(&mut doc, "Paint", Rect::new(0, 0, 2, 2), RED);
        let id = with_reference(&mut doc);
        let before = doc.sprite().clone();

        doc.transact("Resize", |tx| tx.resize_canvas(20, 20, 2, 3))
            .unwrap();
        assert_eq!(reference_rect(&doc, id), Rect::new(2, 7, 16, 8));
        doc.undo();
        assert_eq!(*doc.sprite(), before);

        doc.transact("Scale", |tx| tx.scale_sprite(32, 48)).unwrap();
        assert_eq!(reference_rect(&doc, id), Rect::new(0, 12, 32, 24));
        doc.undo();
        assert_eq!(*doc.sprite(), before);

        let copy = doc.transact("Copy", |tx| tx.duplicate_layer(id)).unwrap();
        assert_eq!(reference_rect(&doc, copy), reference_rect(&doc, id));

        let flat = doc.transact("Flatten", |tx| tx.flatten()).unwrap();
        let kinds: Vec<(LayerId, bool)> = doc
            .sprite()
            .layers()
            .iter()
            .map(|layer| (layer.id, layer.is_reference()))
            .collect();
        assert_eq!(kinds, [(flat, false), (id, true), (copy, true)]);
        assert_eq!(doc.active_layer(), flat);
    }

    #[test]
    fn pictures_are_fitted_inside_the_canvas() {
        let sprite = Sprite::new(8, 8).unwrap();
        assert_eq!(sprite.fitted(16, 8), Rect::new(0, 2, 8, 4));
        assert_eq!(sprite.fitted(4, 16), Rect::new(3, 0, 2, 8));
        assert_eq!(sprite.fitted(2, 2), Rect::new(0, 0, 8, 8));
        assert_eq!(sprite.fitted(1000, 1), Rect::new(0, 3, 8, 1));
    }

    /// A document with three frames, each holding a 1×1 red pixel at x = its frame number,
    /// and a tag over frames 1 to 2.
    fn animated() -> Document {
        let mut doc = document();
        doc.transact("Setup", |tx| {
            tx.add_frame(1, None)?;
            tx.add_frame(2, None)?;
            for frame in 0..3 {
                tx.set_pixels(
                    L1,
                    frame,
                    Rect::new(frame as i32, 0, 1, 1),
                    &filled(1, 1, RED),
                )?;
            }
            tx.set_frame_duration(1, 40)?;
            tx.add_tag(tag("Walk", 1, 2))?;
            Ok::<_, SpriteError>(())
        })
        .unwrap();
        doc
    }

    fn tag(name: &str, from: usize, to: usize) -> Tag {
        Tag {
            name: name.to_owned(),
            from,
            to,
            direction: crate::sprite::Direction::Forward,
            color: BLUE,
        }
    }

    /// Where the red pixel of each frame is: the frame it was first drawn in, or `None`.
    fn frame_marks(doc: &Document) -> Vec<Option<i32>> {
        (0..doc.sprite().frames().len())
            .map(|frame| doc.sprite().cel(L1, frame).map(|cel| cel.origin().x))
            .collect()
    }

    fn tag_spans(doc: &Document) -> Vec<(usize, usize)> {
        doc.sprite().tags().iter().map(|t| (t.from, t.to)).collect()
    }

    /// Runs `edit` as one transaction and checks that undo and redo restore the sprite
    /// exactly.
    fn undoes_exactly<T>(
        doc: &mut Document,
        edit: impl FnOnce(&mut Transaction<'_>) -> Result<T, SpriteError>,
    ) -> T {
        let before = doc.sprite().clone();
        let steps = doc.history_cursor();
        let value = doc.transact("Edit", edit).unwrap();
        assert_eq!(doc.history_cursor(), steps + 1, "not exactly one undo step");
        let after = doc.sprite().clone();
        assert!(doc.undo());
        assert_eq!(*doc.sprite(), before, "undo did not restore the sprite");
        assert!(doc.redo());
        assert_eq!(*doc.sprite(), after, "redo did not restore the change");
        value
    }

    #[test]
    fn frames_are_added_empty_or_as_a_copy() {
        let mut doc = animated();
        undoes_exactly(&mut doc, |tx| tx.add_frame(1, None));
        assert_eq!(frame_marks(&doc), [Some(0), None, Some(1), Some(2)]);
        assert_eq!(doc.sprite().frames()[1].duration_ms, DEFAULT_FRAME_MS);
        // Put before the tag, the frame pushes it along.
        assert_eq!(tag_spans(&doc), [(2, 3)]);

        // A copy takes the cels and the duration; inside a tag it makes the tag longer.
        undoes_exactly(&mut doc, |tx| tx.add_frame(3, Some(2)));
        assert_eq!(
            frame_marks(&doc),
            [Some(0), None, Some(1), Some(1), Some(2)]
        );
        assert_eq!(doc.sprite().frames()[3].duration_ms, 40);
        assert_eq!(tag_spans(&doc), [(2, 4)]);
        // The copy is a cel of its own.
        paint(&mut doc, "Paint", Rect::new(7, 7, 1, 1), BLUE);
        assert_eq!(doc.sprite().pixel(L1, 0, 7, 7), BLUE);
        doc.transact("Paint", |tx| {
            tx.set_pixels(L1, 3, Rect::new(9, 9, 1, 1), &filled(1, 1, BLUE))
        })
        .unwrap();
        assert_eq!(doc.sprite().pixel(L1, 2, 9, 9), Rgba8::TRANSPARENT);

        // At the very end, after the tag: the tag stays as it is.
        undoes_exactly(&mut doc, |tx| tx.add_frame(5, None));
        assert_eq!(tag_spans(&doc), [(2, 4)]);

        let bad = doc.transact("Add", |tx| tx.add_frame(9, None));
        assert_eq!(bad, Err(SpriteError::UnknownFrame(9)));
        let bad = doc.transact("Add", |tx| tx.add_frame(0, Some(9)));
        assert_eq!(bad, Err(SpriteError::UnknownFrame(9)));
    }

    #[test]
    fn frames_are_removed_with_their_cels_and_tags_follow() {
        let mut doc = animated();
        undoes_exactly(&mut doc, |tx| tx.remove_frame(0));
        assert_eq!(frame_marks(&doc), [Some(1), Some(2)]);
        assert_eq!(tag_spans(&doc), [(0, 1)]);
        assert_eq!(doc.sprite().frames()[0].duration_ms, 40);

        undoes_exactly(&mut doc, |tx| tx.remove_frame(1));
        assert_eq!(tag_spans(&doc), [(0, 0)]);
        assert_eq!(
            doc.transact("Remove", |tx| tx.remove_frame(0)),
            Err(SpriteError::LastFrame)
        );
        assert_eq!(
            doc.transact("Remove", |tx| tx.remove_frame(4)),
            Err(SpriteError::UnknownFrame(4))
        );

        // A tag that was only the removed frame goes with it.
        let mut doc = animated();
        doc.transact("Tag", |tx| tx.add_tag(tag("Blink", 0, 0)))
            .unwrap();
        undoes_exactly(&mut doc, |tx| tx.remove_frame(0));
        assert_eq!(tag_spans(&doc), [(0, 1)]);
    }

    #[test]
    fn the_active_frame_stays_on_a_frame_that_exists() {
        let mut doc = animated();
        assert!(doc.set_active_frame(2));
        assert!(!doc.set_active_frame(3));
        assert_eq!(doc.active_frame(), 2);
        // Showing another frame redraws everything, and is not an undo step.
        doc.take_damage();
        let steps = doc.history().len();
        doc.set_active_frame(1);
        assert_eq!(doc.take_damage(), doc.sprite().bounds());
        assert_eq!(doc.history().len(), steps);

        doc.set_active_frame(2);
        doc.transact("Remove", |tx| tx.remove_frame(2)).unwrap();
        assert_eq!(doc.active_frame(), 1);
        doc.undo();
        assert_eq!(doc.active_frame(), 1);
    }

    #[test]
    fn frames_move_with_their_cels() {
        let mut doc = animated();
        undoes_exactly(&mut doc, |tx| tx.move_frame(0, 2));
        assert_eq!(frame_marks(&doc), [Some(1), Some(2), Some(0)]);
        assert_eq!(doc.sprite().frames()[0].duration_ms, 40);
        undoes_exactly(&mut doc, |tx| tx.move_frame(2, 1));
        assert_eq!(frame_marks(&doc), [Some(1), Some(0), Some(2)]);
        // Tags name places on the time axis, not frames.
        assert_eq!(tag_spans(&doc), [(1, 2)]);

        let steps = doc.history().len();
        doc.transact("Move", |tx| tx.move_frame(1, 1)).unwrap();
        assert_eq!(doc.history().len(), steps);
        assert_eq!(
            doc.transact("Move", |tx| tx.move_frame(0, 3)),
            Err(SpriteError::UnknownFrame(3))
        );
    }

    #[test]
    fn durations_are_clamped() {
        let mut doc = animated();
        undoes_exactly(&mut doc, |tx| tx.set_frame_duration(0, 250));
        assert_eq!(doc.sprite().frames()[0].duration_ms, 250);
        doc.transact("Duration", |tx| tx.set_frame_duration(0, 0))
            .unwrap();
        assert_eq!(doc.sprite().frames()[0].duration_ms, 1);
        doc.transact("Duration", |tx| tx.set_frame_duration(0, u32::MAX))
            .unwrap();
        assert_eq!(doc.sprite().frames()[0].duration_ms, MAX_FRAME_MS);
    }

    #[test]
    fn cels_are_moved_and_copied_between_frames_and_layers() {
        let mut doc = animated();
        let upper = doc
            .transact("Add", |tx| tx.add_layer(Slot::new(None, 1), "Upper", false))
            .unwrap();

        undoes_exactly(&mut doc, |tx| tx.move_cel((L1, 0), (upper, 2), false));
        assert!(doc.sprite().cel(L1, 0).is_none());
        assert_eq!(doc.sprite().pixel(upper, 2, 0, 0), RED);

        // A copy leaves the source, and replaces what the target held.
        undoes_exactly(&mut doc, |tx| tx.move_cel((L1, 1), (L1, 2), true));
        assert_eq!(frame_marks(&doc), [None, Some(1), Some(1)]);

        let steps = doc.history().len();
        doc.transact("Move", |tx| tx.move_cel((L1, 1), (L1, 1), false))
            .unwrap();
        assert_eq!(doc.history().len(), steps);
        assert_eq!(
            doc.transact("Move", |tx| tx.move_cel((L1, 1), (L1, 7), false)),
            Err(SpriteError::UnknownFrame(7))
        );
        doc.transact("Lock", |tx| {
            tx.update_layer(upper, |props| {
                if let Some(image) = &mut props.image {
                    image.locked = true;
                }
            })
        })
        .unwrap();
        assert_eq!(
            doc.transact("Move", |tx| tx.move_cel((L1, 1), (upper, 0), true)),
            Err(SpriteError::LayerLocked(upper.0))
        );
    }

    #[test]
    fn linked_cels_change_together_and_undo_together() {
        let mut doc = animated();
        undoes_exactly(&mut doc, |tx| tx.link_cels(L1, 0, &[2]));
        assert_eq!(frame_marks(&doc), [Some(0), Some(1), Some(0)]);
        assert_eq!(doc.sprite().linked_frames(L1, 0), [0, 2]);
        assert_eq!(doc.sprite().linked_frames(L1, 2), [0, 2]);
        assert!(doc.sprite().linked_frames(L1, 1).is_empty());
        assert_eq!(doc.sprite().link_groups(), [(L1, vec![0, 2])]);

        // Drawing on either shows on both, and is still one undo step.
        let before = doc.sprite().clone();
        doc.transact("Paint", |tx| {
            tx.set_pixels(L1, 2, Rect::new(5, 5, 2, 2), &filled(2, 2, BLUE))
        })
        .unwrap();
        assert_eq!(doc.sprite().pixel(L1, 0, 6, 6), BLUE);
        assert_eq!(doc.sprite().pixel(L1, 1, 6, 6), Rgba8::TRANSPARENT);
        assert_eq!(doc.sprite().cel(L1, 0), doc.sprite().cel(L1, 2));
        doc.undo();
        assert_eq!(*doc.sprite(), before);
        doc.redo();

        // Erasing everything empties both; undo brings both back.
        let drawn = doc.sprite().clone();
        doc.transact("Erase", |tx| {
            tx.set_pixels(L1, 0, Rect::new(0, 0, 16, 16), &Image::new(16, 16))
        })
        .unwrap();
        assert!(doc.sprite().cel(L1, 0).is_none() && doc.sprite().cel(L1, 2).is_none());
        doc.undo();
        assert_eq!(*doc.sprite(), drawn);

        // Links follow their frames around.
        undoes_exactly(&mut doc, |tx| tx.add_frame(0, None));
        assert_eq!(doc.sprite().linked_frames(L1, 1), [1, 3]);
        undoes_exactly(&mut doc, |tx| tx.move_frame(3, 0));
        assert_eq!(doc.sprite().linked_frames(L1, 0), [0, 2]);
        // With one of the two gone, the other is an ordinary cel again.
        undoes_exactly(&mut doc, |tx| tx.remove_frame(0));
        assert!(doc.sprite().link_groups().is_empty());
    }

    #[test]
    fn cels_are_linked_in_groups_and_unlinked_one_by_one() {
        let mut doc = animated();
        doc.transact("Link", |tx| tx.link_cels(L1, 1, &[0, 2]))
            .unwrap();
        assert_eq!(frame_marks(&doc), [Some(1), Some(1), Some(1)]);
        assert_eq!(doc.sprite().linked_frames(L1, 0), [0, 1, 2]);

        undoes_exactly(&mut doc, |tx| tx.unlink_cel(L1, 0));
        assert_eq!(doc.sprite().linked_frames(L1, 1), [1, 2]);
        // It kept its pixels, and is on its own now.
        doc.transact("Paint", |tx| {
            tx.set_pixels(L1, 0, Rect::new(9, 9, 1, 1), &filled(1, 1, BLUE))
        })
        .unwrap();
        assert_eq!(frame_marks(&doc), [Some(1), Some(1), Some(1)]);
        assert_eq!(doc.sprite().pixel(L1, 1, 9, 9), Rgba8::TRANSPARENT);

        undoes_exactly(&mut doc, |tx| tx.unlink_cel(L1, 2));
        assert!(doc.sprite().link_groups().is_empty());
        // Unlinking what is not linked is nothing.
        let steps = doc.history().len();
        doc.transact("Unlink", |tx| tx.unlink_cel(L1, 2)).unwrap();
        assert_eq!(doc.history().len(), steps);

        // An empty cel can be linked, and joins in when the other is drawn on.
        let mut doc = animated();
        doc.transact("Add", |tx| tx.add_frame(3, None)).unwrap();
        doc.transact("Link", |tx| tx.link_cels(L1, 3, &[0]))
            .unwrap();
        assert_eq!(frame_marks(&doc), [None, Some(1), Some(2), None]);
        doc.transact("Paint", |tx| {
            tx.set_pixels(L1, 3, Rect::new(4, 0, 1, 1), &filled(1, 1, RED))
        })
        .unwrap();
        assert_eq!(frame_marks(&doc), [Some(4), Some(1), Some(2), Some(4)]);
    }

    #[test]
    fn moving_a_cel_onto_one_it_is_linked_with_keeps_both() {
        let mut doc = animated();
        doc.transact("Link", |tx| tx.link_cels(L1, 0, &[2]))
            .unwrap();
        doc.transact("Move", |tx| tx.move_cel((L1, 0), (L1, 2), false))
            .unwrap();
        assert_eq!(frame_marks(&doc), [Some(0), Some(1), Some(0)]);
    }

    #[test]
    fn tags_are_added_changed_and_removed() {
        let mut doc = animated();
        let index = undoes_exactly(&mut doc, |tx| tx.add_tag(tag("Idle", 0, 0)));
        assert_eq!(index, 1);
        let mut edited = tag("Run", 0, 2);
        edited.direction = crate::sprite::Direction::PingPong;
        undoes_exactly(&mut doc, |tx| tx.set_tag(1, edited.clone()));
        assert_eq!(doc.sprite().tags()[1], edited);
        undoes_exactly(&mut doc, |tx| tx.remove_tag(0));
        assert_eq!(doc.sprite().tags(), [edited]);

        for bad in [tag("Backwards", 2, 1), tag("Too far", 1, 3)] {
            let result = doc.transact("Tag", |tx| tx.add_tag(bad.clone()));
            assert_eq!(result, Err(SpriteError::InvalidTag));
            let result = doc.transact("Tag", |tx| tx.set_tag(0, bad.clone()));
            assert_eq!(result, Err(SpriteError::InvalidTag));
        }
        assert_eq!(
            doc.transact("Tag", |tx| tx.remove_tag(5)),
            Err(SpriteError::UnknownTag(5))
        );
        assert_eq!(
            doc.transact("Tag", |tx| tx.set_tag(5, tag("x", 0, 0))),
            Err(SpriteError::UnknownTag(5))
        );
    }

    #[test]
    fn layer_operations_reach_every_frame() {
        let mut doc = animated();
        let upper = doc
            .transact("Add", |tx| tx.add_layer(Slot::new(None, 1), "Upper", false))
            .unwrap();
        doc.transact("Paint", |tx| {
            tx.set_pixels(upper, 1, Rect::new(8, 8, 1, 1), &filled(1, 1, BLUE))
        })
        .unwrap();
        let pictures: Vec<Image> = (0..3)
            .map(|frame| compositor::compose(doc.sprite(), frame))
            .collect();

        undoes_exactly(&mut doc, |tx| tx.merge_down(upper));
        assert_eq!(doc.sprite().pixel(L1, 1, 8, 8), BLUE);
        doc.undo();
        let flat = undoes_exactly(&mut doc, |tx| tx.flatten());
        for (frame, picture) in pictures.iter().enumerate() {
            assert_eq!(compositor::compose(doc.sprite(), frame), *picture);
        }
        assert_eq!(doc.sprite().pixel(flat, 2, 2, 0), RED);
        doc.undo();

        // Deleting a layer and bringing it back restores its cels in every frame.
        undoes_exactly(&mut doc, |tx| tx.remove_layer(L1));
        let copy = undoes_exactly(&mut doc, |tx| tx.duplicate_layer(upper));
        assert_eq!(doc.sprite().pixel(copy, 1, 8, 8), BLUE);
    }
}

#[cfg(test)]
mod canvas_tests {
    use super::*;
    use crate::color::Rgba8;

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const L1: LayerId = LayerId(1);

    /// 8×8 with a red 2×2 block at (5, 5) and that block selected.
    fn document() -> Document {
        let mut doc = Document::new(Sprite::new(8, 8).unwrap());
        let block = Image::from_pixels(2, 2, vec![RED; 4]).unwrap();
        doc.transact("Paint", |tx| {
            tx.set_pixels(L1, 0, Rect::new(5, 5, 2, 2), &block)?;
            tx.set_selection(Some(Mask::from_rect(8, 8, Rect::new(5, 5, 2, 2))))
        })
        .unwrap();
        doc
    }

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
    fn selection_changes_are_undo_steps_and_must_fit_the_canvas() {
        let mut doc = document();
        step(&mut doc, |tx| tx.set_selection(None));
        assert!(doc.sprite().selection().is_none());
        assert!(doc.sprite().is_selected(0, 0));

        // An empty mask is the same as no selection, and so not a change here.
        let steps = doc.history().len();
        doc.transact("Empty", |tx| tx.set_selection(Some(Mask::empty(8, 8))))
            .unwrap();
        assert_eq!(doc.history().len(), steps);

        let wrong = doc.transact("Wrong", |tx| tx.set_selection(Some(Mask::full(4, 4))));
        assert!(matches!(wrong, Err(SpriteError::SizeMismatch { .. })));

        step(&mut doc, |tx| {
            tx.set_selection(Some(Mask::from_rect(8, 8, Rect::new(0, 0, 2, 2))))
        });
        assert!(doc.sprite().is_selected(1, 1) && !doc.sprite().is_selected(5, 5));
    }

    #[test]
    fn resizing_the_canvas_shifts_and_clips_and_drops_the_selection() {
        let mut doc = document();
        // Grow to the left and top by 4: everything moves 4 right and down.
        step(&mut doc, |tx| tx.resize_canvas(12, 12, 4, 4));
        assert_eq!(doc.sprite().bounds(), Rect::from_size(12, 12));
        assert_eq!(
            doc.sprite().cel(L1, 0).unwrap().rect(),
            Rect::new(9, 9, 2, 2)
        );
        assert!(doc.sprite().selection().is_none());
        assert_eq!(doc.take_damage(), Rect::from_size(12, 12));

        // Shrink so that only one column of the block is left.
        step(&mut doc, |tx| tx.resize_canvas(10, 12, 0, 0));
        assert_eq!(
            doc.sprite().cel(L1, 0).unwrap().rect(),
            Rect::new(9, 9, 1, 2)
        );

        // Undoing all the way brings back the cut-off pixels and the selection.
        while doc.undo() {}
        doc.jump_to(1);
        assert_eq!(
            doc.sprite().cel(L1, 0).unwrap().rect(),
            Rect::new(5, 5, 2, 2)
        );
        assert!(doc.sprite().selection().is_some());

        let bad = doc.transact("Bad", |tx| tx.resize_canvas(0, 5, 0, 0));
        assert_eq!(bad, Err(SpriteError::InvalidSize(0, 5)));
        // The same size and no shift is not a change.
        doc.transact("Deselect", |tx| tx.set_selection(None))
            .unwrap();
        let steps = doc.history().len();
        doc.transact("Same", |tx| tx.resize_canvas(8, 8, 0, 0))
            .unwrap();
        assert_eq!(doc.history().len(), steps);
    }

    #[test]
    fn cropping_to_the_selection_is_a_resize_to_its_bounds() {
        let mut doc = document();
        let bounds = doc.sprite().selection().unwrap().bounds();
        step(&mut doc, |tx| {
            tx.resize_canvas(bounds.w, bounds.h, -bounds.x, -bounds.y)
        });
        assert_eq!(doc.sprite().bounds(), Rect::from_size(2, 2));
        assert_eq!(
            doc.sprite().cel(L1, 0).unwrap().rect(),
            Rect::from_size(2, 2)
        );
    }

    #[test]
    fn scaling_the_sprite_scales_positions_too() {
        let mut doc = document();
        step(&mut doc, |tx| tx.scale_sprite(16, 16));
        assert_eq!(doc.sprite().bounds(), Rect::from_size(16, 16));
        assert_eq!(
            doc.sprite().cel(L1, 0).unwrap().rect(),
            Rect::new(10, 10, 4, 4)
        );

        step(&mut doc, |tx| tx.scale_sprite(8, 4));
        assert_eq!(
            doc.sprite().cel(L1, 0).unwrap().rect(),
            Rect::new(5, 3, 2, 1)
        );
        let bad = doc.transact("Bad", |tx| tx.scale_sprite(5000, 5));
        assert_eq!(bad, Err(SpriteError::InvalidSize(5000, 5)));
    }
}
