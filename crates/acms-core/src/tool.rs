//! Tools (see `PLAN.md`, section 4.5).
//!
//! A tool receives pointer events in canvas pixel coordinates and acts on the document through
//! a [`ToolContext`]. It never knows about windows, zoom or panning.

use crate::color::Rgba8;
use crate::compositor::{self, Overlay};
use crate::document::Document;
use crate::geom::{Point, Rect};
use crate::stroke::{Brush, Ink, Stroke, StrokeBuffer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerEvent {
    /// The canvas pixel under the pointer. It may lie outside the canvas.
    pub position: Point,
    /// The button that started the interaction.
    pub button: PointerButton,
    pub modifiers: Modifiers,
}

/// What a tool may read and change.
pub struct ToolContext<'a> {
    pub document: &'a mut Document,
    pub primary: &'a mut Rgba8,
    pub secondary: &'a mut Rgba8,
    pub brush: Brush,
    pub options: ToolOptions,
}

/// Settings shared by the tools that are not about the brush.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolOptions {
    /// Rectangle and ellipse are filled rather than outlined.
    pub shape_filled: bool,
    /// Bucket and magic wand only take pixels connected to the one clicked.
    pub fill_contiguous: bool,
    /// How different a pixel may be (per channel) and still count as the same colour.
    pub tolerance: u8,
    /// The gradient is made of the two colours in a dither pattern, not blended.
    pub gradient_dither: bool,
}

impl Default for ToolOptions {
    fn default() -> Self {
        Self {
            shape_filled: false,
            fill_contiguous: true,
            tolerance: 0,
            gradient_dither: true,
        }
    }
}

/// A group of settings a tool uses, for the Tool Options panel to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionKind {
    Brush,
    ShapeFill,
    Similarity,
    Gradient,
    SelectionMode,
}

/// A shape a tool is dragging out, drawn by the view on top of the canvas. Coordinates are
/// canvas pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outline {
    Rect(Rect),
    /// Corner-to-corner path through pixel centres, closed.
    Path(Vec<Point>),
}

/// What the viewport should draw around the pointer while this tool is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorHint {
    /// The outline of the brush.
    Brush,
    /// Only the single pixel under the pointer.
    Pixel,
}

pub trait Tool {
    /// Stable id, used in command ids (`tool.<id>`) and by scripts.
    fn id(&self) -> &'static str;

    /// Name shown in the UI.
    fn name(&self) -> &'static str;

    fn pointer_down(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent);

    /// Called while the button from [`Tool::pointer_down`] is held.
    fn pointer_move(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent);

    fn pointer_up(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent);

    /// Abandons whatever is in progress, leaving the document as it was.
    fn cancel(&mut self, ctx: &mut ToolContext<'_>);

    /// The stroke being drawn, for the compositor to show on top of the sprite.
    fn overlay(&self) -> Option<Overlay<'_>> {
        None
    }

    fn cursor_hint(&self) -> CursorHint {
        CursorHint::Pixel
    }

