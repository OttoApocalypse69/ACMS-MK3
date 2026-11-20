//! Flappy Pencil: a hidden minigame. The Konami code (↑ ↑ ↓ ↓ ← → ← → B A) opens it.
//!
//! The game itself is plain arithmetic on a small world of game pixels, stepped at a fixed
//! rate so it plays the same at any frame rate; [`show`] draws it, enlarged by a whole
//! number so it stays crisp. If a small sprite is open, the bird is that sprite.

use acms_core::color::Rgba8;
use acms_core::image::Image;
use egui::{Color32, Key, Pos2, Rect, Sense, Vec2};

use crate::state::AppState;

/// The world, in game pixels.
pub const WIDTH: f32 = 144.0;
pub const HEIGHT: f32 = 256.0;
/// The height of the ground strip at the bottom.
const GROUND: f32 = 24.0;
/// Seconds per simulation step.
const STEP: f32 = 1.0 / 120.0;
/// Downward pull, in game pixels per second per second.
const GRAVITY: f32 = 640.0;
/// The speed a flap gives, upwards.
const FLAP: f32 = 210.0;
/// Fastest fall.
const MAX_FALL: f32 = 300.0;
/// How fast the pipes come, in game pixels per second.
const SPEED: f32 = 52.0;
const PIPE_WIDTH: f32 = 24.0;
/// The opening to fly through.
const GAP: f32 = 62.0;
/// From one pipe to the next.
const SPACING: f32 = 86.0;
/// Where the bird flies, from the left.
const BIRD_X: f32 = 40.0;
/// The bird's hit box: a little smaller than it looks, to be fair.
const BIRD_SIZE: f32 = 12.0;
const HIT_INSET: f32 = 2.0;

/// The keys of the Konami code, in order.
pub const KONAMI: [Key; 10] = [
    Key::ArrowUp,
    Key::ArrowUp,
    Key::ArrowDown,
    Key::ArrowDown,
    Key::ArrowLeft,
    Key::ArrowRight,
    Key::ArrowLeft,
    Key::ArrowRight,
    Key::B,
    Key::A,
];

/// Watches key presses for the Konami code.
#[derive(Default)]
pub struct Konami {
    /// The last keys pressed, at most as many as the code is long.
    recent: Vec<Key>,
}

