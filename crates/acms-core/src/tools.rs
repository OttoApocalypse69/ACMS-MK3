//! The shape, fill, selection and move tools.
//!
//! The painting ones draw through a [`Stroke`], so they get the brush, alpha lock, the
//! selection mask and one-step undo the same way the pencil does.

use crate::color::Rgba8;
use crate::compositor::Overlay;
use crate::geom::{Point, Rect};
use crate::image::Image;
use crate::selection::{Combine, Mask};
use crate::stroke::{Ink, Stroke, StrokeBuffer};
use crate::tool::{
    CursorHint, Modifiers, OptionKind, Outline, PointerButton, PointerEvent, Tool, ToolContext,
};

/// The rectangle with `a` and `b` as opposite corner pixels, both included.
pub fn rect_between(a: Point, b: Point) -> Rect {
    Rect::from_edges(
        a.x.min(b.x),
        a.y.min(b.y),
        a.x.max(b.x) + 1,
        a.y.max(b.y) + 1,
    )
}

/// Moves `b` so that the box from `a` to `b` is square, keeping its direction.
fn squared(a: Point, b: Point) -> Point {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let side = dx.abs().max(dy.abs());
    let sign = |v: i32| if v < 0 { -1 } else { 1 };
    Point::new(a.x + sign(dx) * side, a.y + sign(dy) * side)
}

/// Whether a pixel lies in the ellipse that fits `bounds` exactly.
fn in_ellipse(bounds: Rect, x: i32, y: i32) -> bool {
    let (rx, ry) = (bounds.w as f32 / 2.0, bounds.h as f32 / 2.0);
    let dx = (x - bounds.x) as f32 + 0.5 - rx;
    let dy = (y - bounds.y) as f32 + 0.5 - ry;
    bounds.contains(Point::new(x, y)) && (dx * dx) / (rx * rx) + (dy * dy) / (ry * ry) <= 1.0
}

fn ink_for(ctx: &ToolContext<'_>, button: PointerButton) -> Ink {
    match button {
        PointerButton::Primary => Ink::Simple(*ctx.primary),
        PointerButton::Secondary => Ink::Simple(*ctx.secondary),
    }
}

/// Starts a stroke on the selected layer. `None` if it cannot be drawn on.
fn begin(ctx: &ToolContext<'_>, ink: Ink, buffer: StrokeBuffer) -> Option<Stroke> {
    let document = &*ctx.document;
    Stroke::begin(
        document.sprite(),
        document.active_layer(),
        document.active_frame(),
        ctx.brush,
        ink,
        buffer,
    )
    .ok()
}

fn overlay_of(stroke: Option<&Stroke>) -> Option<Overlay<'_>> {
    stroke.map(|stroke| Overlay {
        layer: stroke.layer(),
        frame: stroke.frame(),
        buffer: stroke.buffer(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Line,
    Rectangle,
    Ellipse,
}

/// Line, rectangle and ellipse: drag from one corner to the other.
pub struct ShapeTool {
    shape: Shape,
    start: Point,
    stroke: Option<Stroke>,
    spare: StrokeBuffer,
}

impl ShapeTool {
    pub fn line() -> Self {
        Self::new(Shape::Line)
    }

    pub fn rectangle() -> Self {
        Self::new(Shape::Rectangle)
    }

    pub fn ellipse() -> Self {
        Self::new(Shape::Ellipse)
    }

    fn new(shape: Shape) -> Self {
        Self {
            shape,
            start: Point::default(),
            stroke: None,
            spare: StrokeBuffer::new(),
        }
    }

    /// Draws the shape from the start point to `event` into the preview, from scratch.
    fn redraw(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        let Some(stroke) = &mut self.stroke else {
            return;
        };
        let sprite = ctx.document.sprite();
        let before = stroke.reset();
        let (a, mut b) = (self.start, event.position);
        if event.modifiers.shift && self.shape != Shape::Line {
            b = squared(a, b);
        }
        let bounds = rect_between(a, b);
        let filled = ctx.options.shape_filled;

        match self.shape {
            Shape::Line => stroke.stamp_line(sprite, a, b),
            Shape::Rectangle if filled => {
                for y in bounds.y..bounds.bottom() {
                    for x in bounds.x..bounds.right() {
                        stroke.fill(sprite, x, y);
                    }
                }
            }
            Shape::Rectangle => {
                let (l, t, r, bt) = (bounds.x, bounds.y, bounds.right() - 1, bounds.bottom() - 1);
                let corners = [(l, t), (r, t), (r, bt), (l, bt)].map(|(x, y)| Point::new(x, y));
                for index in 0..4 {
                    stroke.stamp_line(sprite, corners[index], corners[(index + 1) % 4]);
                }
            }
            Shape::Ellipse => {
                for y in bounds.y..bounds.bottom() {
                    for x in bounds.x..bounds.right() {
                        if !in_ellipse(bounds, x, y) {
                            continue;
                        }
                        let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                            .iter()
                            .any(|(dx, dy)| !in_ellipse(bounds, x + dx, y + dy));
                        if filled {
                            stroke.fill(sprite, x, y);
                        } else if edge {
                            stroke.stamp(sprite, Point::new(x, y));
                        }
                    }
                }
            }
        }
        let after = stroke.buffer().bounds();
        ctx.document.add_damage(before.union(after));
    }
}

impl Tool for ShapeTool {
    fn id(&self) -> &'static str {
        match self.shape {
            Shape::Line => "line",
            Shape::Rectangle => "rectangle",
            Shape::Ellipse => "ellipse",
        }
    }

    fn name(&self) -> &'static str {
        match self.shape {
            Shape::Line => "Line",
            Shape::Rectangle => "Rectangle",
            Shape::Ellipse => "Ellipse",
        }
    }

    fn pointer_down(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.cancel(ctx);
        let ink = ink_for(ctx, event.button);
        self.stroke = begin(ctx, ink, std::mem::take(&mut self.spare));
        self.start = event.position;
        self.redraw(ctx, event);
    }

    fn pointer_move(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.redraw(ctx, event);
    }

    fn pointer_up(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.redraw(ctx, event);
        if let Some(stroke) = self.stroke.take() {
            let covered = stroke.buffer().bounds();
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
        overlay_of(self.stroke.as_ref())
    }

    fn cursor_hint(&self) -> CursorHint {
        CursorHint::Brush
    }

    fn options(&self) -> &'static [OptionKind] {
        match self.shape {
            Shape::Line => &[OptionKind::Brush],
            _ => &[OptionKind::Brush, OptionKind::ShapeFill],
        }
    }
}

