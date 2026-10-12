//! Tests for tilemap layers and slices as a whole: through the document, with undo.

use crate::color::Rgba8;
use crate::document::{Document, Transaction};
use crate::editor::Editor;
use crate::geom::{Point, Rect};
use crate::image::Image;
use crate::sprite::{LayerId, Slot, Sprite, SpriteError};
use crate::tiles::{Slice, TilesetId};
use crate::tool::{Modifiers, PointerButton, PointerEvent};

const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
const BLUE: Rgba8 = Rgba8::rgb(0, 0, 255);
const L1: LayerId = LayerId(1);

fn filled(w: u32, h: u32, color: Rgba8) -> Image {
    Image::from_pixels(w, h, vec![color; (w * h) as usize]).unwrap()
}

/// A 16×8 document with a tilemap layer of 4×4 tiles on top of the first layer. Returns the
/// document and the tilemap layer's id.
fn document() -> (Document, LayerId) {
    let mut doc = Document::new(Sprite::new(16, 8).unwrap());
    let map = doc
        .transact("Tilemap", |tx| {
            let tileset = tx.add_tileset("Ground", 4, 4);
            tx.add_tilemap_layer(Slot::new(None, 1), "Map", tileset)
        })
        .unwrap();
    (doc, map)
}

fn write(doc: &mut Document, layer: LayerId, rect: Rect, color: Rgba8) {
    doc.transact("Write", |tx| {
        tx.set_pixels(layer, 0, rect, &filled(rect.w, rect.h, color))
    })
    .unwrap();
}

fn cells(doc: &Document, layer: LayerId) -> Vec<u32> {
    match doc.sprite().grid(layer, 0) {
        Some(grid) => grid.cells().to_vec(),
        None => vec![0; 8],
    }
}

fn tiles(doc: &Document) -> usize {
    doc.sprite().tilesets()[0].len()
}

