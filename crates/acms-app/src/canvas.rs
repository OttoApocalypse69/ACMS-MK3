//! The canvas viewport (see `PLAN.md`, section 4.6): shows the sprite, turns the mouse wheel
//! and drags into zoom and pan commands, and hands pointer input to the active tool.

use acms_core::color::Rgba8;
use acms_core::command::{CommandRegistry, Params};
use acms_core::compositor::{self, FrameCache, OnionSkin};
use acms_core::editor::Editor;
use acms_core::geom::{Point, Rect as PixelRect};
use acms_core::image::Image;
use acms_core::sprite::Symmetry;
use acms_core::tool::{self, CursorHint, Outline, PointerEvent};
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

    /// The screen position of a pixel corner (the top-left corner of that pixel).
    fn corner(&self, corner: Point) -> Pos2 {
        self.origin + Vec2::new(corner.x as f32, corner.y as f32) * self.scale
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

/// A reference layer as the canvas draws it this frame.
struct ReferenceView {
    texture: egui::TextureId,
    /// The canvas area the picture is stretched over.
    rect: PixelRect,
    opacity: u8,
    /// Drawn under the sprite rather than over it.
    below: bool,
}

/// What the canvas keeps between frames. None of it is part of the document.
#[derive(Default)]
pub struct CanvasView {
    /// The onion skin, and the generation, revision, frame and settings it was made for.
    onion_texture: Option<((u64, u64, usize, OnionSkin), TextureHandle)>,
    /// One texture per reference picture in use.
    references: Vec<(std::sync::Arc<Image>, TextureHandle)>,
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
    /// The border of the selection, and the mask it was worked out for.
    outline: Option<(usize, Vec<(Point, Point)>)>,
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
        sprite_tabs(ui, state, registry);
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

        let whole = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        let references = self.reference_views(ui.ctx(), &state.editor);
        let draw_references = |below: bool| {
            // Clipped to the canvas, like everything else that belongs to the sprite.
            let clipped = painter.with_clip_rect(sprite_rect.intersect(viewport));
            for view in references.iter().filter(|view| view.below == below) {
                let tint = Color32::from_white_alpha(view.opacity);
                clipped.image(view.texture, geometry.screen_rect(view.rect), whole, tint);
            }
        };
        draw_references(true);
        if let Some(onion) = self.onion(ui.ctx(), state) {
            painter.image(onion, sprite_rect, whole, Color32::WHITE);
        }

        let sprite_texture = self.sprite(ui.ctx(), &mut state.editor);
        if let Some(texture) = sprite_texture {
            painter.image(texture, sprite_rect, whole, Color32::WHITE);
        }
        draw_references(false);

        // Tiled mode: the sprite repeated all around, to see how its edges meet.
        if let Some(texture) = sprite_texture.filter(|_| state.editor.options.tiled) {
            let dim = tokens.surface.gamma_multiply(0.35);
            for (dx, dy) in (-1..=1).flat_map(|dy| (-1..=1).map(move |dx| (dx, dy))) {
                if (dx, dy) == (0, 0) {
                    continue;
                }
                let offset = Vec2::new(dx as f32, dy as f32) * sprite_rect.size();
                let copy = sprite_rect.translate(offset);
                if !copy.intersects(viewport) {
                    continue;
                }
                painter.image(checker, copy, checker_uv, Color32::WHITE);
                painter.image(texture, copy, whole, Color32::WHITE);
                painter.rect_filled(copy, 0.0, dim);
            }
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

        // The grid of a tilemap layer, when that is the layer being worked on.
        let tile_size = state.editor.document().and_then(|document| {
            let set = document.sprite().tileset_of(document.active_layer())?;
            Some((set.tile_width() as i32, set.tile_height() as i32))
        });
        if let Some((tile_w, tile_h)) = tile_size {
            let clipped = painter.with_clip_rect(sprite_rect.intersect(viewport));
            let stroke = Stroke::new(1.0, tokens.accent.gamma_multiply(0.45));
            let mut x = tile_w;
            while x < sprite_bounds.right() {
                clipped.vline(
                    geometry.corner(Point::new(x, 0)).x,
                    sprite_rect.y_range(),
                    stroke,
                );
                x += tile_w;
            }
            let mut y = tile_h;
            while y < sprite_bounds.bottom() {
                clipped.hline(
                    sprite_rect.x_range(),
                    geometry.corner(Point::new(0, y)).y,
                    stroke,
                );
                y += tile_h;
            }
        }

        // Slices: each an outlined rectangle with its name; the selected one stronger, with
        // its 9-slice centre and pivot.
        if let Some(document) = state.editor.document() {
            let active = document.active_slice();
            for (index, slice) in document.sprite().slices().iter().enumerate() {
                let c = slice.color;
                let color = Color32::from_rgb(c.r, c.g, c.b);
                let area = geometry.screen_rect(slice.rect);
                let selected = active == Some(index);
                let width = if selected { 2.0 } else { 1.0 };
                painter.rect_stroke(area, 0.0, Stroke::new(width, color), StrokeKind::Inside);
                painter.text(
                    area.left_top() + Vec2::new(2.0, -1.0),
                    egui::Align2::LEFT_BOTTOM,
                    &slice.name,
                    egui::TextStyle::Small.resolve(ui.style()),
                    color,
                );
                if !selected {
                    continue;
                }
                if let Some(center) = slice.center {
                    let inner = center.translated(slice.rect.x, slice.rect.y);
                    let inner = geometry.screen_rect(inner);
                    let thin = Stroke::new(1.0, color.gamma_multiply(0.7));
                    painter.vline(inner.left(), area.y_range(), thin);
                    painter.vline(inner.right(), area.y_range(), thin);
                    painter.hline(area.x_range(), inner.top(), thin);
                    painter.hline(area.x_range(), inner.bottom(), thin);
                }
                if let Some(pivot) = slice.pivot {
                    let at = Point::new(slice.rect.x + pivot.x, slice.rect.y + pivot.y);
                    let at = geometry
                        .screen_rect(PixelRect::new(at.x, at.y, 1, 1))
                        .center();
                    let arm = 5.0;
                    let stroke = Stroke::new(2.0, color);
                    painter
                        .line_segment([at - Vec2::new(arm, 0.0), at + Vec2::new(arm, 0.0)], stroke);
                    painter
                        .line_segment([at - Vec2::new(0.0, arm), at + Vec2::new(0.0, arm)], stroke);
                }
            }
        }

        // The mirror lines, or the spokes between the copies of a radial symmetry.
        let symmetry = state
            .editor
            .document()
            .map_or(Symmetry::Off, |document| document.sprite().symmetry());
        let axis = Stroke::new(1.0, tokens.accent.gamma_multiply(0.8));
        let center = sprite_rect.center();
        let clipped = painter.with_clip_rect(sprite_rect.intersect(viewport));
        if matches!(symmetry, Symmetry::Horizontal | Symmetry::Both) {
            clipped.vline(center.x, sprite_rect.y_range(), axis);
        }
        if matches!(symmetry, Symmetry::Vertical | Symmetry::Both) {
            clipped.hline(sprite_rect.x_range(), center.y, axis);
        }
        if let Symmetry::Radial(count) = symmetry {
            let reach = sprite_rect.size().length();
            for spoke in 0..count {
                let angle = std::f32::consts::TAU * f32::from(spoke) / f32::from(count.max(1))
                    - std::f32::consts::FRAC_PI_2;
                let end = center + Vec2::angled(angle) * reach;
                clipped.line_segment([center, end], axis);
            }
        }

        // The selection, and whatever shape the tool is dragging out, as marching ants.
        let segments = self.selection_outline(&state.editor);
        let mut paths: Vec<[Pos2; 2]> = segments
            .iter()
            .map(|&(a, b)| [geometry.corner(a), geometry.corner(b)])
            .collect();
        match state.editor.outline() {
            Some(Outline::Rect(rect)) => {
                let r = geometry.screen_rect(rect);
                let corners = [
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                ];
                paths.extend((0..4).map(|i| [corners[i], corners[(i + 1) % 4]]));
            }
            Some(Outline::Path(points)) => {
                let center = |p: &Point| {
                    geometry
                        .screen_rect(PixelRect::new(p.x, p.y, 1, 1))
                        .center()
                };
                paths.extend(
                    points
                        .windows(2)
                        .map(|pair| [center(&pair[0]), center(&pair[1])]),
                );
            }
            None => {}
        }
        if !paths.is_empty() {
            // The dashes crawl, which is what makes a selection easy to tell from the drawing.
            let time = ui.input(|input| input.time);
            let offset = (time * 8.0 % 8.0) as f32;
            let dark = Stroke::new(1.0, Color32::BLACK);
            let light = Stroke::new(1.0, Color32::WHITE);
            let mut shapes = Vec::new();
            for path in &paths {
                shapes.push(egui::Shape::line_segment(*path, dark));
                egui::Shape::dashed_line_many_with_offset(
                    path,
                    light,
                    &[4.0],
                    &[4.0],
                    offset,
                    &mut shapes,
                );
            }
            painter.extend(shapes);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(120));
        }

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

    /// The border segments of the selection, recomputed only when the selection changes.
    fn selection_outline(&mut self, editor: &Editor) -> Vec<(Point, Point)> {
        let selection = editor
            .document()
            .and_then(|document| document.sprite().selection());
        let Some(mask) = selection else {
            self.outline = None;
            return Vec::new();
        };
        // A selection is replaced, never changed in place, so its address identifies it.
        let key = std::sync::Arc::as_ptr(mask) as usize;
        match &self.outline {
            Some((made_for, segments)) if *made_for == key => segments.clone(),
            _ => {
                let segments = mask.outline();
                self.outline = Some((key, segments.clone()));
                segments
            }
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
        self.cache.note_revision(document.revision());
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
    /// The onion skin for the frame being shown, redone only when the sprite, the frame or
    /// the settings change. `None` when it is off, when playing, or with a single frame.
    fn onion(&mut self, ctx: &egui::Context, state: &AppState) -> Option<egui::TextureId> {
        let document = state.editor.document()?;
        let sprite = document.sprite();
        if !state.onion_enabled || state.playing.is_some() || sprite.frames().len() < 2 {
            self.onion_texture = None;
            return None;
        }
        let key = (
            state.editor.generation(),
            document.revision(),
            document.active_frame(),
            state.onion,
        );
        if !matches!(&self.onion_texture, Some((made_for, _)) if *made_for == key) {
            let skin = compositor::onion_skin(sprite, key.2, &state.onion);
            let texture = ctx.load_texture("onion", color_image(&skin), SPRITE_TEXTURE);
            self.onion_texture = Some((key, texture));
        }
        self.onion_texture.as_ref().map(|(_, texture)| texture.id())
    }

    /// The visible reference layers, with a texture for each picture. A reference below
    /// every image layer is drawn under the sprite; any other one over it.
    fn reference_views(&mut self, ctx: &egui::Context, editor: &Editor) -> Vec<ReferenceView> {
        let Some(document) = editor.document() else {
            self.references.clear();
            return Vec::new();
        };
        let entries = document.sprite().walk();
        let first_image = entries
            .iter()
            .position(|entry| entry.layer.as_image().is_some());

        let mut views = Vec::new();
        let mut in_use = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let Some(reference) = entry.layer.as_reference() else {
                continue;
            };
            let known = self
                .references
                .iter()
                .position(|(image, _)| std::sync::Arc::ptr_eq(image, &reference.image));
            let slot = known.unwrap_or_else(|| {
                let texture = ctx.load_texture(
                    "reference",
                    color_image(&reference.image),
                    TextureOptions::LINEAR,
                );
                self.references.push((reference.image.clone(), texture));
                self.references.len() - 1
            });
            in_use.push(self.references[slot].1.id());
            if entry.shown && reference.opacity > 0 {
                views.push(ReferenceView {
                    texture: self.references[slot].1.id(),
                    rect: reference.rect,
                    opacity: reference.opacity,
                    below: first_image.is_none_or(|first| index < first),
                });
            }
        }
        self.references
            .retain(|(_, texture)| in_use.contains(&texture.id()));
        views
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
            ui.label(RichText::new("A file dropped on the window is opened").weak());

            // The files worked on last, a click away.
            let recent: Vec<_> = state.recent.iter().take(5).cloned().collect();
            if !recent.is_empty() {
                ui.add_space(12.0);
                ui.label(RichText::new("Recent").weak());
                for path in recent {
                    let name = path.file_name().map_or_else(
                        || path.display().to_string(),
                        |name| name.to_string_lossy().into_owned(),
                    );
                    let tip = path.display().to_string();
                    if ui.link(name).on_hover_text(tip).clicked() {
                        let path = path.to_string_lossy().into_owned();
                        let params = Params::new().with("path", path);
                        commands::run(registry, state, "file.open", &params);
                    }
                }
            }
        });
    });
}

/// One tab per open sprite, above the canvas. Shown once there is more than one, or when the
/// only one could be closed from here.
fn sprite_tabs(ui: &mut Ui, state: &mut AppState, registry: &CommandRegistry<AppState>) {
    let tabs: Vec<(u64, String, bool)> = state
        .editor
        .documents()
        .map(|(id, _)| {
            let active = state.editor.active_document() == Some(id);
            (id.0, state.title_of(id), active)
        })
        .collect();
    if tabs.is_empty() {
        return;
    }
    let mut activate = None;
    let mut close = None;
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (id, title, active) in &tabs {
                    if ui.selectable_label(*active, title).clicked() {
                        activate = Some(*id);
                    }
                    if ui.small_button("×").on_hover_text("Close").clicked() {
                        close = Some(*id);
                    }
                    ui.add_space(8.0);
                }
            });
        });
    if let Some(id) = activate.or(close) {
        let params = Params::new().with("doc", id as i64);
        commands::run(registry, state, "doc.activate", &params);
    }
    if close.is_some() {
        commands::run(registry, state, "file.close", &Params::new());
    }
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
