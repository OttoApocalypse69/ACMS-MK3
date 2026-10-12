//! Playing an animation: which frame is shown as time passes.

use crate::sprite::{Direction, Sprite};

/// The frames that are played and how: a tag's, or all of them forwards.
fn span(sprite: &Sprite, tag: Option<usize>) -> (usize, usize, Direction) {
    let last = sprite.frames().len() - 1;
    match tag.and_then(|index| sprite.tags().get(index)) {
        Some(tag) => (tag.from.min(last), tag.to.min(last), tag.direction),
        None => (0, last, Direction::Forward),
    }
}

/// The frames of one pass through the sprite or a tag, in the order they are shown. A
/// ping-pong tag goes there and back without repeating its end frames, so that the list
/// loops cleanly.
pub fn sequence(sprite: &Sprite, tag: Option<usize>) -> Vec<usize> {
    let (from, to, direction) = span(sprite, tag);
    match direction {
        Direction::Forward => (from..=to).collect(),
        Direction::Reverse => (from..=to).rev().collect(),
        Direction::PingPong => (from..=to).chain((from + 1..to).rev()).collect(),
    }
}

/// Where a running animation is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Playback {
    /// The frame being shown.
    pub frame: usize,
    /// Whether a ping-pong animation is on its way back.
    returning: bool,
    /// How long the frame has been shown, in milliseconds.
    shown_ms: u32,
}

impl Playback {
    /// Starts playing at `frame`.
    pub fn starting_at(frame: usize) -> Self {
        Self {
            frame,
            returning: false,
            shown_ms: 0,
        }
    }

    /// The frame after the current one. A frame outside the span (the user played a tag
    /// from somewhere else) jumps to the start of it.
    fn step(&mut self, from: usize, to: usize, direction: Direction) {
        if self.frame < from || self.frame > to {
            self.returning = false;
            self.frame = if direction == Direction::Reverse {
                to
            } else {
                from
            };
            return;
        }
        if from == to {
            return;
        }
        self.frame = match direction {
            Direction::Forward if self.frame == to => from,
            Direction::Forward => self.frame + 1,
            Direction::Reverse if self.frame == from => to,
            Direction::Reverse => self.frame - 1,
            Direction::PingPong => {
                if self.frame == to {
                    self.returning = true;
                } else if self.frame == from {
                    self.returning = false;
                }
                if self.returning {
                    self.frame - 1
                } else {
                    self.frame + 1
                }
            }
        };
    }

    /// Lets `elapsed_ms` milliseconds pass. Returns whether another frame is shown now.
    /// `tag` is the tag being looped, or `None` for the whole sprite.
    pub fn advance(&mut self, sprite: &Sprite, tag: Option<usize>, elapsed_ms: u32) -> bool {
        let (from, to, direction) = span(sprite, tag);
        let frames = sprite.frames();
        let before = self.frame;
        self.frame = self.frame.min(frames.len() - 1);
        self.shown_ms = self.shown_ms.saturating_add(elapsed_ms);

        // After a long pause (the window was hidden, say) there is no point in replaying
        // every loop that was missed: only where in the loop the time lands matters.
        let cycle: u32 = match direction {
            Direction::PingPong if to > from => {
                let all: u32 = frames[from..=to].iter().map(|f| f.duration_ms).sum();
                2 * all - frames[from].duration_ms - frames[to].duration_ms
            }
            _ => frames[from..=to].iter().map(|f| f.duration_ms).sum(),
        };
        if cycle > 0 && self.shown_ms > 2 * cycle {
            self.shown_ms %= cycle;
        }

        loop {
            let duration = frames[self.frame].duration_ms.max(1);
            if self.shown_ms < duration {
                break;
            }
            self.shown_ms -= duration;
            self.step(from, to, direction);
        }
        self.frame != before
    }

