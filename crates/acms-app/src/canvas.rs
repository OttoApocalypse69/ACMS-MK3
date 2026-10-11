//! The canvas viewport (see `PLAN.md`, section 4.6): shows the sprite, turns the mouse wheel
//! and drags into zoom and pan commands, and hands pointer input to the active tool.

use acms_core::color::Rgba8;
use acms_core::command::{CommandRegistry, Params};
use acms_core::compositor::FrameCache;
use acms_core::editor::Editor;
use acms_core::geom::{Point, Rect as PixelRect};
use acms_core::image::Image;
use acms_core::tool::{self, CursorHint, PointerEvent};
use egui::{
    Color32, ColorImage, CursorIcon, Event, Key, MouseWheelUnit, PointerButton, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, TextureFilter, TextureHandle, TextureOptions, Ui,
    UiBuilder, Vec2,
};

use crate::commands;
use crate::keymap::Keymap;
use crate::state::AppState;
use crate::theme::ThemeKind;

/// Side of one checkerboard square, in screen pixels.
const CHECKER_SIZE: f32 = 8.0;
/// The pixel grid is only drawn when a sprite pixel is at least this many screen pixels wide.
const GRID_MIN_ZOOM: f32 = 8.0;
/// How far a touchpad has to scroll, in points, to count as one notch of a mouse wheel.
const POINTS_PER_WHEEL_STEP: f32 = 50.0;

/// Sprite pixels are shown unfiltered when enlarged, and smoothed when shrunk.
const SPRITE_TEXTURE: TextureOptions = TextureOptions {
    minification: TextureFilter::Linear,
    ..TextureOptions::NEAREST
};

/// Where the sprite sits on screen this frame.
#[derive(Clone, Copy, Debug)]
struct Geometry {
    /// Screen position of the sprite's top-left corner, in points.
    origin: Pos2,
    /// Points per sprite pixel.
    scale: f32,
}

impl Geometry {
    fn new(viewport: Rect, pan: Vec2, zoom: f32, sprite: Vec2, pixels_per_point: f32) -> Self {
        let scale = zoom / pixels_per_point;
        let center = viewport.center() + pan / pixels_per_point;
        let origin = center - sprite * scale / 2.0;
        // Start on a whole screen pixel, or sprite pixels come out uneven.
        let snap = |v: f32| (v * pixels_per_point).round() / pixels_per_point;
        Self {
            origin: Pos2::new(snap(origin.x), snap(origin.y)),
            scale,
        }
    }

    /// The sprite pixel under a screen position.
    fn pixel_at(&self, pos: Pos2) -> Point {
        let local = (pos - self.origin) / self.scale;
        Point::new(local.x.floor() as i32, local.y.floor() as i32)
    }

    /// The screen rectangle covered by a rectangle of sprite pixels.
    fn screen_rect(&self, rect: PixelRect) -> Rect {
        let min = self.origin + Vec2::new(rect.x as f32, rect.y as f32) * self.scale;
        Rect::from_min_size(min, Vec2::new(rect.w as f32, rect.h as f32) * self.scale)
    }
}

fn color_image(image: &Image) -> ColorImage {
    let size = [image.width() as usize, image.height() as usize];
    ColorImage::from_rgba_unmultiplied(size, &image.to_rgba_bytes())
}

/// What the canvas keeps between frames. None of it is part of the document.
#[derive(Default)]
pub struct CanvasView {
    /// The flattened frame; only what changed is recomposed.
    cache: FrameCache,
    /// The texture showing `cache`, and the editor generation it was made for.
    sprite_texture: Option<(u64, TextureHandle)>,
    checker_texture: Option<(ThemeKind, TextureHandle)>,
    /// Wheel movement not yet turned into a whole zoom step.
    wheel: f32,
    /// The button of the tool interaction in progress.
    drawing: Option<PointerButton>,
    /// The last pixel reported to the tool.
    last_pixel: Point,
    geometry: Option<Geometry>,
    /// The sprite pixel under the pointer and the colour seen there.
    hover: Option<(Point, Rgba8)>,
}