    /// The settings this tool uses.
    fn options(&self) -> &'static [OptionKind] {
        &[]
    }

    /// The shape being dragged out, if the tool shows one.
    fn outline(&self) -> Option<Outline> {
        None
    }

    /// Whether the tool paints on the layer, and so cannot work on a locked or hidden one.
    fn paints(&self) -> bool {
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrushToolKind {
    Pencil,
    Eraser,
}

/// Pencil and eraser: both drag a brush across the canvas, with a different ink.
pub struct BrushTool {
    kind: BrushToolKind,
    stroke: Option<Stroke>,
    /// The preview buffer, kept between strokes so it is not reallocated for each one.
    spare: StrokeBuffer,
}

impl BrushTool {
    /// Draws with the primary colour, or the secondary colour for the secondary button.
    pub fn pencil() -> Self {
        Self::new(BrushToolKind::Pencil)
    }

    pub fn eraser() -> Self {
        Self::new(BrushToolKind::Eraser)
    }

    fn new(kind: BrushToolKind) -> Self {
        Self {
            kind,
            stroke: None,
            spare: StrokeBuffer::new(),
        }
    }
}

impl Tool for BrushTool {
    fn id(&self) -> &'static str {
        match self.kind {
            BrushToolKind::Pencil => "pencil",
            BrushToolKind::Eraser => "eraser",
        }
    }

    fn name(&self) -> &'static str {
        match self.kind {
            BrushToolKind::Pencil => "Pencil",
            BrushToolKind::Eraser => "Eraser",
        }
    }

    fn pointer_down(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.cancel(ctx);

        let ink = match (self.kind, event.button) {
            (BrushToolKind::Eraser, _) => Ink::Erase,
            (BrushToolKind::Pencil, PointerButton::Primary) => Ink::Simple(*ctx.primary),
            (BrushToolKind::Pencil, PointerButton::Secondary) => Ink::Simple(*ctx.secondary),
        };
        let document = &mut *ctx.document;
        let buffer = std::mem::take(&mut self.spare);
        // The active layer and frame always exist, so this only fails if the document is
        // broken; then the tool simply does nothing.
        let started = Stroke::begin(
            document.sprite(),
            document.active_layer(),
            document.active_frame(),
            ctx.brush,
            ink,
            buffer,
        );
        if let Ok(mut stroke) = started {
            let damage = stroke.extend_to(document.sprite(), event.position);
            document.add_damage(damage);
            self.stroke = Some(stroke);
        }
    }

    fn pointer_move(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        if let Some(stroke) = &mut self.stroke {
            let damage = stroke.extend_to(ctx.document.sprite(), event.position);
            ctx.document.add_damage(damage);
        }
    }

    fn pointer_up(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.pointer_move(ctx, event);
        if let Some(stroke) = self.stroke.take() {
            let covered = stroke.buffer().bounds();
            // See `pointer_down` for why an error is not expected here.
            if let Ok(buffer) = stroke.commit(ctx.document, self.name()) {
                self.spare = buffer;
            }
            ctx.document.add_damage(covered);
        }
    }

    fn cancel(&mut self, ctx: &mut ToolContext<'_>) {
        if let Some(stroke) = self.stroke.take() {
            let (buffer, covered) = stroke.cancel();
            self.spare = buffer;
            ctx.document.add_damage(covered);
        }
    }

    fn overlay(&self) -> Option<Overlay<'_>> {
        self.stroke.as_ref().map(|stroke| Overlay {
            layer: stroke.layer(),
            frame: stroke.frame(),
            buffer: stroke.buffer(),
        })
    }

    fn cursor_hint(&self) -> CursorHint {
        CursorHint::Brush
    }

    fn options(&self) -> &'static [OptionKind] {
        &[OptionKind::Brush]
    }
}

/// Picks the colour under the pointer: into the primary colour with the primary button, the
/// secondary colour with the other one.
#[derive(Default)]
pub struct Eyedropper;

impl Eyedropper {
    fn pick(ctx: &mut ToolContext<'_>, event: PointerEvent) {
        let sprite = ctx.document.sprite();
        let Point { x, y } = event.position;
        if !sprite.bounds().contains(event.position) {
            return;
        }
        // What the user sees at that pixel, all layers combined.
        let seen = compositor::compose_rect(
            sprite,
            ctx.document.active_frame(),
            None,
            Rect::new(x, y, 1, 1),
        )
        .get(0, 0);
        // Picking "nothing" would turn the pencil into an eraser without saying so.
        if seen.a == 0 {
            return;
        }
        match event.button {
            PointerButton::Primary => *ctx.primary = seen,
            PointerButton::Secondary => *ctx.secondary = seen,
        }
    }
}

impl Tool for Eyedropper {
    fn id(&self) -> &'static str {
        "eyedropper"
    }

    fn name(&self) -> &'static str {
        "Eyedropper"
    }

    fn pointer_down(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        Self::pick(ctx, event);
    }

    fn pointer_move(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        Self::pick(ctx, event);
    }

    fn pointer_up(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        Self::pick(ctx, event);
    }

    fn cancel(&mut self, _ctx: &mut ToolContext<'_>) {}

    fn paints(&self) -> bool {
        false
    }
}