impl Konami {
    /// Notes a key press. Returns `true` when the last keys pressed are the code.
    pub fn press(&mut self, key: Key) -> bool {
        self.recent.push(key);
        if self.recent.len() > KONAMI.len() {
            self.recent.remove(0);
        }
        if self.recent == KONAMI {
            self.recent.clear();
            return true;
        }
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the first flap.
    Ready,
    Playing,
    Over,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pipe {
    /// The left edge.
    pub x: f32,
    /// The top of the opening.
    pub gap_top: f32,
    /// The bird has passed it and it has been counted.
    pub passed: bool,
}

/// A game in progress.
#[derive(Clone, Debug)]
pub struct Game {
    pub phase: Phase,
    /// The top of the bird.
    pub y: f32,
    /// Upwards is negative.
    pub velocity: f32,
    pub pipes: Vec<Pipe>,
    pub score: u32,
    pub best: u32,
    /// How far the ground has scrolled, for its stripes.
    pub scroll: f32,
    /// Time not yet stepped.
    pending: f32,
    /// Seconds since the game was shown, for the bird bobbing while it waits.
    pub clock: f32,
    seed: u32,
    /// The picture of the bird, if it is the open sprite.
    pub bird: Option<Image>,
}

impl Game {
    pub fn new(seed: u32, bird: Option<Image>) -> Self {
        let mut game = Self {
            phase: Phase::Ready,
            y: 0.0,
            velocity: 0.0,
            pipes: Vec::new(),
            score: 0,
            best: 0,
            scroll: 0.0,
            pending: 0.0,
            clock: 0.0,
            seed: seed.max(1),
            bird,
        };
        game.reset();
        game
    }

    /// Back to the start, keeping the best score.
    pub fn reset(&mut self) {
        self.phase = Phase::Ready;
        self.y = (HEIGHT - GROUND) / 2.0 - BIRD_SIZE;
        self.velocity = 0.0;
        self.pipes.clear();
        self.score = 0;
        self.pending = 0.0;
    }

    /// A number from 0 to 1, from a small generator of its own (xorshift).
    fn random(&mut self) -> f32 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        (x % 10_000) as f32 / 10_000.0
    }

    fn new_pipe(&mut self, x: f32) -> Pipe {
        let lowest = HEIGHT - GROUND - GAP - 16.0;
        Pipe {
            x,
            gap_top: 20.0 + self.random() * (lowest - 20.0),
            passed: false,
        }
    }

    /// A press of Space, Up or the mouse: flap, start, or start again.
    pub fn flap(&mut self) {
        match self.phase {
            Phase::Ready => {
                self.phase = Phase::Playing;
                let first = self.new_pipe(WIDTH + 30.0);
                self.pipes.push(first);
                self.velocity = -FLAP;
            }
            Phase::Playing => self.velocity = -FLAP,
            Phase::Over => self.reset(),
        }
    }

    /// Moves the game on by `seconds`, in fixed steps.
    pub fn advance(&mut self, seconds: f32) {
        // A long pause (the window was hidden) is not a long fall.
        self.pending += seconds.min(0.25);
        self.clock += seconds.min(0.25);
        while self.pending >= STEP {
            self.pending -= STEP;
            self.step();
        }
    }

    fn step(&mut self) {
        if self.phase != Phase::Over {
            self.scroll = (self.scroll + SPEED * STEP) % 8.0;
        }
        if self.phase != Phase::Playing {
            return;
        }
        self.velocity = (self.velocity + GRAVITY * STEP).min(MAX_FALL);
        self.y += self.velocity * STEP;
        // The ceiling stops the bird; it does not end the game.
        if self.y < 0.0 {
            self.y = 0.0;
            self.velocity = 0.0;
        }

        for pipe in &mut self.pipes {
            pipe.x -= SPEED * STEP;
        }
        self.pipes.retain(|pipe| pipe.x + PIPE_WIDTH > 0.0);
        if self
            .pipes
            .last()
            .is_none_or(|pipe| pipe.x <= WIDTH - SPACING)
        {
            let x = self.pipes.last().map_or(WIDTH, |pipe| pipe.x + SPACING);
            let pipe = self.new_pipe(x);
            self.pipes.push(pipe);
        }
        for pipe in &mut self.pipes {
            if !pipe.passed && pipe.x + PIPE_WIDTH / 2.0 < BIRD_X {
                pipe.passed = true;
                self.score += 1;
            }
        }

        if self.hits_something() {
            self.phase = Phase::Over;
            self.best = self.best.max(self.score);
        }
    }

    /// The bird's hit box.
    pub fn bird_box(&self) -> (f32, f32, f32, f32) {
        let left = BIRD_X + HIT_INSET;
        let top = self.y + HIT_INSET;
        let size = BIRD_SIZE - 2.0 * HIT_INSET;
        (left, top, left + size, top + size)
    }

    fn hits_something(&self) -> bool {
        let (left, top, right, bottom) = self.bird_box();
        if bottom >= HEIGHT - GROUND {
            return true;
        }
        self.pipes.iter().any(|pipe| {
            let across = right > pipe.x && left < pipe.x + PIPE_WIDTH;
            let outside_gap = top < pipe.gap_top || bottom > pipe.gap_top + GAP;
            across && outside_gap
        })
    }
}

/// The bird drawn when no small sprite is open: 12 × 12, one letter per pixel.
const BIRD: [&str; 12] = [
    "....OOOO....",
    "..OOYYYYOO..",
    ".OYYYYYOWWO.",
    ".OYYYYYOWKO.",
    "OYYYYYYOWWO.",
    "OWWWWYYYOOO.",
    "OWWWWWYOrrrO",
    ".OWWWYYOrrO.",
    "..OYYYYYOO..",
    "...OOOOOO...",
    "............",
    "............",
];

fn bird_color(letter: char) -> Option<Color32> {
    Some(match letter {
        'O' => Color32::from_rgb(0x14, 0x15, 0x1A),
        'Y' => Color32::from_rgb(0xF6, 0xC9, 0x4E),
        'W' => Color32::from_rgb(0xFF, 0xFF, 0xFF),
        'K' => Color32::from_rgb(0x14, 0x15, 0x1A),
        'r' => Color32::from_rgb(0xE5, 0x53, 0x4B),
        _ => return None,
    })
}

/// The open sprite, if it is small enough to fly: its frame on screen, flattened.
pub fn sprite_bird(state: &AppState) -> Option<Image> {
    let document = state.editor.document()?;
    let sprite = document.sprite();
    if sprite.width() > 64 || sprite.height() > 64 {
        return None;
    }
    Some(acms_core::compositor::compose(
        sprite,
        document.active_frame(),
    ))
}

/// What the player did this frame.
pub enum Action {
    None,
    Close,
}

/// Draws the game and handles its input. Called every frame while it is open.
pub fn show(ctx: &egui::Context, game: &mut Game, best_label: &str) -> Action {
    let dt = ctx.input(|input| input.stable_dt).min(0.25);
    game.advance(dt);
    ctx.request_repaint();

    let mut action = Action::None;
    let modal = egui::Modal::new(egui::Id::new("flappy")).show(ctx, |ui| {
        ui.heading("Flappy Pencil");
        // As large as fits, by a whole number of screen pixels per game pixel.
        let room = ctx.content_rect().size() - Vec2::new(80.0, 160.0);
        let scale = (room.x / WIDTH)
            .min(room.y / HEIGHT)
            .floor()
            .clamp(1.0, 4.0);
        let size = Vec2::new(WIDTH, HEIGHT) * scale;
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        draw(ui.painter_at(rect), rect.min, scale, game);
        // The window is as wide as the game: the help gets a line of its own.
        ui.set_max_width(size.x);
        ui.horizontal(|ui| {
            ui.label(format!("Score {}", game.score));
            ui.separator();
            ui.label(format!("{best_label} {}", game.best));
        });
        ui.label(egui::RichText::new("Space or click to flap · Escape to close").weak());

        let flapped = response.clicked()
            || ui.input(|input| input.key_pressed(Key::Space) || input.key_pressed(Key::ArrowUp));
        if flapped {
            game.flap();
        }
    });
    if modal.should_close() {
        action = Action::Close;
    }
    action
}

fn draw(painter: egui::Painter, origin: Pos2, scale: f32, game: &Game) {
    let at = |x: f32, y: f32| origin + Vec2::new(x, y) * scale;
    let block =
        |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(at(x, y), Vec2::new(w, h) * scale);

    // Sky, with a horizon of pale hills.
    painter.rect_filled(
        block(0.0, 0.0, WIDTH, HEIGHT),
        0.0,
        Color32::from_rgb(0x8F, 0xD3, 0xE8),
    );
    painter.rect_filled(
        block(0.0, HEIGHT - GROUND - 18.0, WIDTH, 18.0),
        0.0,
        Color32::from_rgb(0xB6, 0xE3, 0xC2),
    );

    // Pipes: green with a dark edge and a lip at the opening.
    let pipe_color = Color32::from_rgb(0x5F, 0xC2, 0x6B);
    let edge = Color32::from_rgb(0x2E, 0x6B, 0x36);
    for pipe in &game.pipes {
        let bottom_top = pipe.gap_top + GAP;
        for (y, h) in [
            (0.0, pipe.gap_top),
            (bottom_top, HEIGHT - GROUND - bottom_top),
        ] {
            painter.rect_filled(block(pipe.x, y, PIPE_WIDTH, h), 0.0, edge);
            painter.rect_filled(block(pipe.x + 1.0, y, PIPE_WIDTH - 2.0, h), 0.0, pipe_color);
        }
        for y in [pipe.gap_top - 6.0, bottom_top] {
            painter.rect_filled(block(pipe.x - 2.0, y, PIPE_WIDTH + 4.0, 6.0), 0.0, edge);
            painter.rect_filled(
                block(pipe.x - 1.0, y + 1.0, PIPE_WIDTH + 2.0, 4.0),
                0.0,
                pipe_color,
            );
        }
    }

    // Ground, with stripes that scroll.
    let ground = HEIGHT - GROUND;
    painter.rect_filled(
        block(0.0, ground, WIDTH, GROUND),
        0.0,
        Color32::from_rgb(0xDE, 0xD8, 0x95),
    );
    painter.rect_filled(
        block(0.0, ground, WIDTH, 4.0),
        0.0,
        Color32::from_rgb(0x7C, 0xB3, 0x42),
    );
    let mut x = -game.scroll;
    while x < WIDTH {
        painter.rect_filled(
            block(x.max(0.0), ground + 1.0, 4.0, 2.0),
            0.0,
            Color32::from_rgb(0x5F, 0x95, 0x2E),
        );
        x += 8.0;
    }

    // The bird: bobbing while it waits, tilting with its speed while it flies.
    let bob = if game.phase == Phase::Ready {
        (game.clock * 4.0).sin() * 3.0
    } else {
        0.0
    };
    draw_bird(&painter, at(BIRD_X, game.y + bob), scale, game);

    // Big messages, drawn in the middle.
    let message = match game.phase {
        Phase::Ready => Some("Press Space"),
        Phase::Over => Some("Game over"),
        Phase::Playing => None,
    };
    let font = egui::FontId::proportional(10.0 * scale);
    if let Some(message) = message {
        let centre = at(WIDTH / 2.0, HEIGHT / 3.0);
        painter.text(
            centre + Vec2::splat(scale),
            egui::Align2::CENTER_CENTER,
            message,
            font.clone(),
            Color32::from_black_alpha(160),
        );
        painter.text(
            centre,
            egui::Align2::CENTER_CENTER,
            message,
            font.clone(),
            Color32::WHITE,
        );
    }
    if game.phase != Phase::Ready {
        let top = at(WIDTH / 2.0, 14.0);
        let score = game.score.to_string();
        painter.text(
            top + Vec2::splat(scale),
            egui::Align2::CENTER_CENTER,
            &score,
            font.clone(),
            Color32::from_black_alpha(160),
        );
        painter.text(
            top,
            egui::Align2::CENTER_CENTER,
            &score,
            font,
            Color32::WHITE,
        );
    }
}

fn draw_bird(painter: &egui::Painter, top_left: Pos2, scale: f32, game: &Game) {
    let pixel = |x: f32, y: f32, size: f32, color: Color32| {
        let rect = Rect::from_min_size(
            top_left + Vec2::new(x, y) * scale,
            Vec2::splat(size * scale),
        );
        painter.rect_filled(rect, 0.0, color);
    };
    match &game.bird {
        // The open sprite, fitted into the bird's square.
        Some(image) => {
            let side = image.width().max(image.height()) as f32;
            let size = BIRD_SIZE / side;
            for y in 0..image.height() as i32 {
                for x in 0..image.width() as i32 {
                    let Rgba8 { r, g, b, a } = image.get(x, y);
                    if a > 0 {
                        let color = Color32::from_rgba_unmultiplied(r, g, b, a);
                        pixel(x as f32 * size, y as f32 * size, size, color);
                    }
                }
            }
        }
        None => {
            // The wing flaps up for a moment after each flap.
            let wing_up = game.velocity < -60.0;
            for (y, row) in BIRD.iter().enumerate() {
                for (x, letter) in row.chars().enumerate() {
                    let letter = if wing_up && y == 5 && (1..=4).contains(&x) {
                        'Y'
                    } else {
                        letter
                    };
                    if let Some(color) = bird_color(letter) {
                        pixel(x as f32, y as f32, 1.0, color);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(game: &mut Game, seconds: f32) {
        let mut left = seconds;
        while left > 0.0 {
            game.advance(left.min(0.1));
            left -= 0.1;
        }
    }

    #[test]
    fn the_bird_waits_then_flies_falls_and_lands() {
        let mut game = Game::new(7, None);
        let start = game.y;
        run(&mut game, 2.0);
        // Nothing happens before the first flap.
        assert_eq!((game.phase, game.y), (Phase::Ready, start));

        game.flap();
        assert_eq!(game.phase, Phase::Playing);
        run(&mut game, 0.1);
        assert!(game.y < start, "a flap lifts the bird");
        run(&mut game, 3.0);
        // Without flapping it falls onto the ground, and the game is over.
        assert_eq!(game.phase, Phase::Over);
        let (_, _, _, bottom) = game.bird_box();
        assert!(bottom >= HEIGHT - GROUND - 6.0);

        // A press starts again, keeping the best score.
        game.best = 5;
        game.flap();
        assert_eq!((game.phase, game.score, game.best), (Phase::Ready, 0, 5));
        assert!(game.pipes.is_empty());
    }

    #[test]
    fn the_ceiling_stops_the_bird_without_ending_the_game() {
        let mut game = Game::new(7, None);
        game.flap();
        for _ in 0..40 {
            game.flap();
            run(&mut game, 0.05);
        }
        // Pressed against the ceiling, give or take the last flap's drift.
        assert!(game.y < 2.0, "{}", game.y);
        assert_ne!(game.phase, Phase::Over);
    }

    /// Plays well: flaps when the falling bird nears the bottom of the next opening. A flap
    /// lifts it about a third of the way up the world, so flapping any earlier overshoots.
    fn autopilot(game: &mut Game, seconds: f32) {
        let mut left = seconds;
        while left > 0.0 && game.phase == Phase::Playing {
            let floor = game
                .pipes
                .iter()
                .find(|pipe| pipe.x + PIPE_WIDTH > BIRD_X)
                .map_or(HEIGHT / 2.0, |pipe| pipe.gap_top + GAP);
            if game.y + BIRD_SIZE > floor - 8.0 && game.velocity > 0.0 {
                game.flap();
            }
            game.advance(STEP);
            left -= STEP;
        }
    }

    #[test]
    fn passing_pipes_scores() {
        let mut game = Game::new(12345, None);
        game.flap();
        autopilot(&mut game, 30.0);
        assert_eq!(
            game.phase,
            Phase::Playing,
            "the autopilot crashed with {}",
            game.score
        );
        // Pipes come every SPACING / SPEED seconds once the first has arrived.
        assert!(game.score >= 15, "only {} points", game.score);
        // Pipes that have left the screen are dropped.
        assert!(game.pipes.len() <= 4);
        assert!(game.pipes.iter().all(|pipe| pipe.gap_top >= 20.0));
    }

    #[test]
    fn hitting_a_pipe_ends_the_game_and_keeps_the_best() {
        let mut game = Game::new(3, None);
        game.flap();
        game.pipes = vec![Pipe {
            x: BIRD_X + 20.0,
            gap_top: 0.0,
            passed: false,
        }];
        // The opening is at the top, the bird in the middle: it flies into the pipe.
        game.y = 120.0;
        game.score = 3;
        let mut steps = 0;
        while game.phase == Phase::Playing && steps < 1000 {
            game.velocity = 0.0;
            game.advance(STEP);
            steps += 1;
        }
        assert_eq!(game.phase, Phase::Over);
        assert_eq!(game.best, 3);
    }

    #[test]
    fn the_same_seed_plays_the_same_game() {
        let play = |seed| {
            let mut game = Game::new(seed, None);
            game.flap();
            autopilot(&mut game, 10.0);
            game.pipes
                .iter()
                .map(|pipe| pipe.gap_top)
                .collect::<Vec<_>>()
        };
        assert_eq!(play(99), play(99));
        assert_ne!(play(99), play(100));
        // A long pause is not a long fall.
        let mut game = Game::new(1, None);
        game.flap();
        game.advance(60.0);
        assert_eq!(game.phase, Phase::Playing);
    }

    #[test]
    fn the_konami_code_is_recognised() {
        let mut konami = Konami::default();
        let code = KONAMI;
        assert!(!code[..9].iter().any(|key| konami.press(*key)));
        assert!(konami.press(Key::A));
        // A mistake starts over; an extra ↑ at the start does not spoil it.
        assert!(!konami.press(Key::ArrowUp));
        assert!(!konami.press(Key::X));
        let mut fresh = Konami::default();
        fresh.press(Key::ArrowUp);
        let presses: Vec<bool> = code.iter().map(|key| fresh.press(*key)).collect();
        assert_eq!(presses.iter().filter(|done| **done).count(), 1);
        assert!(presses[9]);
    }
}
