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
use acms_core::geom::{Point, Rect};
use acms_core::image::Image;
use acms_core::pattern::Pattern;
use acms_core::selection::Mask;
use acms_core::sprite::{BlendMode, LayerId, Slot, Sprite, Symmetry};
use acms_core::stroke::{Brush, BrushShape};
use acms_core::tool::{InkKind, Modifiers, PointerButton, PointerEvent};
use acms_core::{edit, palette};
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
    check_image(name, &compositor::compose(sprite, 0));
}

fn check_image(name: &str, actual: &Image) {
    let path = golden_dir().join(format!("{name}.png"));

    let write = |path: &PathBuf| {
        let mut bytes = Vec::new();
        encode_png(actual, &mut bytes).unwrap();
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
    if decode_png(&bytes).unwrap() != *actual {
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

/// Fills a rectangle of a layer with one colour.
fn fill(document: &mut Document, layer: LayerId, rect: Rect, color: Rgba8) {
    let pixels = Image::from_pixels(rect.w, rect.h, vec![color; rect.area()]).unwrap();
    document
        .transact("Fill", |tx| tx.set_pixels(layer, 0, rect, &pixels))
        .unwrap();
}

fn add_layer(document: &mut Document, parent: Option<LayerId>, group: bool) -> LayerId {
    let index = document.sprite().siblings(parent).unwrap().len();
    document
        .transact("Add", |tx| {
            tx.add_layer(Slot::new(parent, index), "Layer", group)
        })
        .unwrap()
}

fn style(document: &mut Document, layer: LayerId, opacity: u8, blend: BlendMode) {
    document
        .transact("Style", |tx| {
            tx.update_layer(layer, |props| {
                if let Some(image) = &mut props.image {
                    image.opacity = opacity;
                    image.blend = blend;
                }
            })
        })
        .unwrap();
}

fn hide(document: &mut Document, layer: LayerId) {
    document
        .transact("Hide", |tx| {
            tx.update_layer(layer, |props| props.visible = false)
        })
        .unwrap();
}

/// Four vertical stripes — dark, saturated, light, mid grey — with the bottom rows left empty,
/// under a layer with an opaque band and a half-transparent band that both stick out to the
/// right of nothing and over the empty rows.
fn blend_scene(mode: BlendMode) -> Document {
    let mut document = Document::new(Sprite::new(32, 32).unwrap());
    let stripes = [
        Rgba8::rgb(30, 40, 90),
        Rgba8::rgb(200, 60, 60),
        Rgba8::rgb(240, 220, 120),
        Rgba8::rgb(128, 128, 128),
    ];
    for (index, color) in stripes.into_iter().enumerate() {
        fill(
            &mut document,
            LayerId(1),
            Rect::new(index as i32 * 8, 0, 8, 24),
            color,
        );
    }

    let top = add_layer(&mut document, None, false);
    fill(
        &mut document,
        top,
        Rect::new(2, 4, 28, 8),
        Rgba8::rgb(40, 180, 170),
    );
    fill(
        &mut document,
        top,
        Rect::new(2, 14, 28, 8),
        Rgba8::new(240, 140, 40, 128),
    );
    fill(
        &mut document,
        top,
        Rect::new(12, 20, 8, 10),
        Rgba8::rgb(250, 250, 250),
    );
    style(&mut document, top, 255, mode);
    document
}

#[test]
fn compositing_each_blend_mode() {
    for mode in BlendMode::ALL {
        let document = blend_scene(mode);
        let name = format!("blend_{}", mode.id());
        check_image(&name, &compositor::compose(document.sprite(), 0));
    }
}

#[test]
fn compositing_opacity_groups_and_hidden_layers() {
    let mut document = Document::new(Sprite::new(32, 32).unwrap());
    // A dark backdrop with a light frame.
    fill(
        &mut document,
        LayerId(1),
        Rect::new(0, 0, 32, 32),
        Rgba8::rgb(230, 230, 235),
    );
    fill(
        &mut document,
        LayerId(1),
        Rect::new(2, 2, 28, 28),
        Rgba8::rgb(40, 44, 60),
    );

    // Three squares fading out: opacity 255, 128 and 48.
    for (index, opacity) in [255, 128, 48].into_iter().enumerate() {
        let layer = add_layer(&mut document, None, false);
        fill(
            &mut document,
            layer,
            Rect::new(4 + index as i32 * 8, 4, 7, 7),
            Rgba8::rgb(240, 90, 60),
        );
        style(&mut document, layer, opacity, BlendMode::Normal);
    }

    // A group: its two layers overlap, the upper one multiplied at 200.
    let group = add_layer(&mut document, None, true);
    let lower = add_layer(&mut document, Some(group), false);
    fill(
        &mut document,
        lower,
        Rect::new(4, 14, 14, 7),
        Rgba8::rgb(90, 200, 120),
    );
    let upper = add_layer(&mut document, Some(group), false);
    fill(
        &mut document,
        upper,
        Rect::new(10, 16, 14, 7),
        Rgba8::rgb(120, 140, 250),
    );
    style(&mut document, upper, 200, BlendMode::Multiply);
    // A hidden layer inside the visible group must not show.
    let hidden_inside = add_layer(&mut document, Some(group), false);
    fill(
        &mut document,
        hidden_inside,
        Rect::new(0, 0, 32, 32),
        Rgba8::rgb(255, 0, 255),
    );
    hide(&mut document, hidden_inside);

    // A hidden group: its visible child must not show either.
    let hidden_group = add_layer(&mut document, None, true);
    let child = add_layer(&mut document, Some(hidden_group), false);
    fill(
        &mut document,
        child,
        Rect::new(0, 24, 32, 8),
        Rgba8::rgb(255, 255, 0),
    );
    hide(&mut document, hidden_group);

    // On top of everything: a translucent bar set to Add, and a layer at opacity 0.
    let glow = add_layer(&mut document, None, false);
    fill(
        &mut document,
        glow,
        Rect::new(2, 25, 28, 4),
        Rgba8::new(90, 90, 160, 200),
    );
    style(&mut document, glow, 255, BlendMode::Add);
    let invisible = add_layer(&mut document, None, false);
    fill(
        &mut document,
        invisible,
        Rect::new(0, 0, 32, 32),
        Rgba8::rgb(0, 255, 0),
    );
    style(&mut document, invisible, 0, BlendMode::Normal);

    check_image(
        "layers_opacity_groups",
        &compositor::compose(document.sprite(), 0),
    );
}

#[test]
fn merging_down_and_flattening_do_not_change_the_picture() {
    for mode in BlendMode::ALL {
        let mut document = blend_scene(mode);
        let picture = compositor::compose(document.sprite(), 0);
        let top = document.sprite().layers()[1].id;

        document.transact("Merge", |tx| tx.merge_down(top)).unwrap();
        assert_eq!(
            compositor::compose(document.sprite(), 0),
            picture,
            "merge, {mode:?}"
        );
        document.undo();
        document.transact("Flatten", |tx| tx.flatten()).unwrap();
        assert_eq!(
            compositor::compose(document.sprite(), 0),
            picture,
            "flatten, {mode:?}"
        );
        // Both leave exactly the image the golden test above pins down.
        check_image(&format!("blend_{}", mode.id()), &picture);
    }
}

fn tool(editor: &mut Editor, id: &str) {
    assert!(editor.set_active_tool(id), "no tool `{id}`");
}

#[test]
fn line_rectangle_and_ellipse() {
    let mut editor = editor();
    tool(&mut editor, "line");
    primary(&mut editor, &[(1, 1), (30, 5)]);
    editor.brush = Brush::new(BrushShape::Round, 3);
    drag(&mut editor, PointerButton::Secondary, &[(2, 9), (29, 7)]);

    editor.brush = Brush::new(BrushShape::Round, 1);
    tool(&mut editor, "rectangle");
    primary(&mut editor, &[(1, 12), (9, 19)]);
    editor.options.shape_filled = true;
    drag(&mut editor, PointerButton::Secondary, &[(12, 12), (19, 19)]);
    editor.options.shape_filled = false;
    editor.brush = Brush::new(BrushShape::Square, 2);
    primary(&mut editor, &[(22, 12), (30, 19)]);

    editor.brush = Brush::new(BrushShape::Round, 1);
    tool(&mut editor, "ellipse");
    primary(&mut editor, &[(1, 22), (12, 30)]);
    editor.options.shape_filled = true;
    drag(&mut editor, PointerButton::Secondary, &[(15, 22), (21, 30)]);
    editor.options.shape_filled = false;
    primary(&mut editor, &[(24, 23), (30, 29)]);
    check("shapes", &editor);
}

#[test]
fn bucket_fill_contiguous_and_global() {
    let mut editor = editor();
    // Two frames and a diagonal that cuts the big one in half.
    tool(&mut editor, "rectangle");
    primary(&mut editor, &[(2, 2), (20, 20)]);
    primary(&mut editor, &[(23, 23), (30, 30)]);
    tool(&mut editor, "line");
    primary(&mut editor, &[(2, 2), (20, 20)]);

    tool(&mut editor, "bucket");
    drag(&mut editor, PointerButton::Secondary, &[(15, 5)]);
    editor.primary = YELLOW;
    primary(&mut editor, &[(26, 26)]);
    // Every red pixel, wherever it is, becomes yellow.
    editor.options.fill_contiguous = false;
    primary(&mut editor, &[(2, 2)]);
    check("bucket", &editor);
}

#[test]
fn gradients_inside_selections() {
    let mut editor = editor();
    // Left half: dithered, left to right.
    tool(&mut editor, "select_rectangle");
    primary(&mut editor, &[(0, 0), (15, 31)]);
    tool(&mut editor, "gradient");
    primary(&mut editor, &[(0, 0), (15, 0)]);

    // Right half: smooth, top to bottom, the colours the other way round.
    tool(&mut editor, "select_rectangle");
    primary(&mut editor, &[(16, 0), (31, 31)]);
    tool(&mut editor, "gradient");
    editor.options.gradient_dither = false;
    drag(&mut editor, PointerButton::Secondary, &[(16, 0), (16, 31)]);
    check("gradient", &editor);
}

#[test]
fn drawing_and_moving_inside_a_lasso_selection() {
    let mut editor = editor();
    tool(&mut editor, "lasso");
    primary(&mut editor, &[(16, 2), (29, 16), (16, 29), (3, 16)]);

    // A fat scribble across everything only lands inside the diamond.
    tool(&mut editor, "pencil");
    editor.brush = Brush::new(BrushShape::Round, 9);
    primary(&mut editor, &[(0, 6), (31, 12), (0, 20), (31, 26)]);
    editor.primary = YELLOW;
    editor.brush = Brush::new(BrushShape::Square, 3);
    primary(&mut editor, &[(0, 16), (31, 16)]);

    // Cut a square out of the middle and drag it to the corner.
    tool(&mut editor, "select_rectangle");
    primary(&mut editor, &[(12, 12), (19, 19)]);
    tool(&mut editor, "move");
    primary(&mut editor, &[(15, 15), (26, 26)]);
    check("selection", &editor);
}

// Phase 4: pixel-perfect, symmetry, custom brushes, dither and shade inks, tiled drawing.

/// A hand-drawn wobble: short runs with a step at every joint, the way a mouse moves.
fn wobble(dx: i32, dy: i32) -> Vec<(i32, i32)> {
    [
        (2, 2),
        (3, 2),
        (3, 3),
        (5, 3),
        (5, 4),
        (6, 4),
        (6, 6),
        (9, 6),
        (9, 7),
        (10, 7),
        (10, 9),
        (14, 9),
        (14, 10),
        (20, 12),
        (20, 13),
        (28, 13),
    ]
    .iter()
    .map(|&(x, y)| (x + dx, y + dy))
    .collect()
}

fn select(editor: &mut Editor, rect: Rect) {
    let document = editor.document_mut().expect("a document is open");
    let (width, height) = (document.sprite().width(), document.sprite().height());
    document
        .transact("Select", |tx| {
            tx.set_selection(Some(Mask::from_rect(width, height, rect)))
        })
        .unwrap();
}

fn deselect(editor: &mut Editor) {
    let document = editor.document_mut().expect("a document is open");
    document
        .transact("Deselect", |tx| tx.set_selection(None))
        .unwrap();
}

fn set_symmetry(editor: &mut Editor, symmetry: Symmetry) {
    let document = editor.document_mut().expect("a document is open");
    document.set_symmetry(symmetry);
}

#[test]
fn pixel_perfect_removes_the_corners_of_a_freehand_line() {
    let mut editor = editor();
    // The same line twice: as drawn above, filtered below.
    primary(&mut editor, &wobble(0, 0));
    editor.options.pixel_perfect = true;
    drag(&mut editor, PointerButton::Secondary, &wobble(0, 15));
    check("pixel_perfect", &editor);
}

#[test]
fn symmetry_modes() {
    let cases = [
        ("symmetry_horizontal", Symmetry::Horizontal),
        ("symmetry_vertical", Symmetry::Vertical),
        ("symmetry_both", Symmetry::Both),
        ("symmetry_radial_4", Symmetry::Radial(4)),
        ("symmetry_radial_6", Symmetry::Radial(6)),
    ];
    for (name, symmetry) in cases {
        let mut editor = editor();
        set_symmetry(&mut editor, symmetry);
        // A freehand curve, a thick dot, and a shape.
        primary(&mut editor, &[(3, 4), (8, 3), (12, 6), (13, 11)]);
        editor.brush = Brush::new(BrushShape::Round, 3);
        drag(&mut editor, PointerButton::Secondary, &[(5, 10)]);
        editor.brush = Brush::new(BrushShape::Round, 1);
        editor.set_active_tool("rectangle");
        drag(&mut editor, PointerButton::Secondary, &[(9, 12), (12, 14)]);
        check(name, &editor);
    }
}

#[test]
fn custom_brush_made_from_the_selection() {
    let mut editor = editor();
    // A little flower to pick up.
    primary(&mut editor, &[(3, 2)]);
    primary(&mut editor, &[(2, 3), (4, 3)]);
    primary(&mut editor, &[(3, 4)]);
    editor.primary = YELLOW;
    primary(&mut editor, &[(3, 3)]);
    select(&mut editor, Rect::new(1, 1, 5, 5));
    let document = editor.document().expect("a document is open");
    editor.brush = edit::brush_from_selection(document.sprite(), LayerId(1), 0).unwrap();
    deselect(&mut editor);

    // Single stamps keep the flower's own colours, whatever the pencil's colour is.
    editor.primary = BLUE;
    for x in [10, 15, 20, 25] {
        primary(&mut editor, &[(x, 4)]);
    }
    // Dragged, it smears; with the dither ink only its shape is used.
    primary(&mut editor, &[(4, 12), (26, 14)]);
    editor.options.ink = InkKind::Dither;
    editor.options.pattern = Pattern::Checker;
    editor.secondary = YELLOW;
    primary(&mut editor, &[(4, 21), (26, 19)]);
    editor.secondary = BLUE;
    // And the eraser cuts its shape out of what is there.
    editor.options.ink = InkKind::Simple;
    editor.brush = Brush::new(BrushShape::Square, 6);
    primary(&mut editor, &[(4, 27), (27, 27)]);
    select(&mut editor, Rect::new(1, 1, 5, 5));
    let document = editor.document().expect("a document is open");
    editor.brush = edit::brush_from_selection(document.sprite(), LayerId(1), 0).unwrap();
    deselect(&mut editor);
    editor.set_active_tool("eraser");
    for x in [6, 13, 20, 27] {
        primary(&mut editor, &[(x, 27)]);
    }
    check("custom_brush", &editor);
}

#[test]
fn dither_ink_patterns_and_densities() {
    let mut editor = editor();
    editor.options.ink = InkKind::Dither;
    editor.brush = Brush::new(BrushShape::Square, 4);

    // One row per Bayer matrix, the density rising from left to right.
    let rows = [
        (Pattern::Bayer2, 2),
        (Pattern::Bayer4, 7),
        (Pattern::Bayer8, 12),
    ];
    for (pattern, y) in rows {
        editor.options.pattern = pattern;
        for (step, density) in [0, 13, 25, 38, 50, 63, 75, 100].into_iter().enumerate() {
            editor.options.dither_density = density;
            let x = 2 + 4 * step as i32;
            primary(&mut editor, &[(x, y)]);
        }
    }
    editor.options.dither_density = 50;
    editor.options.pattern = Pattern::Checker;
    primary(&mut editor, &[(2, 17), (14, 17)]);
    // The other button swaps the two colours.
    drag(&mut editor, PointerButton::Secondary, &[(18, 17), (30, 17)]);

    // A custom pattern: a 3×3 tile with a diagonal, picked up from the canvas.
    editor.options.ink = InkKind::Simple;
    editor.brush = Brush::new(BrushShape::Round, 1);
    primary(&mut editor, &[(0, 29), (2, 31)]);
    select(&mut editor, Rect::new(0, 29, 3, 3));
    let document = editor.document().expect("a document is open");
    editor.options.pattern =
        edit::pattern_from_selection(document.sprite(), LayerId(1), 0).unwrap();
    deselect(&mut editor);
    editor.options.ink = InkKind::Dither;
    editor.brush = Brush::new(BrushShape::Square, 6);
    primary(&mut editor, &[(8, 24), (28, 24)]);
    // A filled shape takes the pattern too.
    editor.options.pattern = Pattern::Bayer4;
    editor.options.dither_density = 25;
    editor.options.shape_filled = true;
    editor.brush = Brush::new(BrushShape::Round, 1);
    editor.set_active_tool("ellipse");
    primary(&mut editor, &[(8, 28), (28, 31)]);
    check("dither_ink", &editor);
}

#[test]
fn shade_ink_steps_along_a_ramp() {
    let mut editor = editor();
    let ramp = palette::ramp_between(Rgba8::rgb(40, 20, 60), Rgba8::rgb(250, 230, 160), 6);
    let document = editor.document_mut().expect("a document is open");
    document
        .transact("Palette", |tx| {
            let with_ramp = palette::with_ramp(tx.sprite().palette(), &ramp)
                .expect("six different colours make a ramp");
            tx.set_palette(with_ramp);
            Ok::<_, acms_core::sprite::SpriteError>(())
        })
        .unwrap();

    // Five bands in the ramp's colours, and one in a colour that is in no ramp.
    editor.brush = Brush::new(BrushShape::Square, 4);
    for (band, color) in ramp[..5].iter().enumerate() {
        editor.primary = *color;
        let y = 2 + 5 * band as i32;
        primary(&mut editor, &[(2, y), (29, y)]);
    }
    editor.primary = BLUE;
    primary(&mut editor, &[(2, 28), (29, 28)]);

    // Shading down the left lightens every band by one step, down the right darkens it; in
    // the middle a stroke that crosses itself still only moves each pixel once.
    editor.options.ink = InkKind::Shade;
    editor.brush = Brush::new(BrushShape::Round, 5);
    primary(&mut editor, &[(6, 0), (6, 31)]);
    drag(&mut editor, PointerButton::Secondary, &[(26, 0), (26, 31)]);
    editor.brush = Brush::new(BrushShape::Round, 3);
    primary(&mut editor, &[(13, 1), (19, 30), (19, 1), (13, 30)]);
    check("shade_ink", &editor);
}

#[test]
fn tiled_strokes_wrap_around() {
    let mut editor = editor();
    editor.options.tiled = true;
    editor.brush = Brush::new(BrushShape::Round, 5);
    // Off the right edge and back in on the left; a blob on a corner; a shape over the top.
    primary(&mut editor, &[(24, 8), (40, 14)]);
    drag(&mut editor, PointerButton::Secondary, &[(0, 0)]);
    editor.brush = Brush::new(BrushShape::Round, 1);
    editor.set_active_tool("ellipse");
    primary(&mut editor, &[(10, 26), (22, 38)]);
    editor.set_active_tool("line");
    editor.primary = YELLOW;
    primary(&mut editor, &[(-6, 20), (12, 22)]);
    check("tiled", &editor);
}

/// The "done when" of Phase 4, as a picture.
#[test]
fn symmetry_pixel_perfect_and_dither_in_one_stroke() {
    let mut editor = editor();
    set_symmetry(&mut editor, Symmetry::Both);
    editor.options.pixel_perfect = true;
    editor.options.ink = InkKind::Dither;
    editor.options.pattern = Pattern::Checker;
    let path: Vec<(i32, i32)> = wobble(0, 0)
        .into_iter()
        .filter(|&(x, y)| x < 15 && y < 15)
        .collect();
    primary(&mut editor, &path);
    let document = editor.document().expect("a document is open");
    assert_eq!(document.history().collect::<Vec<_>>(), ["Pencil"]);
    check("symmetry_pixel_perfect_dither", &editor);
}

// Phase 6: tilemaps.

#[test]
fn tile_stamp_and_auto_mode() {
    let mut editor = editor();
    let document = editor.document_mut().expect("a document is open");
    let map = document
        .transact("Tilemap", |tx| {
            let tileset = tx.add_tileset("Tiles", 8, 8);
            tx.add_tilemap_layer(Slot::new(None, 1), "Map", tileset)
        })
        .unwrap();
    document.set_active_layer(map);

    // Auto mode: a cross drawn in the first cell becomes tile 1.
    editor.brush = Brush::new(BrushShape::Round, 2);
    primary(&mut editor, &[(1, 1), (6, 6)]);
    drag(&mut editor, PointerButton::Secondary, &[(1, 6), (6, 1)]);
    // The stamp places it along two drags.
    editor.set_active_tool("tile_stamp");
    editor.options.tile = 1;
    primary(&mut editor, &[(12, 4), (28, 4)]);
    primary(&mut editor, &[(4, 20), (28, 28)]);
    // A dot drawn on one of them shows on all of them.
    editor.set_active_tool("pencil");
    editor.primary = YELLOW;
    primary(&mut editor, &[(19, 27)]);
    // The other button of the stamp takes a tile away.
    editor.set_active_tool("tile_stamp");
    drag(&mut editor, PointerButton::Secondary, &[(12, 4)]);
    check("tiles", &editor);
}