impl CanvasView {
    /// The sprite pixel under the pointer and the colour seen there, as of the last frame.
    pub fn hover(&self) -> Option<(Point, Rgba8)> {
        self.hover
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        state: &mut AppState,
        registry: &CommandRegistry<AppState>,
        keymap: &Keymap,
    ) {
        let tokens = state.theme.tokens();
        let pixels_per_point = ui.ctx().pixels_per_point();
        let viewport = ui.available_rect_before_wrap();
        state.view.viewport = viewport.size() * pixels_per_point;
        ui.painter_at(viewport)
            .rect_filled(viewport, 0.0, tokens.surface);

        let Some(sprite) = state.editor.document().map(|document| document.sprite()) else {
            self.geometry = None;
            self.drawing = None;
            self.hover = None;
            empty_state(ui, viewport, state, registry, keymap);
            return;
        };
        let sprite_bounds = sprite.bounds();
        let sprite_size = Vec2::new(sprite.width() as f32, sprite.height() as f32);

        let response = ui.allocate_rect(viewport, Sense::click_and_drag());
        if std::mem::take(&mut state.view.fit_pending) {
            commands::run(registry, state, "view.zoom_fit", &Params::new());
        }

        let space_held =
            ui.input(|input| input.key_down(Key::Space)) && !ui.ctx().text_edit_focused();
        let panning = response.dragged_by(PointerButton::Middle)
            || (space_held
                && self.drawing.is_none()
                && response.dragged_by(PointerButton::Primary));
        self.zoom_and_pan(ui, &response, viewport, panning, state, registry);

        // Zooming and panning are done; from here on the sprite does not move this frame.
        let geometry = Geometry::new(
            viewport,
            state.view.pan,
            state.view.zoom(),
            sprite_size,
            pixels_per_point,
        );
        self.geometry = Some(geometry);
        self.tool_input(ui, &response, geometry, space_held, &mut state.editor);

        // Painting.
        let painter = ui.painter_at(viewport);
        let sprite_rect = geometry.screen_rect(sprite_bounds);

        let checker = self.checker(ui.ctx(), state.theme);
        let squares = sprite_rect.size() * pixels_per_point / (2.0 * CHECKER_SIZE);
        let checker_uv = Rect::from_min_size(Pos2::ZERO, squares);
        painter.image(checker, sprite_rect, checker_uv, Color32::WHITE);

        if let Some(texture) = self.sprite(ui.ctx(), &mut state.editor) {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            painter.image(texture, sprite_rect, uv, Color32::WHITE);
        }

        if state.view.show_grid && state.view.zoom() >= GRID_MIN_ZOOM {
            let stroke = Stroke::new(
                1.0 / pixels_per_point,
                Color32::from_rgba_unmultiplied(128, 128, 128, 80),
            );
            let visible = sprite_rect.intersect(viewport);
            let first = geometry.pixel_at(visible.min);
            let last = geometry.pixel_at(visible.max);
            for x in first.x.max(1)..=last.x.min(sprite_bounds.right() - 1) {
                let screen_x = geometry.origin.x + x as f32 * geometry.scale;
                painter.vline(screen_x, visible.y_range(), stroke);
            }
            for y in first.y.max(1)..=last.y.min(sprite_bounds.bottom() - 1) {
                let screen_y = geometry.origin.y + y as f32 * geometry.scale;
                painter.hline(visible.x_range(), screen_y, stroke);
            }
        }
        painter.rect_stroke(
            sprite_rect,
            0.0,
            Stroke::new(1.0, tokens.border),
            StrokeKind::Outside,
        );

        let hover_pixel = response.hover_pos().map(|pos| geometry.pixel_at(pos));
        self.hover = hover_pixel.map(|pixel| (pixel, self.cache.image().get(pixel.x, pixel.y)));

        // What the tool would touch at the pointer.
        let brush_tool = state.editor.cursor_hint() == CursorHint::Brush;
        if panning {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if let Some(pixel) = hover_pixel {
            if space_held {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            } else if brush_tool && state.editor.draw_block().is_some() {
                // The status bar says why.
                ui.ctx().set_cursor_icon(CursorIcon::NotAllowed);
            } else {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                let covered = match state.editor.cursor_hint() {
                    CursorHint::Brush => state.editor.brush.bounds_at(pixel),
                    CursorHint::Pixel => PixelRect::new(pixel.x, pixel.y, 1, 1),
                };
                let outline = geometry.screen_rect(covered);
                let dark = Stroke::new(1.0, Color32::from_black_alpha(170));
                let light = Stroke::new(1.0, Color32::from_white_alpha(210));
                painter.rect_stroke(outline, 0.0, dark, StrokeKind::Outside);
                painter.rect_stroke(outline, 0.0, light, StrokeKind::Inside);
            }
        }
    }

    /// Turns the wheel into zoom commands and drags into pan commands.
    fn zoom_and_pan(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        viewport: Rect,
        panning: bool,
        state: &mut AppState,
        registry: &CommandRegistry<AppState>,
    ) {
        let pixels_per_point = ui.ctx().pixels_per_point();

        if response.contains_pointer() {
            let scrolled: f32 = ui.input(|input| {
                input
                    .events
                    .iter()
                    .map(|event| match event {
                        Event::MouseWheel { unit, delta, .. } => match unit {
                            MouseWheelUnit::Line => delta.y,
                            MouseWheelUnit::Page => delta.y * 4.0,
                            MouseWheelUnit::Point => delta.y / POINTS_PER_WHEEL_STEP,
                        },
                        _ => 0.0,
                    })
                    .sum()
            });
            self.wheel += scrolled;
            let steps = self.wheel.trunc();
            self.wheel -= steps;

            // Zoom around the pointer, so what is under it stays under it.
            let anchor = response.hover_pos().map_or(Vec2::ZERO, |pos| {
                (pos - viewport.center()) * pixels_per_point
            });
            let params = Params::new()
                .with("anchor_x", f64::from(anchor.x))
                .with("anchor_y", f64::from(anchor.y));
            let command = if steps > 0.0 {
                "view.zoom_in"
            } else {
                "view.zoom_out"
            };
            for _ in 0..steps.abs() as u32 {
                commands::run(registry, state, command, &params);
            }
        } else {
            self.wheel = 0.0;
        }

        if panning {
            let delta = response.drag_delta() * pixels_per_point;
            if delta != Vec2::ZERO {
                let params = Params::new()
                    .with("dx", f64::from(delta.x))
                    .with("dy", f64::from(delta.y));
                commands::run(registry, state, "view.pan", &params);
            }
        }
    }

    /// Hands this frame's pointer events to the active tool, in the order they happened.
    fn tool_input(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        geometry: Geometry,
        space_held: bool,
        editor: &mut Editor,
    ) {
        enum Phase {
            Down(PointerButton),
            Move,
            Up(PointerButton),
        }

        let (events, modifiers, any_down) = ui.input(|input| {
            let events: Vec<(Phase, Pos2)> = input
                .events
                .iter()
                .filter_map(|event| match *event {
                    Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        ..
                    } => {
                        let phase = if pressed { Phase::Down } else { Phase::Up };
                        Some((phase(button), pos))
                    }
                    Event::PointerMoved(pos) => Some((Phase::Move, pos)),
                    _ => None,
                })
                .collect();
            (events, input.modifiers, input.pointer.any_down())
        });

        let tool_button = |button: PointerButton| match button {
            PointerButton::Primary => Some(tool::PointerButton::Primary),
            PointerButton::Secondary => Some(tool::PointerButton::Secondary),
            _ => None,
        };
        let modifiers = tool::Modifiers {
            shift: modifiers.shift,
            ctrl: modifiers.command,
            alt: modifiers.alt,
        };
        let event_at = |pixel: Point, button: tool::PointerButton| PointerEvent {
            position: pixel,
            button,
            modifiers,
        };

        // A press only starts a stroke if it landed on the canvas itself, not on a dialog or
        // another panel on top of it.
        let pressed_on_canvas = response.is_pointer_button_down_on();

        for (phase, pos) in events {
            let pixel = geometry.pixel_at(pos);
            match phase {
                Phase::Down(button) => {
                    let Some(tool_button) = tool_button(button) else {
                        continue;
                    };
                    let starts_pan = space_held && button == PointerButton::Primary;
                    if self.drawing.is_none() && pressed_on_canvas && !starts_pan {
                        self.drawing = Some(button);
                        self.last_pixel = pixel;
                        editor.pointer_down(event_at(pixel, tool_button));
                    }
                }
                Phase::Move => {
                    if let Some(tool_button) = self.drawing.and_then(tool_button) {
                        self.last_pixel = pixel;
                        editor.pointer_move(event_at(pixel, tool_button));
                    }
                }
                Phase::Up(button) => {
                    if self.drawing == Some(button) {
                        self.drawing = None;
                        if let Some(tool_button) = tool_button(button) {
                            editor.pointer_up(event_at(pixel, tool_button));
                        }
                    }
                }
            }
        }

        if let Some(tool_button) = self.drawing.and_then(tool_button) {
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                self.drawing = None;
                editor.cancel_interaction();
            } else if !any_down {
                // The release never arrived, e.g. it happened outside the window.
                self.drawing = None;
                editor.pointer_up(event_at(self.last_pixel, tool_button));
            }
        }
        if !editor.is_interacting() {
            // Something else ended the stroke, such as an undo or a tool change.
            self.drawing = None;
        }
    }

    /// The texture of the flattened sprite, brought up to date with what changed.
    fn sprite(&mut self, ctx: &egui::Context, editor: &mut Editor) -> Option<egui::TextureId> {
        let generation = editor.generation();
        let damage = editor.take_damage();
        let document = editor.document()?;
        let (sprite, frame) = (document.sprite(), document.active_frame());
        let size = [sprite.width() as usize, sprite.height() as usize];

        let current = match &mut self.sprite_texture {
            Some((made_for, texture)) if *made_for == generation && texture.size() == size => {
                Some(texture)
            }
            _ => None,
        };
        match current {
            Some(texture) => {
                // Only the part that changed is flattened and uploaded again.
                let redrawn = self.cache.refresh(sprite, frame, editor.overlay(), damage);
                if !redrawn.is_empty() {
                    let part = self.cache.image().crop(redrawn);
                    let at = [redrawn.x as usize, redrawn.y as usize];
                    texture.set_partial(at, color_image(&part), SPRITE_TEXTURE);
                }
            }
            None => {
                self.cache.invalidate();
                self.cache.refresh(sprite, frame, editor.overlay(), damage);
                let image = color_image(self.cache.image());
                let texture = ctx.load_texture("canvas", image, SPRITE_TEXTURE);
                self.sprite_texture = Some((generation, texture));
            }
        }
        self.sprite_texture
            .as_ref()
            .map(|(_, texture)| texture.id())
    }
    /// A 2×2 texture that repeats into the checkerboard behind transparent pixels.
    fn checker(&mut self, ctx: &egui::Context, theme: ThemeKind) -> egui::TextureId {
        if !matches!(&self.checker_texture, Some((made_for, _)) if *made_for == theme) {
            let [a, b] = theme.tokens().checker;
            let image = ColorImage::new([2, 2], vec![a, b, b, a]);
            let texture = ctx.load_texture("checker", image, TextureOptions::NEAREST_REPEAT);
            self.checker_texture = Some((theme, texture));
        }
        // Just set above if it was missing.
        self.checker_texture
            .as_ref()
            .map_or(egui::TextureId::default(), |(_, texture)| texture.id())
    }

    /// The centre of a sprite pixel on screen, as of the last frame. For tests.
    #[cfg(test)]
    pub fn screen_pos(&self, x: i32, y: i32) -> Pos2 {
        let geometry = self
            .geometry
            .expect("the canvas has been shown with a sprite");
        geometry.screen_rect(PixelRect::new(x, y, 1, 1)).center()
    }
}