/// Fills the area of similar colour under the pointer.
#[derive(Default)]
pub struct BucketTool {
    spare: StrokeBuffer,
}

impl Tool for BucketTool {
    fn id(&self) -> &'static str {
        "bucket"
    }

    fn name(&self) -> &'static str {
        "Bucket Fill"
    }

    fn pointer_down(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        let ink = ink_for(ctx, event.button);
        let Some(mut stroke) = begin(ctx, ink, std::mem::take(&mut self.spare)) else {
            return;
        };
        let sprite = ctx.document.sprite();
        // The area is found on the layer being filled, as it is before the fill.
        let layer = sprite.read_region(stroke.layer(), stroke.frame(), sprite.bounds());
        let options = ctx.options;
        let area = Mask::similar(
            &layer,
            event.position,
            options.tolerance,
            options.fill_contiguous,
        );
        for p in area.pixels() {
            stroke.fill(sprite, p.x, p.y);
        }
        let covered = stroke.buffer().bounds();
        if let Ok(buffer) = stroke.commit(ctx.document, self.name()) {
            self.spare = buffer;
        }
        ctx.document.add_damage(covered);
    }

    fn pointer_move(&mut self, _ctx: &mut ToolContext<'_>, _event: PointerEvent) {}

    fn pointer_up(&mut self, _ctx: &mut ToolContext<'_>, _event: PointerEvent) {}

    fn cancel(&mut self, _ctx: &mut ToolContext<'_>) {}

    fn options(&self) -> &'static [OptionKind] {
        &[OptionKind::Similarity]
    }
}

/// 4×4 ordered-dither thresholds.
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// The colour of a gradient from `from` to `to` at position `t` (0 to 1) for pixel `(x, y)`.
pub fn gradient_color(from: Rgba8, to: Rgba8, t: f32, x: i32, y: i32, dither: bool) -> Rgba8 {
    let t = t.clamp(0.0, 1.0);
    if dither {
        let threshold = f32::from(BAYER[(y & 3) as usize][(x & 3) as usize]) + 0.5;
        return if t * 16.0 > threshold { to } else { from };
    }
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Rgba8::new(
        mix(from.r, to.r),
        mix(from.g, to.g),
        mix(from.b, to.b),
        mix(from.a, to.a),
    )
}

/// Drag from where the first colour starts to where the second one is reached.
#[derive(Default)]
pub struct GradientTool {
    start: Point,
    colors: (Rgba8, Rgba8),
    stroke: Option<Stroke>,
    spare: StrokeBuffer,
}

impl GradientTool {
    fn redraw(&mut self, ctx: &mut ToolContext<'_>, end: Point) {
        let Some(stroke) = &mut self.stroke else {
            return;
        };
        let sprite = ctx.document.sprite();
        let before = stroke.reset();
        // Everything that may be painted: the selection if there is one, else the canvas.
        let area = sprite
            .selection()
            .map_or(sprite.bounds(), |mask| mask.bounds());
        let (ax, ay) = (self.start.x as f32, self.start.y as f32);
        let (dx, dy) = (end.x as f32 - ax, end.y as f32 - ay);
        let length = dx * dx + dy * dy;
        let (from, to) = self.colors;

        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let t = if length > 0.0 {
                    ((x as f32 - ax) * dx + (y as f32 - ay) * dy) / length
                } else {
                    0.0
                };
                let color = gradient_color(from, to, t, x, y, ctx.options.gradient_dither);
                stroke.plot(sprite, x, y, Ink::Simple(color));
            }
        }
        let after = stroke.buffer().bounds();
        ctx.document.add_damage(before.union(after));
    }
}