    /// How long until the next frame is due, in milliseconds.
    pub fn until_next(&self, sprite: &Sprite) -> u32 {
        let frame = self.frame.min(sprite.frames().len() - 1);
        let duration = sprite.frames()[frame].duration_ms.max(1);
        duration.saturating_sub(self.shown_ms).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;
    use crate::document::Document;
    use crate::sprite::Tag;

    /// Five frames of 100 ms, except the third, which takes 250 ms.
    fn sprite(tag: Option<(usize, usize, Direction)>) -> Sprite {
        let mut document = Document::new(Sprite::new(4, 4).unwrap());
        document
            .transact("Frames", |tx| {
                for _ in 0..4 {
                    tx.add_frame(1, None)?;
                }
                tx.set_frame_duration(2, 250)?;
                if let Some((from, to, direction)) = tag {
                    tx.add_tag(Tag {
                        name: "Run".to_owned(),
                        from,
                        to,
                        direction,
                        color: Rgba8::BLACK,
                    })?;
                }
                Ok::<_, crate::sprite::SpriteError>(())
            })
            .unwrap();
        document.sprite().clone()
    }

    /// The frames shown when time passes in steps of 50 ms.
    fn shown(sprite: &Sprite, tag: Option<usize>, start: usize, steps: usize) -> Vec<usize> {
        let mut playback = Playback::starting_at(start);
        let mut frames = vec![playback.frame];
        for _ in 0..steps {
            if playback.advance(sprite, tag, 50) {
                frames.push(playback.frame);
            }
        }
        frames
    }

    #[test]
    fn the_whole_sprite_loops_forwards_and_respects_durations() {
        let sprite = sprite(None);
        assert_eq!(
            shown(&sprite, None, 0, 26),
            [0, 1, 2, 3, 4, 0, 1, 2, 3, 4, 0]
        );

        let mut playback = Playback::starting_at(1);
        assert!(!playback.advance(&sprite, None, 99));
        assert_eq!(playback.until_next(&sprite), 1);
        assert!(playback.advance(&sprite, None, 1));
        assert_eq!(playback.frame, 2);
        // The long frame stays for its 250 ms.
        assert!(!playback.advance(&sprite, None, 249));
        assert!(playback.advance(&sprite, None, 1));
        assert_eq!(playback.frame, 3);
    }

    #[test]
    fn one_pass_lists_the_frames_in_playing_order() {
        assert_eq!(sequence(&sprite(None), None), [0, 1, 2, 3, 4]);
        let tagged = |direction| sequence(&sprite(Some((1, 3, direction))), Some(0));
        assert_eq!(tagged(Direction::Forward), [1, 2, 3]);
        assert_eq!(tagged(Direction::Reverse), [3, 2, 1]);
        assert_eq!(tagged(Direction::PingPong), [1, 2, 3, 2]);
        let short = sequence(&sprite(Some((1, 2, Direction::PingPong))), Some(0));
        assert_eq!(short, [1, 2]);
        let single = sequence(&sprite(Some((4, 4, Direction::PingPong))), Some(0));
        assert_eq!(single, [4]);
    }

    #[test]
    fn one_big_step_passes_several_frames() {
        let sprite = sprite(None);
        let mut playback = Playback::starting_at(0);
        assert!(playback.advance(&sprite, None, 460));
        assert_eq!(playback.frame, 3);
        // A pause far longer than the animation does not take long to work out.
        assert!(!playback.advance(&sprite, None, 650 * 1_000_000));
        let _ = playback.until_next(&sprite);
    }

    #[test]
    fn a_tag_is_played_in_its_direction() {
        let forward = sprite(Some((1, 3, Direction::Forward)));
        assert_eq!(shown(&forward, Some(0), 1, 24), [1, 2, 3, 1, 2, 3, 1, 2]);
        // Started from outside the tag, it jumps in.
        assert_eq!(shown(&forward, Some(0), 4, 4)[..2], [4, 1]);

        let reverse = sprite(Some((1, 3, Direction::Reverse)));
        assert_eq!(shown(&reverse, Some(0), 3, 16), [3, 2, 1, 3, 2, 1]);
        assert_eq!(shown(&reverse, Some(0), 0, 4)[..2], [0, 3]);

        let bounce = sprite(Some((1, 3, Direction::PingPong)));
        assert_eq!(
            shown(&bounce, Some(0), 1, 30),
            [1, 2, 3, 2, 1, 2, 3, 2, 1, 2]
        );

        // A tag of one frame stays on it; a tag that is not there plays everything.
        let single = sprite(Some((2, 2, Direction::PingPong)));
        assert_eq!(shown(&single, Some(0), 2, 20), [2]);
        assert_eq!(shown(&single, Some(7), 3, 4), [3, 4, 0]);
    }
}