/// Runs `edit` as one transaction and checks that undo and redo restore the sprite exactly.
fn undoes_exactly<T>(
    doc: &mut Document,
    edit: impl FnOnce(&mut Transaction<'_>) -> Result<T, SpriteError>,
) -> T {
    let before = doc.sprite().clone();
    let steps = doc.history_cursor();
    let value = doc.transact("Edit", edit).unwrap();
    assert_eq!(doc.history_cursor(), steps + 1, "not exactly one undo step");
    let after = doc.sprite().clone();
    assert!(doc.undo());
    assert_eq!(*doc.sprite(), before, "undo did not restore the sprite");
    assert!(doc.redo());
    assert_eq!(*doc.sprite(), after, "redo did not restore the change");
    value
}

#[test]
fn drawing_on_an_empty_cell_makes_a_tile_and_alike_cells_share_it() {
    let (mut doc, map) = document();
    assert_eq!(tiles(&doc), 0);
    // A red dot in the corner of the first cell.
    undoes_exactly(&mut doc, |tx| {
        tx.set_pixels(map, 0, Rect::new(0, 0, 1, 1), &filled(1, 1, RED))
    });
    assert_eq!(cells(&doc, map), [1, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(tiles(&doc), 1);
    assert_eq!(doc.sprite().pixel(map, 0, 0, 0), RED);

    // The same dot in another cell is the same tile, not a second one.
    write(&mut doc, map, Rect::new(8, 4, 1, 1), RED);
    assert_eq!(cells(&doc, map), [1, 0, 0, 0, 0, 0, 1, 0]);
    assert_eq!(tiles(&doc), 1);
    // A different picture is a new tile.
    write(&mut doc, map, Rect::new(5, 1, 2, 2), BLUE);
    assert_eq!(cells(&doc, map), [1, 2, 0, 0, 0, 0, 1, 0]);
    assert_eq!(tiles(&doc), 2);
    // The first layer is an ordinary one and was never touched.
    assert!(doc.sprite().cel(L1, 0).is_none());
}

/// The "done when" of Phase 6: editing one tile updates all its instances.
#[test]
fn editing_one_tile_updates_all_its_instances() {
    let (mut doc, map) = document();
    write(&mut doc, map, Rect::new(0, 0, 1, 1), RED);
    write(&mut doc, map, Rect::new(8, 4, 1, 1), RED);
    write(&mut doc, map, Rect::new(12, 0, 1, 1), RED);
    assert_eq!(cells(&doc, map), [1, 0, 0, 1, 0, 0, 1, 0]);

    // One pixel drawn in the middle of one of the three…
    undoes_exactly(&mut doc, |tx| {
        tx.set_pixels(map, 0, Rect::new(10, 6, 1, 1), &filled(1, 1, BLUE))
    });
    // …shows in all three, and it is still one tile.
    for (cx, cy) in [(0, 0), (8, 4), (12, 0)] {
        assert_eq!(
            doc.sprite().pixel(map, 0, cx + 2, cy + 2),
            BLUE,
            "{cx}, {cy}"
        );
        assert_eq!(doc.sprite().pixel(map, 0, cx, cy), RED);
    }
    assert_eq!(tiles(&doc), 1);
    assert_eq!(cells(&doc, map), [1, 0, 0, 1, 0, 0, 1, 0]);
    // The flattened picture shows it too.
    let flat = crate::compositor::compose(doc.sprite(), 0);
    assert_eq!((flat.get(2, 2), flat.get(14, 2)), (BLUE, BLUE));

    // One undo takes it out of all three.
    doc.undo();
    for (cx, cy) in [(0, 0), (8, 4), (12, 0)] {
        assert_eq!(
            doc.sprite().pixel(map, 0, cx + 2, cy + 2),
            Rgba8::TRANSPARENT
        );
    }
}

#[test]
fn a_stroke_across_two_instances_of_a_tile_adds_up() {
    let (mut doc, map) = document();
    write(&mut doc, map, Rect::new(0, 0, 1, 1), RED);
    write(&mut doc, map, Rect::new(4, 0, 1, 1), RED);
    assert_eq!(cells(&doc, map)[..2], [1, 1]);

    // One write that puts a pixel at a different place in each of the two cells: as a
    // stroke does, it carries the pixels as they were for everything it did not change.
    let mut pixels = doc.sprite().read_region(map, 0, Rect::new(0, 0, 8, 4));
    pixels.set(3, 3, BLUE);
    pixels.set(5, 2, BLUE);
    undoes_exactly(&mut doc, |tx| {
        tx.set_pixels(map, 0, Rect::new(0, 0, 8, 4), &pixels)
    });
    // Both changes are in the tile, so both cells show both.
    for cell in [0, 4] {
        assert_eq!(doc.sprite().pixel(map, 0, cell + 3, 3), BLUE);
        assert_eq!(doc.sprite().pixel(map, 0, cell + 1, 2), BLUE);
        assert_eq!(doc.sprite().pixel(map, 0, cell, 0), RED);
    }
    assert_eq!(tiles(&doc), 1);
}

#[test]
fn tiles_that_become_alike_are_merged_and_empty_ones_dropped() {
    let (mut doc, map) = document();
    write(&mut doc, map, Rect::new(0, 0, 4, 4), RED);
    write(&mut doc, map, Rect::new(4, 0, 4, 4), BLUE);
    write(&mut doc, map, Rect::new(8, 0, 2, 2), BLUE);
    assert_eq!(cells(&doc, map)[..3], [1, 2, 3]);

    // Painting the blue tile red makes it the red tile.
    undoes_exactly(&mut doc, |tx| {
        tx.set_pixels(map, 0, Rect::new(4, 0, 4, 4), &filled(4, 4, RED))
    });
    assert_eq!(cells(&doc, map)[..3], [1, 1, 2]);
    assert_eq!(tiles(&doc), 2);

    // Erasing all of a tile takes it off the map and out of the set.
    undoes_exactly(&mut doc, |tx| {
        tx.set_pixels(map, 0, Rect::new(8, 0, 4, 4), &Image::new(4, 4))
    });
    assert_eq!(cells(&doc, map)[..3], [1, 1, 0]);
    assert_eq!(tiles(&doc), 1);
    // Both red cells are one tile: erasing one erases the other.
    write(&mut doc, map, Rect::new(0, 0, 4, 4), Rgba8::TRANSPARENT);
    assert!(doc.sprite().grid(map, 0).is_none());
    assert!(doc.sprite().cel(map, 0).is_none());
    assert_eq!(tiles(&doc), 0);
}

#[test]
fn tiles_are_placed_by_number() {
    let (mut doc, map) = document();
    write(&mut doc, map, Rect::new(0, 0, 4, 4), RED);
    undoes_exactly(&mut doc, |tx| {
        tx.set_tiles(map, 0, &[(2, 1, 1), (3, 1, 1), (9, 9, 1), (1, 0, 7)])
    });
    assert_eq!(cells(&doc, map), [1, 0, 0, 0, 0, 0, 1, 1]);
    assert_eq!(doc.sprite().pixel(map, 0, 13, 6), RED);
    // Placing "no tile" clears a cell; the tile itself stays in the set.
    undoes_exactly(&mut doc, |tx| tx.set_tiles(map, 0, &[(0, 0, 0), (2, 1, 0)]));
    assert_eq!(cells(&doc, map), [0, 0, 0, 0, 0, 0, 0, 1]);
    assert_eq!(tiles(&doc), 1);

    assert_eq!(
        doc.transact("Tiles", |tx| tx.set_tiles(L1, 0, &[(0, 0, 1)])),
        Err(SpriteError::NotATilemap(1))
    );
    assert_eq!(
        doc.transact("Tiles", |tx| tx.set_tiles(map, 3, &[(0, 0, 1)])),
        Err(SpriteError::UnknownFrame(3))
    );
    assert_eq!(
        doc.transact("Add", |tx| {
            tx.add_tilemap_layer(Slot::new(None, 0), "Stray", TilesetId(9))
        }),
        Err(SpriteError::UnknownTileset(9))
    );
}

#[test]
fn two_layers_and_frames_of_one_tileset_change_together() {
    let (mut doc, map) = document();
    let tileset = doc.sprite().tilesets()[0].id;
    let other = doc
        .transact("Tilemap", |tx| {
            tx.add_frame(1, None)?;
            tx.add_tilemap_layer(Slot::new(None, 2), "Other", tileset)
        })
        .unwrap();
    write(&mut doc, map, Rect::new(0, 0, 4, 4), RED);
    doc.transact("Place", |tx| {
        tx.set_tiles(other, 0, &[(3, 1, 1)])?;
        tx.set_tiles(map, 1, &[(1, 0, 1)])
    })
    .unwrap();

    undoes_exactly(&mut doc, |tx| {
        tx.set_pixels(other, 0, Rect::new(13, 5, 1, 1), &filled(1, 1, BLUE))
    });
    assert_eq!(doc.sprite().pixel(map, 0, 1, 1), BLUE);
    assert_eq!(doc.sprite().pixel(map, 1, 5, 1), BLUE);
    assert_eq!(doc.sprite().pixel(other, 0, 13, 5), BLUE);

    // Frames carry their tiles along; so does a copy of the layer.
    undoes_exactly(&mut doc, |tx| tx.move_frame(0, 1));
    assert_eq!(doc.sprite().grid(map, 0).unwrap().get(1, 0), 1);
    assert_eq!(doc.sprite().grid(map, 1).unwrap().get(0, 0), 1);
    undoes_exactly(&mut doc, |tx| tx.remove_frame(0));
    assert_eq!(doc.sprite().grid(map, 0).unwrap().get(0, 0), 1);
    let copy = undoes_exactly(&mut doc, |tx| tx.duplicate_layer(map));
    assert_eq!(doc.sprite().grid(copy, 0), doc.sprite().grid(map, 0));
    write(&mut doc, copy, Rect::new(0, 3, 1, 1), BLUE);
    assert_eq!(doc.sprite().pixel(map, 0, 0, 3), BLUE);
    // Deleting a tilemap layer and bringing it back keeps its tiles.
    undoes_exactly(&mut doc, |tx| tx.remove_layer(other));
}

#[test]
fn what_tilemaps_do_not_allow_is_refused() {
    let (mut doc, map) = document();
    write(&mut doc, map, Rect::new(0, 0, 4, 4), RED);
    doc.transact("Frame", |tx| tx.add_frame(1, None)).unwrap();
    let refused = [
        doc.transact("x", |tx| tx.resize_canvas(32, 8, 0, 0)),
        doc.transact("x", |tx| tx.scale_sprite(32, 16)),
    ];
    assert_eq!(
        refused,
        [Err(SpriteError::HasTilemaps), Err(SpriteError::HasTilemaps)]
    );
    let refused = [
        doc.transact("x", |tx| tx.move_cel((map, 0), (map, 1), true)),
        doc.transact("x", |tx| tx.move_cel((L1, 0), (map, 1), false)),
        doc.transact("x", |tx| tx.link_cels(map, 0, &[1])),
    ];
    for result in refused {
        assert_eq!(result, Err(SpriteError::TilemapCel));
    }
    // A layer cannot be merged down onto a tilemap, but a tilemap can be merged down.
    let upper = doc
        .transact("Add", |tx| tx.add_layer(Slot::new(None, 2), "Upper", false))
        .unwrap();
    assert_eq!(
        doc.transact("x", |tx| tx.merge_down(upper)),
        Err(SpriteError::NothingBelow)
    );
    assert_eq!(doc.transact("Merge", |tx| tx.merge_down(map)), Ok(L1));
    assert_eq!(doc.sprite().pixel(L1, 0, 2, 2), RED);
    // A locked tilemap takes neither pixels nor tiles.
    doc.undo();
    doc.transact("Lock", |tx| {
        tx.update_layer(map, |props| {
            if let Some(image) = &mut props.image {
                image.locked = true;
            }
        })
    })
    .unwrap();
    assert!(doc.sprite().layer(map).unwrap().as_image().unwrap().locked);
    let locked = SpriteError::LayerLocked(map.0);
    assert_eq!(
        doc.transact("x", |tx| tx.set_tiles(map, 0, &[(0, 0, 0)])),
        Err(locked)
    );
}

fn event(x: i32, y: i32, button: PointerButton) -> PointerEvent {
    PointerEvent {
        position: Point::new(x, y),
        button,
        modifiers: Modifiers::default(),
    }
}

fn drag(editor: &mut Editor, button: PointerButton, path: &[(i32, i32)]) {
    editor.pointer_down(event(path[0].0, path[0].1, button));
    for &(x, y) in &path[1..] {
        editor.pointer_move(event(x, y, button));
    }
    let last = path[path.len() - 1];
    editor.pointer_up(event(last.0, last.1, button));
}

fn editor() -> (Editor, LayerId) {
    let (mut doc, map) = document();
    doc.set_active_layer(map);
    let mut editor = Editor::new();
    editor.add_document(doc);
    editor.primary = RED;
    (editor, map)
}

#[test]
fn the_pencil_draws_tiles_in_auto_mode_and_the_stamp_places_them() {
    let (mut editor, map) = editor();
    assert_eq!(editor.draw_block(), None);
    // Auto mode: the pencil draws, and the cells it crosses become tiles.
    drag(&mut editor, PointerButton::Primary, &[(1, 1), (6, 1)]);
    let doc = editor.document().unwrap();
    assert_eq!(cells(doc, map)[..2], [1, 2]);
    assert_eq!(doc.history().collect::<Vec<_>>().last(), Some(&"Pencil"));

    // The stamp places the picked tile along a drag, as one step, and shows it meanwhile.
    assert!(editor.set_active_tool("tile_stamp"));
    editor.options.tile = 2;
    editor.pointer_down(event(9, 5, PointerButton::Primary));
    editor.pointer_move(event(13, 6, PointerButton::Primary));
    assert_eq!(editor.overlay().unwrap().buffer.get(14, 5), Some(RED));
    assert_eq!(cells(editor.document().unwrap(), map)[6..], [0, 0]);
    editor.pointer_up(event(13, 6, PointerButton::Primary));
    let doc = editor.document().unwrap();
    assert_eq!(cells(doc, map), [1, 2, 0, 0, 0, 0, 2, 2]);
    assert_eq!(
        doc.history().collect::<Vec<_>>().last(),
        Some(&"Tile Stamp")
    );
    // A fast drag leaves no cell out on the way: every cell the straight line between two
    // pointer positions passes through gets the tile.
    editor.options.tile = 1;
    drag(&mut editor, PointerButton::Primary, &[(2, 6), (15, 2)]);
    let mut expected = vec![1, 2, 0, 0, 0, 0, 2, 2];
    for p in crate::stroke::line(Point::new(2, 6), Point::new(15, 2)) {
        expected[(p.y / 4 * 4 + p.x / 4) as usize] = 1;
    }
    assert!(expected.iter().filter(|tile| **tile == 1).count() >= 5);
    assert_eq!(cells(editor.document().unwrap(), map), expected);
    editor.document_mut().unwrap().undo();
    editor.options.tile = 2;
    // The other button takes tiles away; outside the map nothing happens.
    drag(&mut editor, PointerButton::Secondary, &[(0, 0), (2, -30)]);
    assert_eq!(cells(editor.document().unwrap(), map)[..2], [0, 2]);

    // Stamp mode: pixel tools leave the tilemap alone, and say why.
    editor.options.tile_auto = false;
    assert_eq!(editor.draw_block(), None);
    assert!(editor.set_active_tool("pencil"));
    assert_eq!(editor.draw_block(), Some(SpriteError::StampMode));
    let steps = editor.document().unwrap().history().len();
    drag(&mut editor, PointerButton::Primary, &[(1, 6), (14, 6)]);
    assert_eq!(editor.document().unwrap().history().len(), steps);
    // The stamp has nothing to do on an ordinary layer.
    editor.document_mut().unwrap().set_active_layer(L1);
    assert_eq!(editor.draw_block(), None);
    assert!(editor.set_active_tool("tile_stamp"));
    assert_eq!(editor.draw_block(), Some(SpriteError::NotATilemap(1)));
    drag(&mut editor, PointerButton::Primary, &[(1, 1)]);
    assert_eq!(editor.document().unwrap().history().len(), steps);
}

fn slice(name: &str, rect: Rect) -> Slice {
    Slice {
        name: name.to_owned(),
        rect,
        center: None,
        pivot: None,
        color: BLUE,
    }
}

#[test]
fn slices_are_added_changed_and_removed() {
    let (mut doc, _) = document();
    let index = undoes_exactly(&mut doc, |tx| {
        tx.add_slice(slice("Button", Rect::new(1, 1, 6, 4)))
    });
    assert_eq!(index, 0);
    let mut nine = slice("Panel", Rect::new(8, 0, 8, 8));
    nine.center = Some(Rect::new(2, 2, 4, 4));
    nine.pivot = Some(Point::new(4, 7));
    undoes_exactly(&mut doc, |tx| tx.set_slice(0, nine.clone()));
    assert_eq!(doc.sprite().slices(), [nine]);
    // Slices are not pixels: nothing is redrawn.
    doc.take_damage();
    undoes_exactly(&mut doc, |tx| {
        tx.add_slice(slice("Other", Rect::new(0, 0, 2, 2)))
    });
    assert!(doc.take_damage().is_empty());

    doc.set_active_slice(Some(1));
    undoes_exactly(&mut doc, |tx| tx.remove_slice(0));
    doc.transact("Remove", |tx| tx.remove_slice(0)).unwrap();
    // The selected slice is gone, so nothing is selected.
    assert_eq!(doc.active_slice(), None);

    for bad in [
        slice(" ", Rect::new(0, 0, 2, 2)),
        slice("Empty", Rect::EMPTY),
    ] {
        let result = doc.transact("x", |tx| tx.add_slice(bad.clone()));
        assert_eq!(result, Err(SpriteError::InvalidSlice));
    }
    assert_eq!(
        doc.transact("x", |tx| tx.remove_slice(4)),
        Err(SpriteError::UnknownSlice(4))
    );
    assert_eq!(
        doc.transact("x", |tx| tx.set_slice(4, slice("x", Rect::new(0, 0, 1, 1)))),
        Err(SpriteError::UnknownSlice(4))
    );
}

#[test]
fn the_slice_tool_makes_and_selects_slices() {
    let (mut editor, _) = editor();
    assert!(editor.set_active_tool("slice"));
    assert_eq!(editor.draw_block(), None);
    editor.pointer_down(event(2, 1, PointerButton::Primary));
    editor.pointer_move(event(7, 5, PointerButton::Primary));
    assert!(editor.outline().is_some());
    editor.pointer_up(event(7, 5, PointerButton::Primary));
    drag(&mut editor, PointerButton::Primary, &[(10, 2), (40, 40)]);

    let doc = editor.document().unwrap();
    let rects: Vec<Rect> = doc.sprite().slices().iter().map(|s| s.rect).collect();
    assert_eq!(rects, [Rect::new(2, 1, 6, 5), Rect::new(10, 2, 6, 6)]);
    assert_eq!(doc.sprite().slices()[1].name, "Slice 2");
    assert_eq!(doc.active_slice(), Some(1));
    assert_eq!(
        doc.history().collect::<Vec<_>>()[1..],
        ["New Slice", "New Slice"]
    );

    // A click picks the slice under it, or none.
    drag(&mut editor, PointerButton::Primary, &[(3, 3)]);
    assert_eq!(editor.document().unwrap().active_slice(), Some(0));
    drag(&mut editor, PointerButton::Primary, &[(8, 7)]);
    assert_eq!(editor.document().unwrap().active_slice(), None);
    // A drag wholly off the canvas makes nothing.
    drag(&mut editor, PointerButton::Primary, &[(30, 30), (40, 40)]);
    assert_eq!(editor.document().unwrap().sprite().slices().len(), 2);
}