impl Tool for GradientTool {
    fn id(&self) -> &'static str {
        "gradient"
    }

    fn name(&self) -> &'static str {
        "Gradient"
    }

    fn pointer_down(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.cancel(ctx);
        self.colors = match event.button {
            PointerButton::Primary => (*ctx.primary, *ctx.secondary),
            PointerButton::Secondary => (*ctx.secondary, *ctx.primary),
        };
        self.stroke = begin(ctx, Ink::Erase, std::mem::take(&mut self.spare));
        self.start = event.position;
        self.redraw(ctx, event.position);
    }

    fn pointer_move(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.redraw(ctx, event.position);
    }

    fn pointer_up(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.redraw(ctx, event.position);
        if let Some(stroke) = self.stroke.take() {
            let covered = stroke.buffer().bounds();
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
        overlay_of(self.stroke.as_ref())
    }

    fn options(&self) -> &'static [OptionKind] {
        &[OptionKind::Gradient]
    }
}

/// How the modifier keys held when a selection starts combine it with the existing one.
pub fn combine_for(modifiers: Modifiers) -> Combine {
    match (modifiers.shift, modifiers.alt) {
        (true, true) => Combine::Intersect,
        (true, false) => Combine::Add,
        (false, true) => Combine::Subtract,
        (false, false) => Combine::Replace,
    }
}

/// Applies a newly made selection as one undo step.
fn apply_selection(ctx: &mut ToolContext<'_>, new: &Mask, how: Combine) {
    let sprite = ctx.document.sprite();
    let (width, height) = (sprite.width(), sprite.height());
    let current = sprite
        .selection()
        .map_or_else(|| Mask::empty(width, height), |mask| (**mask).clone());
    let combined = current.combined(new, how);
    // A selection of the right size is never refused.
    let _ = ctx
        .document
        .transact("Select", |tx| tx.set_selection(Some(combined)));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectKind {
    Rectangle,
    Lasso,
    Wand,
}

/// The three selection tools. Shift adds, Alt subtracts, both together intersect.
pub struct SelectTool {
    kind: SelectKind,
    how: Combine,
    /// The points dragged through so far; empty when idle.
    points: Vec<Point>,
}

impl SelectTool {
    pub fn rectangle() -> Self {
        Self::new(SelectKind::Rectangle)
    }

    pub fn lasso() -> Self {
        Self::new(SelectKind::Lasso)
    }

    pub fn wand() -> Self {
        Self::new(SelectKind::Wand)
    }

    fn new(kind: SelectKind) -> Self {
        Self {
            kind,
            how: Combine::Replace,
            points: Vec::new(),
        }
    }
}

impl Tool for SelectTool {
    fn id(&self) -> &'static str {
        match self.kind {
            SelectKind::Rectangle => "select_rectangle",
            SelectKind::Lasso => "lasso",
            SelectKind::Wand => "magic_wand",
        }
    }

    fn name(&self) -> &'static str {
        match self.kind {
            SelectKind::Rectangle => "Rectangle Select",
            SelectKind::Lasso => "Lasso",
            SelectKind::Wand => "Magic Wand",
        }
    }

    fn pointer_down(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.how = combine_for(event.modifiers);
        self.points.clear();
        if self.kind != SelectKind::Wand {
            self.points.push(event.position);
            return;
        }
        let document = &*ctx.document;
        let sprite = document.sprite();
        let layer = sprite.read_region(
            document.active_layer(),
            document.active_frame(),
            sprite.bounds(),
        );
        let options = ctx.options;
        let area = Mask::similar(
            &layer,
            event.position,
            options.tolerance,
            options.fill_contiguous,
        );
        apply_selection(ctx, &area, self.how);
    }

    fn pointer_move(&mut self, _ctx: &mut ToolContext<'_>, event: PointerEvent) {
        match (self.kind, self.points.len()) {
            (SelectKind::Wand, _) | (_, 0) => {}
            (SelectKind::Rectangle, _) => {
                self.points.truncate(1);
                self.points.push(event.position);
            }
            (SelectKind::Lasso, _) => {
                if self.points.last() != Some(&event.position) {
                    self.points.push(event.position);
                }
            }
        }
    }

    fn pointer_up(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.pointer_move(ctx, event);
        let points = std::mem::take(&mut self.points);
        let Some(&first) = points.first() else {
            return;
        };
        let sprite = ctx.document.sprite();
        let (width, height) = (sprite.width(), sprite.height());
        let dragged = points.iter().any(|&p| p != first);

        let new = match self.kind {
            SelectKind::Wand => return,
            // A plain click, without dragging, means "select nothing".
            _ if !dragged && self.how == Combine::Replace => Mask::empty(width, height),
            SelectKind::Rectangle => {
                let last = points.last().copied().unwrap_or(first);
                Mask::from_rect(width, height, rect_between(first, last))
            }
            SelectKind::Lasso => Mask::from_polygon(width, height, &points),
        };
        apply_selection(ctx, &new, self.how);
    }

    fn cancel(&mut self, _ctx: &mut ToolContext<'_>) {
        self.points.clear();
    }

    fn options(&self) -> &'static [OptionKind] {
        match self.kind {
            SelectKind::Wand => &[OptionKind::SelectionMode, OptionKind::Similarity],
            _ => &[OptionKind::SelectionMode],
        }
    }

    fn outline(&self) -> Option<Outline> {
        let (&first, &last) = (self.points.first()?, self.points.last()?);
        Some(match self.kind {
            SelectKind::Lasso => Outline::Path(self.points.clone()),
            _ => Outline::Rect(rect_between(first, last)),
        })
    }

    fn paints(&self) -> bool {
        false
    }
}

