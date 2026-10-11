//! The editing state that is not part of the sprite itself: which document is open, the
//! colours and brush in use, the tools and which one is active.
//!
//! This is the document-related half of what commands act on; the app adds its own UI state
//! around it.

use crate::color::Rgba8;
use crate::compositor::Overlay;
use crate::document::Document;
use crate::geom::Rect;
use crate::sprite::SpriteError;
use crate::stroke::Brush;
use crate::tool::{BrushTool, CursorHint, Eyedropper, PointerEvent, Tool, ToolContext};

pub struct Editor {
    document: Option<Document>,
    /// Goes up every time a different document is put in place, so views can tell that
    /// everything they cached about the old one is stale.
    generation: u64,
    pub primary: Rgba8,
    pub secondary: Rgba8,
    pub brush: Brush,
    /// The tool registry.
    tools: Vec<Box<dyn Tool>>,
    active_tool: usize,
    /// Whether the active tool is in the middle of a pointer interaction.
    interacting: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    /// An editor with the built-in tools and no document.
    pub fn new() -> Self {
        Self {
            document: None,
            generation: 0,
            primary: Rgba8::BLACK,
            secondary: Rgba8::WHITE,
            brush: Brush::default(),
            tools: vec![
                Box::new(BrushTool::pencil()),
                Box::new(BrushTool::eraser()),
                Box::new(Eyedropper),
            ],
            active_tool: 0,
            interacting: false,
        }
    }

    pub fn document(&self) -> Option<&Document> {
        self.document.as_ref()
    }

    /// The open document, for running transactions, undo and redo on it.
    pub fn document_mut(&mut self) -> Option<&mut Document> {
        self.cancel_interaction();
        self.document.as_mut()
    }

    /// Replaces the open document (or closes it, with `None`).
    pub fn set_document(&mut self, document: Option<Document>) {
        self.cancel_interaction();
        self.document = document;
        self.generation += 1;
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The canvas area that needs redrawing since the last call; empty if none does.
    /// Unlike [`Editor::document_mut`] this leaves a stroke in progress alone.
    pub fn take_damage(&mut self) -> Rect {
        self.document
            .as_mut()
            .map_or(Rect::EMPTY, Document::take_damage)
    }

    /// Ids and names of all tools, in registration order.
    pub fn tools(&self) -> impl Iterator<Item = (&'static str, &'static str)> {
        self.tools.iter().map(|tool| (tool.id(), tool.name()))
    }

    pub fn active_tool(&self) -> &'static str {
        self.tools[self.active_tool].id()
    }

    /// Switches tool, abandoning any stroke in progress. Returns `false` for an unknown id.
    pub fn set_active_tool(&mut self, id: &str) -> bool {
        let Some(index) = self.tools.iter().position(|tool| tool.id() == id) else {
            return false;
        };
        self.cancel_interaction();
        self.active_tool = index;
        true
    }

    pub fn swap_colors(&mut self) {
        std::mem::swap(&mut self.primary, &mut self.secondary);
    }

    /// Runs `action` on the active tool. Does nothing without a document.
    fn with_tool(&mut self, action: impl FnOnce(&mut dyn Tool, &mut ToolContext<'_>)) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let mut ctx = ToolContext {
            document,
            primary: &mut self.primary,
            secondary: &mut self.secondary,
            brush: self.brush,
        };
        action(self.tools[self.active_tool].as_mut(), &mut ctx);
    }

    /// A button went down on the canvas.
    pub fn pointer_down(&mut self, event: PointerEvent) {
        if self.document.is_some() {
            self.interacting = true;
            self.with_tool(|tool, ctx| tool.pointer_down(ctx, event));
        }
    }

    /// The pointer moved while the button is held.
    pub fn pointer_move(&mut self, event: PointerEvent) {
        if self.interacting {
            self.with_tool(|tool, ctx| tool.pointer_move(ctx, event));
        }
    }

    /// The button was released.
    pub fn pointer_up(&mut self, event: PointerEvent) {
        if std::mem::take(&mut self.interacting) {
            self.with_tool(|tool, ctx| tool.pointer_up(ctx, event));
        }
    }

