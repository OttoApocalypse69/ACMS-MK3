//! The editing state that is not part of a sprite itself: which documents are open, the
//! colours and brush in use, the tools and which one is active.
//!
//! This is the document-related half of what commands act on; the app adds its own UI state
//! around it.

use crate::color::Rgba8;
use crate::compositor::Overlay;
use crate::document::Document;
use crate::edit::Clip;
use crate::geom::Rect;
use crate::pattern::Pattern;
use crate::sprite::SpriteError;
use crate::stroke::Brush;
use crate::tool::{
    BrushTool, CursorHint, Eyedropper, OptionKind, Outline, PointerEvent, Tool, ToolContext,
    ToolOptions,
};
use crate::tools::{
    BucketTool, GradientTool, MoveTool, SelectTool, ShapeTool, SliceTool, TileStampTool,
};

/// Identifies an open document for as long as it is open. Never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocId(pub u64);

pub struct Editor {
    /// The open documents, in the order of their tabs.
    documents: Vec<(DocId, Document)>,
    /// Index into `documents` of the one being edited.
    active: usize,
    next_doc_id: u64,
    /// Goes up every time a different document becomes the active one, so views can tell
    /// that everything they cached about the old one is stale.
    generation: u64,
    pub primary: Rgba8,
    pub secondary: Rgba8,
    pub brush: Brush,
    pub options: ToolOptions,
    /// What Copy and Cut took; shared by all open documents.
    pub clipboard: Option<Clip>,
    /// The dither pattern the user made last, kept while a built-in one is in use.
    pub custom_pattern: Option<Pattern>,
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
            documents: Vec::new(),
            active: 0,
            next_doc_id: 1,
            generation: 0,
            primary: Rgba8::BLACK,
            secondary: Rgba8::WHITE,
            brush: Brush::default(),
            options: ToolOptions::default(),
            clipboard: None,
            custom_pattern: None,
            tools: vec![
                Box::new(BrushTool::pencil()),
                Box::new(BrushTool::eraser()),
                Box::new(Eyedropper),
                Box::new(ShapeTool::line()),
                Box::new(ShapeTool::rectangle()),
                Box::new(ShapeTool::ellipse()),
                Box::new(BucketTool::default()),
                Box::new(GradientTool::default()),
                Box::new(MoveTool::default()),
                Box::new(SelectTool::rectangle()),
                Box::new(SelectTool::lasso()),
                Box::new(SelectTool::wand()),
                Box::new(TileStampTool::default()),
                Box::new(SliceTool::default()),
            ],
            active_tool: 0,
            interacting: false,
        }
    }

    /// The document being edited.
    pub fn document(&self) -> Option<&Document> {
        self.documents
            .get(self.active)
            .map(|(_, document)| document)
    }

    /// The document being edited, for running transactions, undo and redo on it.
    pub fn document_mut(&mut self) -> Option<&mut Document> {
        self.cancel_interaction();
        self.documents
            .get_mut(self.active)
            .map(|(_, document)| document)
    }

    pub fn active_document(&self) -> Option<DocId> {
        self.documents.get(self.active).map(|(id, _)| *id)
    }

    /// Every open document, in tab order.
    pub fn documents(&self) -> impl ExactSizeIterator<Item = (DocId, &Document)> {
        self.documents.iter().map(|(id, document)| (*id, document))
    }

    pub fn document_by_id(&self, id: DocId) -> Option<&Document> {
        self.documents
            .iter()
            .find_map(|(doc_id, document)| (*doc_id == id).then_some(document))
    }

    /// Opens a document in a new tab and makes it the one being edited.
    pub fn add_document(&mut self, document: Document) -> DocId {
        self.cancel_interaction();
        let id = DocId(self.next_doc_id);
        self.next_doc_id += 1;
        self.documents.push((id, document));
        self.active = self.documents.len() - 1;
        self.generation += 1;
        id
    }

    /// Switches to another open document. Returns `false` if it is not open.
    pub fn set_active_document(&mut self, id: DocId) -> bool {
        let Some(index) = self.documents.iter().position(|(doc_id, _)| *doc_id == id) else {
            return false;
        };
        if index != self.active {
            self.cancel_interaction();
            self.active = index;
            self.generation += 1;
        }
        true
    }

    /// Closes a document. The tab to its left becomes active if it was the active one.
    pub fn close_document(&mut self, id: DocId) -> Option<Document> {
        let index = self
            .documents
            .iter()
            .position(|(doc_id, _)| *doc_id == id)?;
        self.cancel_interaction();
        let (_, document) = self.documents.remove(index);
        if self.active > index || self.active >= self.documents.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.generation += 1;
        Some(document)
    }

    /// Opens `document` in a new tab, or with `None` closes the active one.
    pub fn set_document(&mut self, document: Option<Document>) {
        match (document, self.active_document()) {
            (Some(document), _) => {
                self.add_document(document);
            }
            (None, Some(active)) => {
                self.close_document(active);
            }
            (None, None) => {}
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The canvas area that needs redrawing since the last call; empty if none does.
    /// Unlike [`Editor::document_mut`] this leaves a stroke in progress alone.
    pub fn take_damage(&mut self) -> Rect {
        self.documents
            .get_mut(self.active)
            .map_or(Rect::EMPTY, |(_, document)| document.take_damage())
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
        let Some((_, document)) = self.documents.get_mut(self.active) else {
            return;
        };
        let mut ctx = ToolContext {
            document,
            primary: &mut self.primary,
            secondary: &mut self.secondary,
            brush: self.brush.clone(),
            options: self.options.clone(),
        };
        action(self.tools[self.active_tool].as_mut(), &mut ctx);
    }

    /// A button went down on the canvas.
    pub fn pointer_down(&mut self, event: PointerEvent) {
        if self.document().is_some() {
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

    /// Why the active tool has no effect right now, if that is the case: it paints, and the
    /// selected layer is a group, locked or hidden. `None` when it works, and when no sprite
    /// is open.
    pub fn draw_block(&self) -> Option<SpriteError> {
        if !self.tools[self.active_tool].paints() {
            return None;
        }
        let document = self.document()?;
        let layer = document.active_layer();
        if let Err(block) = document
            .sprite()
            .check_drawable(layer, document.active_frame())
        {
            return Some(block);
        }
        // Tilemaps take the tile stamp, and pixel tools only in auto mode; other layers take
        // everything but the tile stamp.
        let on_tilemap = document.sprite().tileset_of(layer).is_some();
        let stamping = self.active_tool() == "tile_stamp";
        match (on_tilemap, stamping) {
            (false, true) => Some(SpriteError::NotATilemap(layer.0)),
            (true, false) if !self.options.tile_auto => Some(SpriteError::StampMode),
            _ => None,
        }
    }

    /// The stroke being drawn right now, to be shown on top of the sprite.
    pub fn overlay(&self) -> Option<Overlay<'_>> {
        self.tools[self.active_tool].overlay()
    }

    pub fn cursor_hint(&self) -> CursorHint {
        self.tools[self.active_tool].cursor_hint()
    }

    /// The settings the active tool uses.
    pub fn tool_options(&self) -> &'static [OptionKind] {
        self.tools[self.active_tool].options()
    }

    /// The shape the active tool is dragging out, if any.
    pub fn outline(&self) -> Option<Outline> {
        self.tools[self.active_tool].outline()
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
        assert_eq!(
            ids,
            [
                "pencil",
                "eraser",
                "eyedropper",
                "line",
                "rectangle",
                "ellipse",
                "bucket",
                "gradient",
                "move",
                "select_rectangle",
                "lasso",
                "magic_wand",
                "tile_stamp",
                "slice"
            ]
        );
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
    /// The painted pixels of the first layer, sorted.
    fn painted(editor: &Editor) -> Vec<(i32, i32)> {
        let sprite = editor.document().unwrap().sprite();
        let bounds = sprite.bounds();
        let mut pixels = Vec::new();
        for y in 0..bounds.h as i32 {
            for x in 0..bounds.w as i32 {
                if sprite.pixel(L1, 0, x, y).a != 0 {
                    pixels.push((x, y));
                }
            }
        }
        pixels
    }

    /// For tests that already have a variable called editor.
    fn new_editor() -> Editor {
        editor()
    }

    fn large_editor(size: u32) -> Editor {
        let mut editor = Editor::new();
        editor.add_document(Document::new(Sprite::new(size, size).unwrap()));
        editor.primary = RED;
        editor.secondary = GREEN;
        editor
    }

    /// The "done when" of Phase 4: symmetry, the pixel-perfect filter and the dither ink in
    /// one pencil stroke.
    #[test]
    fn symmetry_pixel_perfect_and_dither_work_together_in_one_stroke() {
        use crate::pattern::Pattern;
        use crate::sprite::Symmetry;
        use crate::tool::InkKind;

        const SIZE: i32 = 32;
        // Down and to the right all the way, with a step at every joint.
        let path = [
            (2, 2),
            (3, 2),
            (3, 3),
            (6, 3),
            (6, 4),
            (9, 9),
            (12, 9),
            (12, 10),
            (14, 14),
        ];

        let mut editor = large_editor(SIZE as u32);
        let before = editor.document().unwrap().sprite().clone();
        editor.document_mut().unwrap().set_symmetry(Symmetry::Both);
        let with_setting = editor.document().unwrap().sprite().clone();
        editor.options.pixel_perfect = true;
        editor.options.ink = InkKind::Dither;
        editor.options.pattern = Pattern::Checker;
        drag(&mut editor, PointerButton::Primary, &path);

        let pixels = painted(&editor);
        let has = |x: i32, y: i32| pixels.contains(&(x, y));

        // Symmetry: every pixel has its three mirror images.
        assert!(!pixels.is_empty());
        for &(x, y) in &pixels {
            assert!(has(SIZE - 1 - x, y), "({x}, {y}) has no left-right image");
            assert!(has(x, SIZE - 1 - y), "({x}, {y}) has no top-bottom image");
        }
        let quadrant: Vec<(i32, i32)> = pixels
            .iter()
            .copied()
            .filter(|&(x, y)| x < SIZE / 2 && y < SIZE / 2)
            .collect();
        assert_eq!(pixels.len(), quadrant.len() * 4);

        // Pixel-perfect: the line still joins both ends without a break, but no pixel is the
        // corner of an L.
        assert!(has(2, 2) && has(14, 14));
        for &(x, y) in &quadrant {
            let beside = has(x - 1, y) || has(x + 1, y);
            let above_or_below = has(x, y - 1) || has(x, y + 1);
            assert!(!(beside && above_or_below), "({x}, {y}) is a corner");
        }
        for &(x, y) in &quadrant {
            let neighbours = quadrant
                .iter()
                .filter(|&&(ox, oy)| {
                    (ox, oy) != (x, y) && (ox - x).abs() <= 1 && (oy - y).abs() <= 1
                })
                .count();
            let is_end = (x, y) == (2, 2) || (x, y) == (14, 14);
            assert_eq!(neighbours, if is_end { 1 } else { 2 }, "({x}, {y})");
        }
        // Dither: both colours, laid out by the pattern at each pixel's own position.
        let sprite = editor.document().unwrap().sprite();
        for &(x, y) in &pixels {
            let expected = if (x + y) % 2 == 0 { RED } else { GREEN };
            assert_eq!(sprite.pixel(L1, 0, x, y), expected, "({x}, {y})");
        }
        let reds = pixels.iter().filter(|(x, y)| (x + y) % 2 == 0).count();
        assert!(reds > 0 && reds < pixels.len());

        // One stroke, one undo step.
        let document = editor.document_mut().unwrap();
        assert_eq!(document.history().collect::<Vec<_>>(), ["Pencil"]);
        let after = document.sprite().clone();
        document.undo();
        assert_eq!(*document.sprite(), with_setting);
        assert_ne!(*document.sprite(), before);
        document.redo();
        assert_eq!(*document.sprite(), after);

        // Without the filter the same stroke keeps its corners.
        let mut plain = large_editor(SIZE as u32);
        drag(&mut plain, PointerButton::Primary, &path);
        assert!(painted(&plain).len() > quadrant.len());
    }

    #[test]
    fn the_inks_follow_the_button_that_draws() {
        use crate::pattern::Pattern;
        use crate::sprite::Palette;
        use crate::tool::InkKind;

        // Dither: the two colours swap roles for the secondary button.
        let mut editor = editor();
        editor.options.ink = InkKind::Dither;
        editor.options.pattern = Pattern::Checker;
        drag(&mut editor, PointerButton::Primary, &[(0, 0), (3, 0)]);
        drag(&mut editor, PointerButton::Secondary, &[(0, 1), (3, 1)]);
        let row =
            |editor: &Editor, y| -> Vec<Rgba8> { (0..4).map(|x| pixel(editor, x, y)).collect() };
        assert_eq!(row(&editor, 0), [RED, GREEN, RED, GREEN]);
        assert_eq!(row(&editor, 1), [RED, GREEN, RED, GREEN]);

        // Shade: forwards along the sprite's ramp with one button, back with the other.
        let blue = Rgba8::rgb(0, 0, 255);
        let mut editor = editor_with_ramp(vec![RED, GREEN, blue]);
        editor.options.ink = InkKind::Simple;
        editor.primary = GREEN;
        drag(&mut editor, PointerButton::Primary, &[(0, 0), (3, 0)]);
        editor.options.ink = InkKind::Shade;
        drag(&mut editor, PointerButton::Primary, &[(0, 0), (1, 0)]);
        drag(&mut editor, PointerButton::Secondary, &[(3, 0)]);
        assert_eq!(row(&editor, 0), [blue, blue, GREEN, RED]);
        // Where there is nothing to shade, nothing happens.
        let steps = editor.document().unwrap().history().len();
        drag(&mut editor, PointerButton::Primary, &[(0, 5), (9, 5)]);
        assert_eq!(editor.document().unwrap().history().len(), steps);

        fn editor_with_ramp(ramp: Vec<Rgba8>) -> Editor {
            let mut editor = new_editor();
            let palette = Palette::from_colors(ramp.clone()).with_ramps(vec![ramp]);
            editor
                .document_mut()
                .unwrap()
                .transact("Palette", |tx| {
                    tx.set_palette(palette);
                    Ok::<_, SpriteError>(())
                })
                .unwrap();
            editor
        }
    }

    #[test]
    fn symmetry_applies_to_drawing_tools_but_not_to_fills() {
        use crate::sprite::Symmetry;

        let mut editor = editor();
        editor
            .document_mut()
            .unwrap()
            .set_symmetry(Symmetry::Horizontal);

        assert!(editor.set_active_tool("line"));
        drag(&mut editor, PointerButton::Primary, &[(1, 1), (3, 1)]);
        assert_eq!(
            painted(&editor),
            [(1, 1), (2, 1), (3, 1), (12, 1), (13, 1), (14, 1)]
        );
        assert!(editor.set_active_tool("eraser"));
        drag(&mut editor, PointerButton::Primary, &[(1, 1)]);
        assert_eq!(painted(&editor), [(2, 1), (3, 1), (12, 1), (13, 1)]);

        // The bucket fills the area clicked, once.
        assert!(editor.set_active_tool("bucket"));
        drag(&mut editor, PointerButton::Primary, &[(2, 1)]);
        assert_eq!(pixel(&editor, 2, 1), RED);
        editor.primary = GREEN;
        drag(&mut editor, PointerButton::Primary, &[(2, 1)]);
        assert_eq!(pixel(&editor, 3, 1), GREEN);
        assert_eq!(pixel(&editor, 12, 1), RED);
    }

    #[test]
    fn tiled_mode_and_custom_brushes_reach_the_tools() {
        use crate::image::Image;

        let mut editor = editor();
        editor.options.tiled = true;
        drag(&mut editor, PointerButton::Primary, &[(14, 2), (17, 2)]);
        assert_eq!(painted(&editor), [(0, 2), (1, 2), (14, 2), (15, 2)]);

        let mut editor = new_editor();
        let mut image = Image::new(3, 1);
        image.set(0, 0, GREEN);
        image.set(2, 0, RED);
        editor.brush = Brush::custom(&image).unwrap();
        editor.primary = Rgba8::BLACK;
        drag(&mut editor, PointerButton::Primary, &[(5, 5)]);
        assert_eq!(pixel(&editor, 4, 5), GREEN);
        assert_eq!(pixel(&editor, 5, 5), Rgba8::TRANSPARENT);
        assert_eq!(pixel(&editor, 6, 5), RED);
    }
    #[test]
    fn tools_draw_on_the_frame_being_shown() {
        let mut editor = editor();
        let document = editor.document_mut().unwrap();
        document
            .transact("Frame", |tx| tx.add_frame(1, None))
            .unwrap();
        assert!(document.set_active_frame(1));

        drag(&mut editor, PointerButton::Primary, &[(1, 1), (4, 1)]);
        let sprite = editor.document().unwrap().sprite();
        assert_eq!(sprite.pixel(L1, 1, 2, 1), RED);
        assert!(sprite.cel(L1, 0).is_none());

        // The eyedropper sees the shown frame too, and the bucket fills it.
        editor.set_active_tool("eyedropper");
        editor.primary = Rgba8::BLACK;
        drag(&mut editor, PointerButton::Primary, &[(2, 1)]);
        assert_eq!(editor.primary, RED);
        editor.document_mut().unwrap().set_active_frame(0);
        editor.primary = GREEN;
        drag(&mut editor, PointerButton::Primary, &[(2, 1)]);
        assert_eq!(editor.primary, GREEN);
        editor.set_active_tool("bucket");
        drag(&mut editor, PointerButton::Primary, &[(8, 8)]);
        let sprite = editor.document().unwrap().sprite();
        assert_eq!(sprite.pixel(L1, 0, 0, 0), GREEN);
        assert_eq!(sprite.pixel(L1, 1, 0, 0), Rgba8::TRANSPARENT);
    }
}