/// Shown instead of the canvas when no sprite is open.
fn empty_state(
    ui: &mut Ui,
    viewport: Rect,
    state: &mut AppState,
    registry: &CommandRegistry<AppState>,
    keymap: &Keymap,
) {
    let shortcut = |command: &str| {
        keymap
            .shortcuts_for(command)
            .next()
            .map(|shortcut| ui.ctx().format_shortcut(shortcut))
            .unwrap_or_default()
    };
    let hint = format!(
        "{} opens the command palette",
        shortcut("view.command_palette")
    );
    let actions = [("file.new", "New Sprite…"), ("file.open", "Open…")]
        .map(|(command, label)| (command, label, shortcut(command)));

    ui.scope_builder(UiBuilder::new().max_rect(viewport), |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space((viewport.height() * 0.32).max(12.0));
            ui.heading("No sprite open");
            ui.add_space(4.0);
            for (command, label, shortcut) in actions {
                if ui.button(label).on_hover_text(shortcut).clicked() {
                    commands::run(registry, state, command, &Params::new());
                }
            }
            ui.add_space(4.0);
            ui.label(RichText::new(hint).weak());
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(400.0, 300.0))
    }

    #[test]
    fn a_centred_sprite_maps_pixels_both_ways() {
        // 32×32 at 8× in the middle of a 400×300 viewport: 256 wide, starting 72 in.
        let geometry = Geometry::new(viewport(), Vec2::ZERO, 8.0, Vec2::splat(32.0), 1.0);
        assert_eq!(geometry.origin, Pos2::new(172.0, 72.0));
        assert_eq!(geometry.pixel_at(Pos2::new(172.0, 72.0)), Point::new(0, 0));
        assert_eq!(geometry.pixel_at(Pos2::new(179.9, 79.9)), Point::new(0, 0));
        assert_eq!(geometry.pixel_at(Pos2::new(180.0, 72.0)), Point::new(1, 0));
        assert_eq!(geometry.pixel_at(Pos2::new(171.9, 72.0)), Point::new(-1, 0));
        assert_eq!(
            geometry.pixel_at(Pos2::new(427.9, 327.9)),
            Point::new(31, 31)
        );

        let rect = geometry.screen_rect(PixelRect::new(2, 3, 4, 1));
        assert_eq!(rect.min, Pos2::new(188.0, 96.0));
        assert_eq!(rect.size(), Vec2::new(32.0, 8.0));
    }

    #[test]
    fn panning_moves_the_sprite_by_screen_pixels() {
        let still = Geometry::new(viewport(), Vec2::ZERO, 4.0, Vec2::splat(16.0), 1.0);
        let moved = Geometry::new(
            viewport(),
            Vec2::new(10.0, -6.0),
            4.0,
            Vec2::splat(16.0),
            1.0,
        );
        assert_eq!(moved.origin - still.origin, Vec2::new(10.0, -6.0));
    }

    #[test]
    fn on_a_scaled_display_a_sprite_pixel_is_a_whole_number_of_screen_pixels() {
        for pixels_per_point in [1.25, 1.5, 2.0] {
            let geometry = Geometry::new(
                viewport(),
                Vec2::new(3.0, 7.0),
                3.0,
                Vec2::splat(17.0),
                pixels_per_point,
            );
            // 3× zoom means exactly three physical pixels per sprite pixel…
            assert!((geometry.scale * pixels_per_point - 3.0).abs() < 1e-4);
            // …and the sprite starts on a physical pixel boundary.
            let origin = geometry.origin.to_vec2() * pixels_per_point;
            assert!((origin.x - origin.x.round()).abs() < 1e-3);
            assert!((origin.y - origin.y.round()).abs() < 1e-3);
        }
    }
}