/// Drags the selected pixels of the layer (or, without a selection, all of them) somewhere
/// else. A selection moves along with its pixels.
#[derive(Default)]
pub struct MoveTool {
    start: Point,
    offset: (i32, i32),
    /// The pixels being moved, as they were when the drag began.
    lifted: Option<(Mask, Image)>,
    had_selection: bool,
    stroke: Option<Stroke>,
    spare: StrokeBuffer,
}

impl MoveTool {
    fn redraw(&mut self, ctx: &mut ToolContext<'_>, position: Point) {
        let (Some(stroke), Some((mask, pixels))) = (&mut self.stroke, &self.lifted) else {
            return;
        };
        let before = stroke.reset();
        let (dx, dy) = (position.x - self.start.x, position.y - self.start.y);
        self.offset = (dx, dy);
        let origin = mask.bounds().origin();
        // Where the pixels land first, then holes where they came from: the first write to
        // a pixel wins, so a landing spot inside the old area keeps its new content.
        for p in mask.pixels() {
            let color = pixels.get(p.x - origin.x, p.y - origin.y);
            if color.a != 0 {
                stroke.plot_unmasked(p.x + dx, p.y + dy, color);
            }
        }
        for p in mask.pixels() {
            stroke.plot_unmasked(p.x, p.y, Rgba8::TRANSPARENT);
        }
        let after = stroke.buffer().bounds();
        ctx.document.add_damage(before.union(after));
    }
}