    /// Abandons the stroke (or other interaction) in progress, if any.
    pub fn cancel_interaction(&mut self) {
        if std::mem::take(&mut self.interacting) {
            self.with_tool(|tool, ctx| tool.cancel(ctx));
        }
    }

    pub fn is_interacting(&self) -> bool {
        self.interacting
    }

    /// Why drawing tools have no effect right now, if that is the case: the selected layer is
    /// a group, locked or hidden. `None` when drawing works, and when no sprite is open.
    pub fn draw_block(&self) -> Option<SpriteError> {
        let document = self.document.as_ref()?;
        document
            .sprite()
            .check_drawable(document.active_layer(), document.active_frame())
            .err()
    }

    /// The stroke being drawn right now, to be shown on top of the sprite.
    pub fn overlay(&self) -> Option<Overlay<'_>> {
        self.tools[self.active_tool].overlay()
    }

    pub fn cursor_hint(&self) -> CursorHint {
        self.tools[self.active_tool].cursor_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;
    use crate::sprite::{LayerId, Sprite};
    use crate::stroke::BrushShape;
    use crate::tool::{Modifiers, PointerButton};

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const GREEN: Rgba8 = Rgba8::rgb(0, 255, 0);
    const L1: LayerId = LayerId(1);

    fn editor() -> Editor {
        let mut editor = Editor::new();
        editor.set_document(Some(Document::new(Sprite::new(16, 16).unwrap())));
        editor.primary = RED;
        editor.secondary = GREEN;
        editor
    }

    fn at(x: i32, y: i32, button: PointerButton) -> PointerEvent {
        PointerEvent {
            position: Point::new(x, y),
            button,
            modifiers: Modifiers::default(),
        }
    }

    fn drag(editor: &mut Editor, button: PointerButton, path: &[(i32, i32)]) {
        let (first, rest) = path.split_first().unwrap();
        editor.pointer_down(at(first.0, first.1, button));
        for &(x, y) in rest {
            editor.pointer_move(at(x, y, button));
        }
        let last = path.last().unwrap();
        editor.pointer_up(at(last.0, last.1, button));
    }

    fn pixel(editor: &Editor, x: i32, y: i32) -> Rgba8 {
        editor.document().unwrap().sprite().pixel(L1, 0, x, y)
    }

    #[test]
    fn built_in_tools_are_registered_and_pencil_is_active() {
        let editor = Editor::new();
        let ids: Vec<&str> = editor.tools().map(|(id, _)| id).collect();
        assert_eq!(ids, ["pencil", "eraser", "eyedropper"]);
        assert_eq!(editor.active_tool(), "pencil");
    }

    #[test]
    fn pencil_draws_with_primary_or_secondary_colour_as_one_undo_step() {
        let mut editor = editor();
        drag(&mut editor, PointerButton::Primary, &[(1, 1), (5, 1)]);
        drag(&mut editor, PointerButton::Secondary, &[(1, 3), (5, 3)]);

        assert_eq!(pixel(&editor, 3, 1), RED);
        assert_eq!(pixel(&editor, 3, 3), GREEN);
        let doc = editor.document().unwrap();
        assert_eq!(doc.history().collect::<Vec<_>>(), ["Pencil", "Pencil"]);
    }

    #[test]
    fn the_stroke_is_only_a_preview_until_the_button_is_released() {
        let mut editor = editor();
        editor.pointer_down(at(2, 2, PointerButton::Primary));
        editor.pointer_move(at(6, 2, PointerButton::Primary));

        assert_eq!(pixel(&editor, 4, 2), Rgba8::TRANSPARENT);
        assert_eq!(editor.overlay().unwrap().buffer.get(4, 2), Some(RED));
        assert!(editor.is_interacting());

        editor.pointer_up(at(6, 2, PointerButton::Primary));
        assert_eq!(pixel(&editor, 4, 2), RED);
        assert!(editor.overlay().is_none());
        assert!(!editor.is_interacting());
    }

    #[test]
    fn eraser_erases_with_the_brush_size() {
        let mut editor = editor();
        editor.brush = Brush::new(BrushShape::Square, 16);
        drag(&mut editor, PointerButton::Primary, &[(8, 8)]);
        assert_eq!(pixel(&editor, 0, 0), RED);

        assert!(editor.set_active_tool("eraser"));
        editor.brush = Brush::new(BrushShape::Square, 2);
        drag(&mut editor, PointerButton::Primary, &[(1, 1), (14, 1)]);
        assert_eq!(pixel(&editor, 7, 0), Rgba8::TRANSPARENT);
        assert_eq!(pixel(&editor, 7, 1), Rgba8::TRANSPARENT);
        assert_eq!(pixel(&editor, 7, 2), RED);
        assert_eq!(pixel(&editor, 15, 0), RED);

        let doc = editor.document().unwrap();
        assert_eq!(doc.history().collect::<Vec<_>>(), ["Pencil", "Eraser"]);
    }

    #[test]
    fn eyedropper_picks_into_either_colour_and_ignores_empty_pixels() {
        let mut editor = editor();
        drag(&mut editor, PointerButton::Primary, &[(1, 1)]);
        drag(&mut editor, PointerButton::Secondary, &[(3, 1)]);
        editor.primary = Rgba8::BLACK;
        editor.secondary = Rgba8::BLACK;

        assert!(editor.set_active_tool("eyedropper"));
        drag(&mut editor, PointerButton::Primary, &[(3, 1)]);
        assert_eq!(editor.primary, GREEN);
        drag(&mut editor, PointerButton::Secondary, &[(1, 1)]);
        assert_eq!(editor.secondary, RED);

        // Empty pixels and positions off the canvas leave the colours alone.
        drag(&mut editor, PointerButton::Primary, &[(10, 10), (-5, 40)]);
        assert_eq!(editor.primary, GREEN);
        // Picking does not touch the document.
        assert_eq!(editor.document().unwrap().history().len(), 2);
    }

    #[test]
    fn cancelling_or_switching_tool_abandons_the_stroke() {
        let mut editor = editor();
        editor.pointer_down(at(2, 2, PointerButton::Primary));
        editor.pointer_move(at(6, 2, PointerButton::Primary));
        editor.cancel_interaction();
        editor.pointer_up(at(6, 2, PointerButton::Primary));
        assert_eq!(editor.document().unwrap().sprite().cels().count(), 0);
        assert!(editor.overlay().is_none());

        editor.pointer_down(at(2, 2, PointerButton::Primary));
        assert!(editor.set_active_tool("eraser"));
        editor.pointer_up(at(2, 2, PointerButton::Primary));
        assert_eq!(editor.document().unwrap().sprite().cels().count(), 0);
        assert!(!editor.set_active_tool("nope"));
        assert_eq!(editor.active_tool(), "eraser");
    }

    #[test]
    fn previews_and_commits_report_damage() {
        let mut editor = editor();

        editor.pointer_down(at(2, 2, PointerButton::Primary));
        assert_eq!(editor.take_damage(), Rect::new(2, 2, 1, 1));
        editor.pointer_move(at(5, 2, PointerButton::Primary));
        assert_eq!(editor.take_damage(), Rect::new(2, 2, 4, 1));
        // Taking damage must not disturb the stroke.
        assert!(editor.is_interacting());
        editor.cancel_interaction();
        assert_eq!(editor.take_damage(), Rect::new(2, 2, 4, 1));
        assert!(editor.take_damage().is_empty());
    }

    #[test]
    fn tools_draw_on_the_selected_layer_and_respect_its_state() {
        use crate::sprite::Slot;

        let mut editor = editor();
        let document = editor.document_mut().unwrap();
        let upper = document
            .transact("Add", |tx| tx.add_layer(Slot::new(None, 1), "Upper", false))
            .unwrap();
        let group = document
            .transact("Add", |tx| tx.add_layer(Slot::new(None, 2), "Group", true))
            .unwrap();
        document.set_active_layer(upper);
        assert_eq!(editor.draw_block(), None);

        drag(&mut editor, PointerButton::Primary, &[(1, 1), (5, 1)]);
        let sprite = editor.document().unwrap().sprite();
        assert_eq!(sprite.pixel(upper, 0, 3, 1), RED);
        assert_eq!(sprite.pixel(L1, 0, 3, 1), Rgba8::TRANSPARENT);

        // A group has no pixels: nothing is drawn and nothing is recorded.
        let steps = editor.document().unwrap().history().len();
        editor.document_mut().unwrap().set_active_layer(group);
        assert_eq!(
            editor.draw_block(),
            Some(SpriteError::NotAnImageLayer(group.0))
        );
        drag(&mut editor, PointerButton::Primary, &[(1, 4), (5, 4)]);
        assert_eq!(editor.document().unwrap().history().len(), steps);
        assert!(editor.overlay().is_none());

        // Locked, then hidden.
        let document = editor.document_mut().unwrap();
        document.set_active_layer(upper);
        let set = |document: &mut Document, locked: bool, visible: bool| {
            document
                .transact("Props", |tx| {
                    tx.update_layer(upper, |props| {
                        props.visible = visible;
                        if let Some(image) = &mut props.image {
                            image.locked = locked;
                        }
                    })
                })
                .unwrap();
        };
        set(document, true, true);
        assert_eq!(editor.draw_block(), Some(SpriteError::LayerLocked(upper.0)));
        drag(&mut editor, PointerButton::Primary, &[(1, 6), (5, 6)]);
        assert_eq!(pixel_on(&editor, upper, 3, 6), Rgba8::TRANSPARENT);

        set(editor.document_mut().unwrap(), false, false);
        assert_eq!(editor.draw_block(), Some(SpriteError::LayerHidden(upper.0)));
        drag(&mut editor, PointerButton::Primary, &[(1, 6), (5, 6)]);
        assert_eq!(pixel_on(&editor, upper, 3, 6), Rgba8::TRANSPARENT);

        set(editor.document_mut().unwrap(), false, true);
        drag(&mut editor, PointerButton::Primary, &[(1, 6), (5, 6)]);
        assert_eq!(pixel_on(&editor, upper, 3, 6), RED);
    }

    #[test]
    fn the_eyedropper_sees_all_layers_together() {
        use crate::sprite::Slot;

        let mut editor = editor();
        drag(&mut editor, PointerButton::Primary, &[(1, 1), (5, 1)]);
        let document = editor.document_mut().unwrap();
        let upper = document
            .transact("Add", |tx| tx.add_layer(Slot::new(None, 1), "Upper", false))
            .unwrap();
        document.set_active_layer(upper);
        editor.primary = GREEN;
        drag(&mut editor, PointerButton::Primary, &[(3, 1)]);

        // The upper layer is selected, but the eyedropper picks what is seen at each pixel.
        editor.set_active_tool("eyedropper");
        editor.primary = Rgba8::BLACK;
        drag(&mut editor, PointerButton::Primary, &[(1, 1)]);
        assert_eq!(editor.primary, RED);
        drag(&mut editor, PointerButton::Primary, &[(3, 1)]);
        assert_eq!(editor.primary, GREEN);
    }

    fn pixel_on(editor: &Editor, layer: LayerId, x: i32, y: i32) -> Rgba8 {
        editor.document().unwrap().sprite().pixel(layer, 0, x, y)
    }

    #[test]
    fn tools_do_nothing_without_a_document() {
        let mut editor = Editor::new();
        drag(&mut editor, PointerButton::Primary, &[(1, 1), (5, 1)]);
        assert!(editor.document().is_none());
        assert!(!editor.is_interacting());
    }

    #[test]
    fn replacing_the_document_bumps_the_generation() {
        let mut editor = Editor::new();
        let before = editor.generation();
        editor.set_document(Some(Document::new(Sprite::new(4, 4).unwrap())));
        assert_ne!(editor.generation(), before);
    }
}
