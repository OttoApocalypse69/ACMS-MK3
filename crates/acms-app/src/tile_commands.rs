//! Commands for tilemap layers, tiles, slices and their exports.

use std::path::Path;

use acms_core::command::{Command, CommandError, CommandRegistry, ParamKind, Params};
use acms_core::geom::{Point, Rect};
use acms_core::sprite::{Slot, Sprite};
use acms_core::tiles::{MAX_TILE_SIZE, Slice, Tileset, TilesetId};
use acms_io::tiled::{export_csv, export_tiled, export_tileset};

use crate::layer_commands::transact;
use crate::state::{AppState, Dialog, FileRequest, SizePurpose};

type CommandResult = Result<(), CommandError>;

/// Tile size offered by the New Tilemap Layer dialog.
const DEFAULT_TILE_SIZE: u32 = 16;

fn has_document(state: &AppState) -> bool {
    state.editor.document().is_some()
}

fn has_tilemap(state: &AppState) -> bool {
    state
        .editor
        .document()
        .is_some_and(|document| !document.sprite().tilesets().is_empty())
}

fn has_slice(state: &AppState) -> bool {
    state
        .editor
        .document()
        .is_some_and(|document| document.active_slice().is_some())
}

fn sprite(state: &AppState) -> Result<&Sprite, CommandError> {
    state
        .editor
        .document()
        .map(|document| document.sprite())
        .ok_or_else(|| CommandError::failed("No sprite is open."))
}

/// The tileset tile commands are about: the selected layer's if it is a tilemap, otherwise
/// the sprite's first one.
pub fn active_tileset(state: &AppState) -> Option<&Tileset> {
    let document = state.editor.document()?;
    let sprite = document.sprite();
    sprite
        .tileset_of(document.active_layer())
        .or(sprite.tilesets().first())
}

fn new_tilemap(state: &mut AppState, params: &Params) -> CommandResult {
    let (Some(width), Some(height)) = (params.int("width"), params.int("height")) else {
        sprite(state)?;
        state.dialog = Some(Dialog::Size {
            purpose: SizePurpose::NewTilemap,
            width: DEFAULT_TILE_SIZE,
            height: DEFAULT_TILE_SIZE,
            anchor: crate::state::Anchor::default(),
        });
        return Ok(());
    };
    let side = |n: i64| u32::try_from(n).unwrap_or(0);
    let (width, height) = (side(width), side(height));
    let valid = 1..=MAX_TILE_SIZE;
    if !valid.contains(&width) || !valid.contains(&height) {
        return Err(CommandError::failed(format!(
            "A tile must be between 1 and {MAX_TILE_SIZE} pixels on each side."
        )));
    }
    let sprite = sprite(state)?;
    let top = Slot::new(None, sprite.layers().len());
    let existing = params
        .int("tileset")
        .map(|id| TilesetId(u32::try_from(id).unwrap_or(u32::MAX)));
    let number = sprite.tilesets().len() + 1;
    let layer = transact(state, "New Tilemap Layer", |tx| {
        let tileset = match existing {
            Some(id) => id,
            None => tx.add_tileset(&format!("Tileset {number}"), width, height),
        };
        tx.add_tilemap_layer(top, &format!("Tilemap {number}"), tileset)
    })?;
    if let Some(document) = state.editor.document_mut() {
        document.set_active_layer(layer);
    }
    Ok(())
}

fn tile_select(state: &mut AppState, params: &Params) -> CommandResult {
    let count = active_tileset(state).map_or(0, Tileset::len) as i64;
    let tile = params.int("tile").unwrap_or(0);
    if !(0..=count).contains(&tile) {
        return Err(CommandError::failed(format!("There is no tile {tile}.")));
    }
    state.editor.options.tile = tile as u32;
    Ok(())
}

/// The selected slice and its position.
fn active_slice(state: &AppState) -> Result<(usize, Slice), CommandError> {
    let document = state
        .editor
        .document()
        .ok_or_else(|| CommandError::failed("No sprite is open."))?;
    let index = document.active_slice().ok_or_else(|| {
        CommandError::failed("No slice is selected: click one with the Slice tool first.")
    })?;
    Ok((index, document.sprite().slices()[index].clone()))
}

