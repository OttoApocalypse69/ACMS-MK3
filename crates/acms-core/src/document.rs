//! A sprite together with its undo history (see `PLAN.md`, section 4.3).
//!
//! The only way to change the sprite is [`Document::transact`]. Everything done inside one
//! call becomes a single undo step.

use std::collections::VecDeque;

use crate::geom::Rect;
use crate::image::Image;
use crate::sprite::{LayerId, Sprite, SpriteError};

/// Default cap on the memory the undo history may hold.
pub const DEFAULT_HISTORY_BYTES: usize = 256 * 1024 * 1024;

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
        }
    }

    /// The canvas area this op changes.
    fn rect(&self) -> Rect {
        match self {
            Self::Pixels { rect, .. } => *rect,
        }
    }

    fn byte_size(&self) -> usize {
        match self {
            Self::Pixels { before, after, .. } => before.byte_size() + after.byte_size(),
        }
    }
}

/// Collects the changes made inside [`Document::transact`].
pub struct Transaction<'a> {
    sprite: &'a mut Sprite,
    ops: Vec<Op>,
}

impl Transaction<'_> {
    /// The sprite as it is right now, including the changes made so far in this transaction.
    pub fn sprite(&self) -> &Sprite {
        self.sprite
    }

    /// Replaces the pixels of the canvas rectangle `rect` on one cel with `pixels`, which must
    /// be exactly `rect`-sized. Parts outside the canvas are ignored.
    ///
    /// Only the area that really changes is written and remembered for undo.
    pub fn set_pixels(
        &mut self,
        layer: LayerId,
        frame: usize,
        rect: Rect,
        pixels: &Image,
    ) -> Result<(), SpriteError> {
        self.sprite.check_cel(layer, frame)?;
        if (rect.w, rect.h) != (pixels.width(), pixels.height()) {
            return Err(SpriteError::SizeMismatch {
                want_w: rect.w,
                want_h: rect.h,
                got_w: pixels.width(),
                got_h: pixels.height(),
            });
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

        let op = Op::Pixels {
            layer,
            frame,
            rect: changed.translated(target.x, target.y),
            before: before.crop(changed),
            after: after.crop(changed),
        };
        op.apply(self.sprite);
        self.ops.push(op);
        Ok(())
    }
}

struct Entry {
    /// The document's revision once this entry is applied.
    revision: u64,
    name: String,
    ops: Vec<Op>,
}

impl Entry {
    fn byte_size(&self) -> usize {
        self.ops.iter().map(Op::byte_size).sum()
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
    /// Canvas area changed since the last [`Document::take_damage`].
    damage: Rect,
    active_layer: LayerId,
    active_frame: usize,
}

impl Document {
    pub fn new(sprite: Sprite) -> Self {
        // `Sprite` guarantees at least one layer.
        let active_layer = sprite.layers().last().map_or(LayerId(0), |layer| layer.id);
        Self {
            sprite,
            entries: VecDeque::new(),
            cursor: 0,
            base_revision: 0,
            last_revision: 0,
            history_limit: DEFAULT_HISTORY_BYTES,
            damage: Rect::EMPTY,
            active_layer,
            active_frame: 0,
        }
    }

    pub fn sprite(&self) -> &Sprite {
        &self.sprite
    }

    /// The layer tools draw on.
    pub fn active_layer(&self) -> LayerId {
        self.active_layer
    }

    /// The frame being shown and edited.
    pub fn active_frame(&self) -> usize {
        self.active_frame
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
        let mut transaction = Transaction {
            sprite: &mut self.sprite,
            ops: Vec::new(),
        };
        let result = edit(&mut transaction);
        let ops = transaction.ops;

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
            self.damage = self.damage.union(op.rect());
        }
        // A new change discards whatever could have been redone.
        self.entries.truncate(self.cursor);
        self.last_revision += 1;
        self.entries.push_back(Entry {
            revision: self.last_revision,
            name: name.to_owned(),
            ops,
        });
        self.cursor = self.entries.len();
        self.enforce_history_limit();
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
            op.revert(&mut self.sprite);
            self.damage = self.damage.union(op.rect());
        }
        true
    }

    /// Re-applies the step that was undone last. Returns `false` if there was none.
    pub fn redo(&mut self) -> bool {
        if !self.can_redo() {
            return false;
        }
        for op in &self.entries[self.cursor].ops {
            op.apply(&mut self.sprite);
            self.damage = self.damage.union(op.rect());
        }
        self.cursor += 1;
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

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const BLUE: Rgba8 = Rgba8::rgb(0, 0, 255);
    const L1: LayerId = LayerId(1);

    fn filled(w: u32, h: u32, color: Rgba8) -> Image {
        Image::from_pixels(w, h, vec![color; (w * h) as usize]).unwrap()
    }

    fn document() -> Document {
        Document::new(Sprite::new(16, 16).unwrap())
    }

    fn paint(doc: &mut Document, name: &str, rect: Rect, color: Rgba8) {
        doc.transact(name, |tx| {
            tx.set_pixels(L1, 0, rect, &filled(rect.w, rect.h, color))
        })
        .unwrap();
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

        // Repainting everything red again changes a single pixel's worth: nothing.
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
        let before = doc.sprite().clone();
        let result = doc.transact("Broken", |tx| {
            tx.set_pixels(L1, 0, Rect::new(1, 1, 1, 1), &filled(1, 1, RED))?;
            tx.set_pixels(LayerId(99), 0, Rect::new(1, 1, 1, 1), &filled(1, 1, RED))
        });
        assert_eq!(result, Err(SpriteError::UnknownLayer(99)));
        assert_eq!(*doc.sprite(), before);
        assert_eq!(doc.history().len(), 0);
        assert!(!doc.can_undo());
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
}