impl Tool for MoveTool {
    fn id(&self) -> &'static str {
        "move"
    }

    fn name(&self) -> &'static str {
        "Move"
    }

    fn pointer_down(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.cancel(ctx);
        let Some(stroke) = begin(ctx, Ink::Erase, std::mem::take(&mut self.spare)) else {
            return;
        };
        let sprite = ctx.document.sprite();
        let Some(mask) = crate::edit::region(sprite, stroke.layer(), stroke.frame()) else {
            self.spare = stroke.cancel().0;
            return;
        };
        let pixels = sprite.read_region(stroke.layer(), stroke.frame(), mask.bounds());
        self.had_selection = sprite.selection().is_some();
        self.lifted = Some((mask, pixels));
        self.stroke = Some(stroke);
        self.start = event.position;
        self.offset = (0, 0);
    }

    fn pointer_move(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.redraw(ctx, event.position);
    }

    fn pointer_up(&mut self, ctx: &mut ToolContext<'_>, event: PointerEvent) {
        self.redraw(ctx, event.position);
        let (Some(stroke), Some((mask, _))) = (self.stroke.take(), self.lifted.take()) else {
            return;
        };
        let covered = stroke.buffer().bounds();
        let (dx, dy) = self.offset;
        let moved_selection = self.had_selection.then(|| mask.translated(dx, dy));
        let result = ctx.document.transact("Move", |tx| {
            let buffer = stroke.apply(tx)?;
            if let Some(selection) = moved_selection {
                tx.set_selection(Some(selection))?;
            }
            Ok::<_, crate::sprite::SpriteError>(buffer)
        });
        if let Ok(buffer) = result {
            self.spare = buffer;
        }
        ctx.document.add_damage(covered);
    }

    fn cancel(&mut self, ctx: &mut ToolContext<'_>) {
        self.lifted = None;
        if let Some(stroke) = self.stroke.take() {
            let (buffer, covered) = stroke.cancel();
            self.spare = buffer;
            ctx.document.add_damage(covered);
        }
    }

    fn overlay(&self) -> Option<Overlay<'_>> {
        overlay_of(self.stroke.as_ref())
    }

    fn outline(&self) -> Option<Outline> {
        let (mask, _) = self.lifted.as_ref()?;
        let (dx, dy) = self.offset;
        Some(Outline::Rect(mask.bounds().translated(dx, dy)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::editor::Editor;
    use crate::sprite::{LayerId, Sprite};
    use crate::stroke::{Brush, BrushShape};

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const GREEN: Rgba8 = Rgba8::rgb(0, 255, 0);
    const BLUE: Rgba8 = Rgba8::rgb(0, 0, 255);
    const L1: LayerId = LayerId(1);

    fn editor() -> Editor {
        let mut editor = Editor::new();
        editor.set_document(Some(Document::new(Sprite::new(16, 16).unwrap())));
        editor.primary = RED;
        editor.secondary = GREEN;
        editor
    }

    fn event(x: i32, y: i32, button: PointerButton, modifiers: Modifiers) -> PointerEvent {
        PointerEvent {
            position: Point::new(x, y),
            button,
            modifiers,
        }
    }

    fn drag_with(editor: &mut Editor, modifiers: Modifiers, path: &[(i32, i32)]) {
        let button = PointerButton::Primary;
        let (first, last) = (path[0], path[path.len() - 1]);
        editor.pointer_down(event(first.0, first.1, button, modifiers));
        for &(x, y) in &path[1..] {
            editor.pointer_move(event(x, y, button, modifiers));
        }
        editor.pointer_up(event(last.0, last.1, button, modifiers));
    }

    fn drag(editor: &mut Editor, path: &[(i32, i32)]) {
        drag_with(editor, Modifiers::default(), path);
    }

    fn use_tool(editor: &mut Editor, id: &str) {
        assert!(editor.set_active_tool(id), "no tool `{id}`");
    }

    fn sprite(editor: &Editor) -> &Sprite {
        editor.document().unwrap().sprite()
    }

    fn px(editor: &Editor, x: i32, y: i32) -> Rgba8 {
        sprite(editor).pixel(L1, 0, x, y)
    }

    fn steps(editor: &Editor) -> usize {
        editor.document().unwrap().history().len()
    }

    fn select(editor: &mut Editor, rect: Rect) {
        let mask = Mask::from_rect(16, 16, rect);
        let document = editor.document_mut().unwrap();
        document
            .transact("Select", |tx| tx.set_selection(Some(mask)))
            .unwrap();
    }

    fn rows(editor: &Editor, area: Rect) -> Vec<String> {
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| if px(editor, x, y).a != 0 { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn every_painting_tool_stays_inside_the_selection_and_is_one_undo_step() {
        let selection = Rect::new(4, 4, 6, 6);
        for tool in [
            "pencil",
            "eraser",
            "line",
            "rectangle",
            "ellipse",
            "bucket",
            "gradient",
        ] {
            let mut editor = editor();
            // Something to erase and to fill over: the whole canvas painted blue.
            editor.primary = BLUE;
            editor.brush = Brush::new(BrushShape::Square, 16);
            drag(&mut editor, &[(8, 8)]);
            editor.primary = RED;
            editor.brush = Brush::new(BrushShape::Round, 3);
            select(&mut editor, selection);

            let before = sprite(&editor).clone();
            let count = steps(&editor);
            use_tool(&mut editor, tool);
            drag(&mut editor, &[(1, 1), (9, 6), (7, 7)]);

            let after = sprite(&editor).clone();
            assert_eq!(steps(&editor), count + 1, "{tool}: not one undo step");
            let mut changed = 0;
            for y in 0..16 {
                for x in 0..16 {
                    if before.pixel(L1, 0, x, y) != after.pixel(L1, 0, x, y) {
                        changed += 1;
                        let inside = selection.contains(Point::new(x, y));
                        assert!(inside, "{tool} drew outside at {x}, {y}");
                    }
                }
            }
            assert!(changed > 0, "{tool} drew nothing");

            let document = editor.document_mut().unwrap();
            assert!(document.undo());
            assert_eq!(*document.sprite(), before, "{tool}: undo");
            assert!(document.redo());
            assert_eq!(*document.sprite(), after, "{tool}: redo");
        }
    }

    #[test]
    fn line_rectangle_and_ellipse_shapes() {
        let mut e = editor();
        use_tool(&mut e, "line");
        drag(&mut e, &[(1, 1), (9, 3), (6, 1)]);
        // Only the final line is drawn, not the positions passed on the way.
        assert_eq!(
            rows(&e, Rect::new(0, 0, 8, 3)),
            ["........", ".######.", "........"]
        );

        let mut e = editor();
        use_tool(&mut e, "rectangle");
        drag(&mut e, &[(5, 4), (1, 1)]);
        assert_eq!(
            rows(&e, Rect::new(0, 0, 7, 6)),
            [
                ".......", ".#####.", ".#...#.", ".#...#.", ".#####.", "......."
            ]
        );
        e.options.shape_filled = true;
        e.primary = BLUE;
        drag(&mut e, &[(2, 2), (4, 3)]);
        assert_eq!(
            (px(&e, 3, 2), px(&e, 4, 3), px(&e, 1, 2)),
            (BLUE, BLUE, RED)
        );

        let mut e = editor();
        use_tool(&mut e, "ellipse");
        drag(&mut e, &[(0, 0), (6, 4)]);
        assert_eq!(
            rows(&e, Rect::new(0, 0, 7, 5)),
            [".#####.", "#.....#", "#.....#", "#.....#", ".#####."]
        );
        e.options.shape_filled = true;
        drag(&mut e, &[(8, 8), (14, 12)]);
        assert_eq!(
            rows(&e, Rect::new(8, 8, 7, 5)),
            [".#####.", "#######", "#######", "#######", ".#####."]
        );
    }

    #[test]
    fn shift_makes_rectangles_and_ellipses_square() {
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let mut e = editor();
        use_tool(&mut e, "rectangle");
        drag_with(&mut e, shift, &[(2, 2), (9, 4)]);
        assert_eq!(sprite(&e).cel(L1, 0).unwrap().rect(), Rect::new(2, 2, 8, 8));
        assert_eq!(
            squared(Point::new(5, 5), Point::new(2, 6)),
            Point::new(2, 8)
        );
    }

    #[test]
    fn shapes_use_the_brush_and_the_button() {
        let mut e = editor();
        e.brush = Brush::new(BrushShape::Square, 3);
        use_tool(&mut e, "line");
        let button = PointerButton::Secondary;
        let none = Modifiers::default();
        e.pointer_down(event(4, 8, button, none));
        e.pointer_up(event(10, 8, button, none));
        assert_eq!(sprite(&e).cel(L1, 0).unwrap().rect(), Rect::new(3, 7, 9, 3));
        assert_eq!(px(&e, 6, 8), GREEN);
    }

    #[test]
    fn bucket_fills_the_touching_area_or_every_matching_pixel() {
        let mut e = editor();
        // A red frame splits the canvas into an inside and an outside.
        use_tool(&mut e, "rectangle");
        drag(&mut e, &[(4, 4), (11, 11)]);

        use_tool(&mut e, "bucket");
        e.primary = BLUE;
        drag(&mut e, &[(8, 8)]);
        assert_eq!((px(&e, 5, 5), px(&e, 10, 10)), (BLUE, BLUE));
        assert_eq!((px(&e, 4, 4), px(&e, 0, 0)), (RED, Rgba8::TRANSPARENT));

        // Not contiguous: the frame and nothing else, wherever it is.
        e.options.fill_contiguous = false;
        e.primary = GREEN;
        drag(&mut e, &[(4, 4)]);
        assert_eq!((px(&e, 11, 11), px(&e, 8, 8)), (GREEN, BLUE));

        // Filling with the colour that is already there changes nothing.
        let count = steps(&e);
        drag(&mut e, &[(4, 4)]);
        assert_eq!(steps(&e), count);
    }

    #[test]
    fn bucket_tolerance_takes_in_near_colours() {
        let nearly = Rgba8::rgb(110, 100, 100);
        let mut e = editor();
        e.primary = Rgba8::rgb(100, 100, 100);
        drag(&mut e, &[(0, 0), (7, 0)]);
        e.primary = nearly;
        drag(&mut e, &[(8, 0), (15, 0)]);

        use_tool(&mut e, "bucket");
        e.primary = BLUE;
        drag(&mut e, &[(2, 0)]);
        assert_eq!((px(&e, 7, 0), px(&e, 8, 0)), (BLUE, nearly));

        e.options.tolerance = 10;
        e.primary = Rgba8::rgb(105, 100, 100);
        drag(&mut e, &[(9, 0)]);
        assert_eq!(px(&e, 15, 0), Rgba8::rgb(105, 100, 100));
        assert_eq!(px(&e, 2, 0), BLUE);
    }

    #[test]
    fn gradient_dithered_or_smooth() {
        let mut e = editor();
        use_tool(&mut e, "gradient");
        drag(&mut e, &[(0, 0), (15, 0)]);
        let colors: std::collections::HashSet<Rgba8> = (0..16)
            .flat_map(|y| (0..16).map(move |x| (x, y)))
            .map(|(x, y)| px(&e, x, y))
            .collect();
        // Dithered: only the two colours, the first at the start and the second at the end.
        assert_eq!(colors, [RED, GREEN].into_iter().collect());
        assert!((0..16).all(|y| px(&e, 0, y) == RED));
        assert!((0..16).all(|y| px(&e, 15, y) == GREEN));
        let reds = |x: i32| (0..16).filter(|&y| px(&e, x, y) == RED).count();
        assert!(reds(4) > reds(11));

        e.options.gradient_dither = false;
        drag(&mut e, &[(0, 0), (0, 15)]);
        assert_eq!((px(&e, 7, 0), px(&e, 7, 15)), (RED, GREEN));
        let middle = px(&e, 7, 8);
        assert!(middle.r > 50 && middle.g > 50, "{middle:?}");
        assert_eq!(steps(&e), 2);

        assert_eq!(gradient_color(RED, GREEN, -3.0, 0, 0, false), RED);
        assert_eq!(gradient_color(RED, GREEN, 9.0, 0, 0, true), GREEN);
    }

    #[test]
    fn rectangle_select_with_modifiers() {
        let none = Modifiers::default();
        let shift = Modifiers {
            shift: true,
            ..none
        };
        let alt = Modifiers { alt: true, ..none };
        let both = Modifiers {
            shift: true,
            alt: true,
            ..none
        };
        let bounds = |e: &Editor| sprite(e).selection().map(|mask| mask.bounds());
        let count = |e: &Editor| {
            sprite(e)
                .selection()
                .map_or(0, |mask| mask.pixels().count())
        };

        let mut e = editor();
        use_tool(&mut e, "select_rectangle");
        e.pointer_down(event(2, 2, PointerButton::Primary, none));
        e.pointer_move(event(5, 4, PointerButton::Primary, none));
        assert_eq!(e.outline(), Some(Outline::Rect(Rect::new(2, 2, 4, 3))));
        assert_eq!(bounds(&e), None);
        e.pointer_up(event(5, 4, PointerButton::Primary, none));
        assert_eq!(bounds(&e), Some(Rect::new(2, 2, 4, 3)));
        assert_eq!(e.outline(), None);

        drag_with(&mut e, shift, &[(8, 8), (9, 9)]);
        assert_eq!((bounds(&e), count(&e)), (Some(Rect::new(2, 2, 8, 8)), 16));
        drag_with(&mut e, alt, &[(2, 2), (3, 15)]);
        assert_eq!(count(&e), 10);
        drag_with(&mut e, both, &[(0, 0), (5, 5)]);
        assert_eq!(bounds(&e), Some(Rect::new(4, 2, 2, 3)));

        // Each of those was an undo step; a plain click selects nothing.
        assert_eq!(steps(&e), 4);
        drag(&mut e, &[(12, 12)]);
        assert_eq!(bounds(&e), None);
        e.document_mut().unwrap().undo();
        assert_eq!(bounds(&e), Some(Rect::new(4, 2, 2, 3)));

        // Dragging off the canvas selects what is on it.
        drag(&mut e, &[(13, 13), (40, 40)]);
        assert_eq!(bounds(&e), Some(Rect::new(13, 13, 3, 3)));
    }

    #[test]
    fn lasso_and_magic_wand() {
        let mut e = editor();
        use_tool(&mut e, "lasso");
        drag(&mut e, &[(2, 2), (10, 2), (10, 10), (2, 10)]);
        let mask = sprite(&e).selection().unwrap();
        assert_eq!(mask.bounds(), Rect::new(2, 2, 9, 9));
        assert!(mask.contains(6, 6) && !mask.contains(12, 6));

        // The wand selects by the colours of the layer, here a red bar.
        let mut e = editor();
        e.brush = Brush::new(BrushShape::Square, 4);
        drag(&mut e, &[(3, 3), (9, 3)]);
        use_tool(&mut e, "magic_wand");
        drag(&mut e, &[(5, 3)]);
        assert_eq!(
            sprite(&e).selection().unwrap().bounds(),
            Rect::new(1, 1, 10, 4)
        );
        drag(&mut e, &[(14, 14)]);
        let outside = sprite(&e).selection().unwrap();
        assert!(outside.contains(0, 0) && !outside.contains(5, 3));
        // Selection tools do not paint, so nothing blocks them.
        assert_eq!(e.draw_block(), None);
    }

    #[test]
    fn move_takes_the_selection_and_its_pixels_along() {
        let none = Modifiers::default();
        let mut e = editor();
        e.brush = Brush::new(BrushShape::Square, 4);
        drag(&mut e, &[(3, 3), (9, 3)]);
        select(&mut e, Rect::new(1, 1, 4, 4));
        let before = sprite(&e).clone();
        let count = steps(&e);

        use_tool(&mut e, "move");
        e.pointer_down(event(2, 2, PointerButton::Primary, none));
        e.pointer_move(event(4, 9, PointerButton::Primary, none));
        // Still a preview: the sprite is untouched, the outline shows where it will land.
        assert_eq!(*sprite(&e), before);
        assert_eq!(e.outline(), Some(Outline::Rect(Rect::new(3, 8, 4, 4))));
        e.pointer_up(event(4, 9, PointerButton::Primary, none));

        assert_eq!(steps(&e), count + 1);
        assert_eq!(
            sprite(&e).selection().unwrap().bounds(),
            Rect::new(3, 8, 4, 4)
        );
        assert_eq!((px(&e, 3, 8), px(&e, 6, 11)), (RED, RED));
        let t = Rgba8::TRANSPARENT;
        assert_eq!((px(&e, 1, 1), px(&e, 4, 4)), (t, t));
        // What was not selected stayed.
        assert_eq!(px(&e, 5, 1), RED);

        let document = e.document_mut().unwrap();
        document.undo();
        assert_eq!(*document.sprite(), before);
    }

    #[test]
    fn move_without_a_selection_moves_the_whole_layer() {
        let none = Modifiers::default();
        let mut e = editor();
        drag(&mut e, &[(2, 2), (5, 2)]);
        use_tool(&mut e, "move");
        drag(&mut e, &[(8, 8), (10, 13)]);
        assert_eq!(sprite(&e).cel(L1, 0).unwrap().rect(), Rect::new(4, 7, 4, 1));
        assert!(sprite(&e).selection().is_none());

        // A click without movement is not an edit, and neither is a cancelled drag.
        let count = steps(&e);
        drag(&mut e, &[(8, 8)]);
        e.pointer_down(event(1, 1, PointerButton::Primary, none));
        e.pointer_move(event(9, 9, PointerButton::Primary, none));
        e.cancel_interaction();
        assert_eq!(steps(&e), count);
        assert_eq!(sprite(&e).cel(L1, 0).unwrap().rect(), Rect::new(4, 7, 4, 1));
    }

    #[test]
    fn moving_onto_other_pixels_covers_them_and_overlap_is_handled() {
        let mut e = editor();
        e.primary = BLUE;
        drag(&mut e, &[(0, 0), (7, 0)]);
        e.primary = RED;
        drag(&mut e, &[(0, 0), (3, 0)]);
        // Move the red half two pixels right: it overlaps its own old place.
        select(&mut e, Rect::new(0, 0, 4, 1));
        use_tool(&mut e, "move");
        drag(&mut e, &[(0, 0), (2, 0)]);
        let row: Vec<Rgba8> = (0..8).map(|x| px(&e, x, 0)).collect();
        let t = Rgba8::TRANSPARENT;
        assert_eq!(row, [t, t, RED, RED, RED, RED, BLUE, BLUE]);
    }

    #[test]
    fn several_documents_can_be_open() {
        let mut e = editor();
        let first = e.active_document().unwrap();
        drag(&mut e, &[(1, 1)]);
        let generation = e.generation();

        let second = e.add_document(Document::new(Sprite::new(4, 4).unwrap()));
        assert_eq!(e.active_document(), Some(second));
        assert_ne!(e.generation(), generation);
        assert_eq!(sprite(&e).width(), 4);
        assert_eq!(e.documents().len(), 2);

        assert!(e.set_active_document(first));
        assert_eq!(px(&e, 1, 1), RED);
        assert_eq!(e.document_by_id(second).unwrap().sprite().width(), 4);

        // Closing the active one moves to its neighbour; an unknown id does nothing.
        assert!(e.close_document(first).is_some());
        assert_eq!(e.active_document(), Some(second));
        assert!(e.close_document(first).is_none());
        assert!(e.close_document(second).is_some());
        assert_eq!(e.active_document(), None);
        assert!(e.document().is_none());
    }
}