fn slice_edit(state: &mut AppState, params: &Params) -> CommandResult {
    let (index, slice) = active_slice(state)?;
    let Some(name) = params.text("name") else {
        state.dialog = Some(Dialog::Slice {
            name: slice.name,
            rect: slice.rect,
            center: slice.center,
            pivot: slice.pivot,
        });
        return Ok(());
    };
    let number = |key: &str, current: i64| params.int(key).unwrap_or(current);
    let size = |value: i64| u32::try_from(value.max(0)).unwrap_or(u32::MAX);
    let place = |value: i64| value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let rect = |prefix: &str, current: Rect| {
        Rect::new(
            place(number(&format!("{prefix}x"), i64::from(current.x))),
            place(number(&format!("{prefix}y"), i64::from(current.y))),
            size(number(&format!("{prefix}width"), i64::from(current.w))),
            size(number(&format!("{prefix}height"), i64::from(current.h))),
        )
    };
    // `nine` and `pivot` switch the two optional parts on and off; without them they stay.
    let center = match params.bool("nine") {
        Some(false) => None,
        Some(true) => Some(rect("center_", slice.center.unwrap_or(Rect::EMPTY))),
        None => slice.center,
    };
    let pivot = match params.bool("pivot") {
        Some(false) => None,
        Some(true) => {
            let old = slice.pivot.unwrap_or_default();
            Some(Point::new(
                place(number("pivot_x", i64::from(old.x))),
                place(number("pivot_y", i64::from(old.y))),
            ))
        }
        None => slice.pivot,
    };
    let edited = Slice {
        name: name.trim().to_owned(),
        rect: rect("", slice.rect),
        center,
        pivot,
        color: params.color("color").unwrap_or(slice.color),
    };
    transact(state, "Edit Slice", |tx| tx.set_slice(index, edited))
}

fn slice_delete(state: &mut AppState, _: &Params) -> CommandResult {
    let (index, _) = active_slice(state)?;
    transact(state, "Delete Slice", |tx| tx.remove_slice(index))
}

/// The path an export was given, or else asks for one and returns `None`.
fn path_or_ask<'a>(
    state: &mut AppState,
    params: &'a Params,
    command: &'static str,
    file_type: (&'static str, &'static str),
) -> Option<&'a Path> {
    match params.text("path") {
        Some(path) => Some(Path::new(path)),
        None => {
            state.file_request = Some(FileRequest::Export {
                command,
                params: params.clone(),
                file_type,
            });
            None
        }
    }
}

fn could_not_export(path: &Path, err: impl std::fmt::Display) -> CommandError {
    CommandError::failed(format!("Could not export {}: {err}", path.display()))
}

fn export_tileset_command(state: &mut AppState, params: &Params) -> CommandResult {
    let file_type = ("PNG image", "png");
    let Some(path) = path_or_ask(state, params, "file.export_tileset", file_type) else {
        return Ok(());
    };
    let tileset =
        active_tileset(state).ok_or_else(|| CommandError::failed("The sprite has no tileset."))?;
    export_tileset(tileset, path).map_err(|err| could_not_export(path, err))?;
    let count = tileset.len();
    state
        .log
        .push(format!("Exported {} ({count} tiles)", path.display()));
    Ok(())
}

/// Exports the tilemap layers of the frame being shown with `write`, and logs the files.
fn export_map(
    state: &mut AppState,
    path: &Path,
    write: fn(&Sprite, usize, &Path) -> Result<Vec<std::path::PathBuf>, acms_io::IoError>,
) -> CommandResult {
    let document = state
        .editor
        .document()
        .ok_or_else(|| CommandError::failed("No sprite is open."))?;
    let files = write(document.sprite(), document.active_frame(), path)
        .map_err(|err| could_not_export(path, err))?;
    let names: Vec<String> = files
        .iter()
        .filter_map(|file| Some(file.file_name()?.to_string_lossy().into_owned()))
        .collect();
    state.log.push(format!("Exported {}", names.join(", ")));
    Ok(())
}

