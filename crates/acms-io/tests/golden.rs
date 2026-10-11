//! Golden image tests (see `PLAN.md`, section 7).
//!
//! Each test drives a tool with a scripted stroke and compares the flattened result with a PNG
//! stored in `tests/golden/` at the workspace root.
//!
//! When a tool is changed on purpose, regenerate the images and look at them before
//! committing:
//!
//! ```text
//! ACMS_UPDATE_GOLDEN=1 cargo test -p acms-io --test golden
//! ```

use std::path::PathBuf;

use acms_core::color::Rgba8;
use acms_core::compositor;
use acms_core::document::Document;
use acms_core::editor::Editor;
use acms_core::geom::Point;
use acms_core::sprite::Sprite;
use acms_core::stroke::{Brush, BrushShape};
use acms_core::tool::{Modifiers, PointerButton, PointerEvent};
use acms_io::png::{decode_png, encode_png};

const RED: Rgba8 = Rgba8::rgb(0xC2, 0x29, 0x3A);
const BLUE: Rgba8 = Rgba8::rgb(0x3B, 0x6F, 0xE0);
const YELLOW: Rgba8 = Rgba8::rgb(0xF2, 0xCE, 0x3B);

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

/// A 32×32 sprite with the pencil selected, red as primary and blue as secondary colour.
fn editor() -> Editor {
    let mut editor = Editor::new();
    editor.set_document(Some(Document::new(Sprite::new(32, 32).unwrap())));
    editor.primary = RED;
    editor.secondary = BLUE;
    editor
}

/// Presses `button` at the first point, drags through the rest, releases at the last.
fn drag(editor: &mut Editor, button: PointerButton, path: &[(i32, i32)]) {
    let event = |&(x, y): &(i32, i32)| PointerEvent {
        position: Point::new(x, y),
        button,
        modifiers: Modifiers::default(),
    };
    let (first, rest) = path.split_first().expect("a stroke needs a point");
    editor.pointer_down(event(first));
    for point in rest {
        editor.pointer_move(event(point));
    }
    editor.pointer_up(event(path.last().unwrap_or(first)));
}

fn primary(editor: &mut Editor, path: &[(i32, i32)]) {
    drag(editor, PointerButton::Primary, path);
}

fn check(name: &str, editor: &Editor) {
    let sprite = editor.document().expect("a document is open").sprite();
    let actual = compositor::compose(sprite, 0);
    let path = golden_dir().join(format!("{name}.png"));

    let write = |path: &PathBuf| {
        let mut bytes = Vec::new();
        encode_png(&actual, &mut bytes).unwrap();
        std::fs::create_dir_all(golden_dir()).unwrap();
        std::fs::write(path, bytes).unwrap();
    };

    if std::env::var_os("ACMS_UPDATE_GOLDEN").is_some() {
        write(&path);
        return;
    }

    let bytes = std::fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "cannot read {}: {err}. Run with ACMS_UPDATE_GOLDEN=1 to create it.",
            path.display()
        )
    });
    if decode_png(&bytes).unwrap() != actual {
        let actual_path = golden_dir().join(format!("{name}.actual.png"));
        write(&actual_path);
        panic!(
            "`{name}` no longer matches its golden image. The new result was written to {}.",
            actual_path.display()
        );
    }
}

#[test]
fn pencil_one_pixel_line_through_far_apart_samples() {
    let mut editor = editor();
    primary(&mut editor, &[(2, 2), (29, 6), (4, 14), (28, 28), (2, 29)]);
    check("pencil_line", &editor);
}

#[test]
fn pencil_brush_shapes_and_sizes() {
    let mut editor = editor();
    // A row of single stamps: round 1 to 8.
    let mut x = 2;
    for size in 1..=8 {
        editor.brush = Brush::new(BrushShape::Round, size);
        primary(&mut editor, &[(x + size as i32 / 2, 6)]);
        x += size as i32 + 1;
    }
    // Square stamps with the secondary colour.
    for (size, x) in [(1, 2), (2, 6), (3, 11), (4, 17), (5, 24)] {
        editor.brush = Brush::new(BrushShape::Square, size);
        drag(&mut editor, PointerButton::Secondary, &[(x, 15)]);
    }
    // Thick lines.
    editor.brush = Brush::new(BrushShape::Round, 3);
    primary(&mut editor, &[(3, 22), (28, 20)]);
    editor.brush = Brush::new(BrushShape::Square, 2);
    drag(
        &mut editor,
        PointerButton::Secondary,
        &[(3, 27), (16, 30), (28, 26)],
    );
    check("pencil_brushes", &editor);
}

#[test]
fn pencil_replaces_pixels_and_strokes_run_off_the_canvas() {
    let mut editor = editor();
    editor.brush = Brush::new(BrushShape::Square, 6);
    primary(&mut editor, &[(-4, 8), (36, 8)]);
    // A translucent colour replaces what is there; it does not blend with it.
    editor.primary = Rgba8::new(0xF2, 0xCE, 0x3B, 0x80);
    primary(&mut editor, &[(16, -3), (16, 35)]);
    editor.primary = YELLOW;
    editor.brush = Brush::new(BrushShape::Round, 1);
    primary(&mut editor, &[(0, 31), (31, 31), (31, 0)]);
    check("pencil_replace", &editor);
}

#[test]
fn eraser_cuts_through_a_filled_canvas() {
    let mut editor = editor();
    editor.brush = Brush::new(BrushShape::Square, 32);
    primary(&mut editor, &[(16, 16)]);

    assert!(editor.set_active_tool("eraser"));
    editor.brush = Brush::new(BrushShape::Round, 5);
    primary(&mut editor, &[(4, 4), (27, 27)]);
    editor.brush = Brush::new(BrushShape::Square, 3);
    // The eraser erases with either button.
    drag(&mut editor, PointerButton::Secondary, &[(2, 24), (14, 24)]);
    editor.brush = Brush::new(BrushShape::Round, 1);
    primary(&mut editor, &[(26, 5)]);
    primary(&mut editor, &[(28, 5), (28, 9)]);
    check("eraser", &editor);
}

#[test]
fn eyedropper_picks_the_colours_later_strokes_use() {
    let mut editor = editor();
    editor.brush = Brush::new(BrushShape::Square, 4);
    primary(&mut editor, &[(4, 4)]);
    drag(&mut editor, PointerButton::Secondary, &[(12, 4)]);
    editor.primary = YELLOW;
    editor.secondary = YELLOW;

    // Pick blue into primary and red into secondary, the other way round from before.
    assert!(editor.set_active_tool("eyedropper"));
    primary(&mut editor, &[(12, 4)]);
    drag(&mut editor, PointerButton::Secondary, &[(4, 4)]);
    // An empty pixel is not picked.
    primary(&mut editor, &[(28, 28)]);

    assert!(editor.set_active_tool("pencil"));
    editor.brush = Brush::new(BrushShape::Round, 2);
    primary(&mut editor, &[(4, 16), (27, 16)]);
    drag(&mut editor, PointerButton::Secondary, &[(4, 24), (27, 24)]);
    check("eyedropper", &editor);
}