pub fn register(registry: &mut CommandRegistry<AppState>) -> Result<(), CommandError> {
    use ParamKind::{Bool, Color, Int, Text};

    registry.register(
        Command::new("layer.new_tilemap", "New Tilemap Layer…", new_tilemap)
            .optional_param("width", Int)
            .optional_param("height", Int)
            .optional_param("tileset", Int)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("tile.select", "Select Tile", tile_select)
            .param("tile", Int)
            .enabled_when(has_tilemap),
    )?;
    registry.register(
        Command::new(
            "tile.toggle_mode",
            "Toggle Tilemap Mode (Stamp / Auto)",
            |state: &mut AppState, params| {
                let options = &mut state.editor.options;
                options.tile_auto = params.bool("auto").unwrap_or(!options.tile_auto);
                Ok(())
            },
        )
        .optional_param("auto", Bool),
    )?;

    let mut edit = Command::new("slice.edit", "Edit Slice…", slice_edit)
        .optional_param("name", Text)
        .optional_param("color", Color)
        .optional_param("nine", Bool)
        .optional_param("pivot", Bool)
        .enabled_when(has_slice);
    for key in [
        "x",
        "y",
        "width",
        "height",
        "center_x",
        "center_y",
        "center_width",
        "center_height",
        "pivot_x",
        "pivot_y",
    ] {
        edit = edit.optional_param(key, Int);
    }
    registry.register(edit)?;
    registry.register(
        Command::new("slice.delete", "Delete Slice", slice_delete).enabled_when(has_slice),
    )?;

    registry.register(
        Command::new(
            "file.export_tileset",
            "Export Tileset…",
            export_tileset_command,
        )
        .optional_param("path", Text)
        .enabled_when(has_tilemap),
    )?;
    registry.register(
        Command::new(
            "file.export_tilemap",
            "Export Tilemap for Tiled…",
            |state: &mut AppState, params| {
                let file_type = ("Tiled map", "tmj");
                match path_or_ask(state, params, "file.export_tilemap", file_type) {
                    Some(path) => export_map(state, path, export_tiled),
                    None => Ok(()),
                }
            },
        )
        .optional_param("path", Text)
        .enabled_when(has_tilemap),
    )?;
    registry.register(
        Command::new(
            "file.export_tilemap_csv",
            "Export Tilemap as CSV…",
            |state: &mut AppState, params| {
                let file_type = ("CSV", "csv");
                match path_or_ask(state, params, "file.export_tilemap_csv", file_type) {
                    Some(path) => export_map(state, path, export_csv),
                    None => Ok(()),
                }
            },
        )
        .optional_param("path", Text)
        .enabled_when(has_tilemap),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testing::{Fixture, RED};
    use crate::config::TempFile;
    use acms_core::color::Rgba8;
    use acms_core::sprite::LayerId;
    use acms_core::tool::{Modifiers, PointerButton, PointerEvent};

    fn text(file: &TempFile) -> String {
        file.path().to_string_lossy().into_owned()
    }

    fn message(f: &mut Fixture) -> String {
        match f.state.dialog.take() {
            Some(Dialog::Message { text, .. }) => text,
            other => panic!("expected a message, found {other:?}"),
        }
    }

    fn history(f: &Fixture) -> Vec<String> {
        f.document().history().map(str::to_owned).collect()
    }

    fn drag(f: &mut Fixture, button: PointerButton, path: &[(i32, i32)]) {
        let event = |&(x, y): &(i32, i32)| PointerEvent {
            position: Point::new(x, y),
            button,
            modifiers: Modifiers::default(),
        };
        f.state.editor.pointer_down(event(&path[0]));
        for point in &path[1..] {
            f.state.editor.pointer_move(event(point));
        }
        f.state.editor.pointer_up(event(&path[path.len() - 1]));
    }

    /// A 16×16 sprite with a tilemap layer of 4×4 tiles selected. Returns its id.
    fn with_tilemap() -> (Fixture, LayerId) {
        let mut f = Fixture::with_sprite();
        let size = Params::new().with("width", 4_i64).with("height", 4_i64);
        f.run_with("layer.new_tilemap", size);
        let layer = f.document().active_layer();
        (f, layer)
    }

    fn cells(f: &Fixture, layer: LayerId) -> Vec<u32> {
        match f.document().sprite().grid(layer, 0) {
            Some(grid) => grid.cells().to_vec(),
            None => vec![0; 16],
        }
    }

    #[test]
    fn a_tilemap_layer_is_made_with_its_tileset() {
        let mut f = Fixture::with_sprite();
        assert!(!f.is_enabled("tile.select") && !f.is_enabled("file.export_tilemap"));
        f.run("layer.new_tilemap");
        assert!(matches!(
            f.state.dialog.take(),
            Some(Dialog::Size {
                purpose: SizePurpose::NewTilemap,
                width: 16,
                height: 16,
                ..
            })
        ));

        let (mut f, layer) = with_tilemap();
        let sprite = f.document().sprite();
        assert_eq!(sprite.layers().last().map(|top| top.id), Some(layer));
        let tileset = sprite.tileset_of(layer).unwrap();
        assert_eq!((tileset.tile_width(), tileset.tile_height()), (4, 4));
        assert_eq!(history(&f), ["New Tilemap Layer"]);
        // A second layer can share the first one's tileset.
        let shared = Params::new()
            .with("width", 4_i64)
            .with("height", 4_i64)
            .with("tileset", 1_i64);
        f.run_with("layer.new_tilemap", shared);
        assert_eq!(f.document().sprite().tilesets().len(), 1);
        f.run("edit.undo");
        f.run("edit.undo");
        assert!(f.document().sprite().tilesets().is_empty());

        let huge = Params::new().with("width", 4_i64).with("height", 999_i64);
        f.run_with("layer.new_tilemap", huge);
        assert!(message(&mut f).contains("between 1 and 256"));
    }

    /// The "done when" through the app: draw a tile, place it elsewhere, edit one, see both.
    #[test]
    fn tiles_are_drawn_stamped_and_edited_through_the_tools() {
        let (mut f, layer) = with_tilemap();
        f.state.editor.primary = RED;
        drag(&mut f, PointerButton::Primary, &[(1, 1), (2, 2)]);
        assert_eq!(cells(&f, layer)[0], 1);

        f.run("tool.tile_stamp");
        f.run_with("tile.select", Params::new().with("tile", 1_i64));
        drag(&mut f, PointerButton::Primary, &[(13, 13)]);
        assert_eq!(cells(&f, layer)[15], 1);
        f.run_with("tile.select", Params::new().with("tile", 5_i64));
        assert!(message(&mut f).contains("no tile 5"));

        // Editing the stamped copy changes the original.
        f.run("tool.pencil");
        f.state.editor.primary = Rgba8::WHITE;
        drag(&mut f, PointerButton::Primary, &[(15, 12)]);
        assert_eq!(f.pixel_on(layer, 3, 0), Rgba8::WHITE);
        assert_eq!(f.document().sprite().tilesets()[0].len(), 1);
        f.run("edit.undo");
        assert_eq!(f.pixel_on(layer, 3, 0), Rgba8::TRANSPARENT);

        // In stamp mode the pencil leaves the tilemap alone.
        f.run("tile.toggle_mode");
        assert!(!f.state.editor.options.tile_auto);
        assert!(f.state.editor.draw_block().is_some());
        let steps = history(&f).len();
        drag(&mut f, PointerButton::Primary, &[(6, 6)]);
        assert_eq!(history(&f).len(), steps);
        f.run_with("tile.toggle_mode", Params::new().with("auto", true));
        assert!(f.state.editor.options.tile_auto);

        // The canvas size is fixed while there are tilemaps.
        let size = Params::new().with("width", 32_i64).with("height", 32_i64);
        f.run_with("sprite.canvas_size", size);
        assert!(message(&mut f).contains("tilemap"));
    }

    #[test]
    fn slices_are_made_with_the_tool_and_changed_with_the_command() {
        let mut f = Fixture::with_sprite();
        assert!(!f.is_enabled("slice.edit"));
        f.run("tool.slice");
        drag(&mut f, PointerButton::Primary, &[(2, 2), (9, 7)]);
        assert_eq!(f.document().active_slice(), Some(0));
        f.run("slice.edit");
        assert_eq!(
            f.state.dialog.take(),
            Some(Dialog::Slice {
                name: "Slice 1".to_owned(),
                rect: Rect::new(2, 2, 8, 6),
                center: None,
                pivot: None,
            })
        );

        let edit = Params::new()
            .with("name", " Button ")
            .with("x", 1_i64)
            .with("width", 10_i64)
            .with("nine", true)
            .with("center_x", 2_i64)
            .with("center_y", 2_i64)
            .with("center_width", 6_i64)
            .with("center_height", 2_i64)
            .with("pivot", true)
            .with("pivot_x", 5_i64)
            .with("pivot_y", 5_i64);
        f.run_with("slice.edit", edit);
        let slice = f.document().sprite().slices()[0].clone();
        assert_eq!(
            (slice.name.as_str(), slice.rect),
            ("Button", Rect::new(1, 2, 10, 6))
        );
        assert_eq!(slice.center, Some(Rect::new(2, 2, 6, 2)));
        assert_eq!(slice.pivot, Some(Point::new(5, 5)));
        // Leaving the switches out keeps both; switching one off removes it.
        f.run_with("slice.edit", Params::new().with("name", "Button"));
        assert_eq!(f.document().sprite().slices()[0], slice);
        let off = Params::new().with("name", "Button").with("nine", false);
        f.run_with("slice.edit", off);
        assert_eq!(f.document().sprite().slices()[0].center, None);
        assert_eq!(
            f.document().sprite().slices()[0].pivot,
            Some(Point::new(5, 5))
        );

        f.run_with("slice.edit", Params::new().with("name", " "));
        assert!(message(&mut f).contains("needs a name"));
        assert_eq!(history(&f), ["New Slice", "Edit Slice", "Edit Slice"]);
        f.run("slice.delete");
        assert!(f.document().sprite().slices().is_empty());
        assert!(!f.is_enabled("slice.delete"));
        f.run("edit.undo");
        assert_eq!(f.document().sprite().slices().len(), 1);
    }

    #[test]
    fn tilesets_and_tilemaps_are_exported_and_survive_saving() {
        let (mut f, layer) = with_tilemap();
        f.state.editor.primary = RED;
        drag(&mut f, PointerButton::Primary, &[(1, 1), (6, 1)]);
        for command in [
            "file.export_tileset",
            "file.export_tilemap",
            "file.export_tilemap_csv",
        ] {
            f.run(command);
            let Some(FileRequest::Export { command: asked, .. }) = f.state.file_request.take()
            else {
                panic!("expected a file request for {command}");
            };
            assert_eq!(asked, command);
        }

        let png = TempFile::path_only("tiles.png");
        f.run_with(
            "file.export_tileset",
            Params::new().with("path", text(&png)),
        );
        let image = acms_io::png::decode_png(&std::fs::read(png.path()).unwrap()).unwrap();
        assert_eq!((image.width(), image.height()), (8, 4));
        assert!(f.state.log.last().unwrap().ends_with("(2 tiles)"));

        let map = TempFile::path_only("level.tmj");
        f.run_with(
            "file.export_tilemap",
            Params::new().with("path", text(&map)),
        );
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(map.path()).unwrap()).unwrap();
        assert_eq!(json["layers"][0]["data"][1], 2);
        assert_eq!((&json["width"], &json["tilewidth"]), (&4.into(), &4.into()));
        let picture = json["tilesets"][0]["image"].as_str().unwrap();
        let picture = map.path().with_file_name(picture);
        assert!(picture.exists());
        std::fs::remove_file(picture).unwrap();

        let csv = TempFile::path_only("level.csv");
        f.run_with(
            "file.export_tilemap_csv",
            Params::new().with("path", text(&csv)),
        );
        let rows = std::fs::read_to_string(csv.path()).unwrap();
        assert!(rows.starts_with("0,1,-1,-1\n"));

        // Saved and opened again, the tiles are still tiles.
        let saved = TempFile::path_only("tiles.acms");
        f.run_with("file.save_as", Params::new().with("path", text(&saved)));
        let sprite = f.document().sprite().clone();
        f.run("file.close");
        f.run_with("file.open", Params::new().with("path", text(&saved)));
        assert_eq!(*f.document().sprite(), sprite);
        assert_eq!(cells(&f, layer)[..2], [1, 2]);

        // A sprite without tilemaps has nothing to export.
        let mut plain = Fixture::with_sprite();
        assert!(matches!(
            plain.try_run("file.export_tilemap", Params::new()),
            Err(CommandError::Disabled(_))
        ));
    }
}
